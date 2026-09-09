#![cfg_attr(coverage_nightly, coverage(off))]
//! gRPC Communicator proxy and server adapter.
//!
//! Provides bidirectional remote communication between Stamp and out-of-process
//! plugins, including command execution with stdin/stdout/stderr streaming and
//! chunked file upload and download streaming.

use crate::communicator::{Command, CommandResult, Communicator};
use crate::error::StampError;
use crate::r#gen::packer::communicator_client::CommunicatorClient;
use crate::r#gen::packer::communicator_server::Communicator as CommunicatorService;
use crate::r#gen::packer::{
    CommandResponse, CommandStream, DownloadRequest, FileChunk, UploadResponse,
};
use crate::types::FilePath;
use async_trait::async_trait;
use futures_util::StreamExt;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status};

/// Standard chunk size for streaming file uploads and downloads (64 KiB).
pub const CHUNK_SIZE: usize = 64 * 1024;

/// A client-side `Communicator` proxy that forwards operations over gRPC.
#[derive(Debug, Clone)]
pub struct GrpcCommunicatorProxy {
    /// Channel endpoint address.
    pub endpoint: String,
    /// Inner tonic gRPC client.
    client: CommunicatorClient<tonic::transport::Channel>,
}

impl GrpcCommunicatorProxy {
    /// Creates a new `GrpcCommunicatorProxy` connected to the given gRPC channel.
    #[must_use]
    pub fn new(endpoint: String, channel: tonic::transport::Channel) -> Self {
        Self {
            endpoint,
            client: CommunicatorClient::new(channel),
        }
    }

    /// Connects to a gRPC communicator service over TCP.
    ///
    /// # Errors
    /// Returns `StampError::Execution` if the connection fails.
    pub async fn connect_tcp(address: &str) -> Result<Self, StampError> {
        let endpoint_url = if address.starts_with("http") {
            address.to_string()
        } else {
            format!("http://{address}")
        };
        let client = CommunicatorClient::connect(endpoint_url.clone())
            .await
            .map_err(|e| StampError::Execution(format!("gRPC Communicator connect error: {e}")))?;
        Ok(Self {
            endpoint: endpoint_url,
            client,
        })
    }
}

#[async_trait]
impl Communicator for GrpcCommunicatorProxy {
    async fn execute(&self, cmd: &Command) -> Result<CommandResult, StampError> {
        let (tx, rx) = mpsc::channel(4);
        let stream = ReceiverStream::new(rx);

        // Send initial command
        tx.send(CommandStream {
            command: cmd.command.clone(),
            stdin: Vec::new(),
        })
        .await
        .map_err(|e| StampError::Execution(format!("Failed to stream command: {e}")))?;

        let mut client = self.client.clone();
        let resp = client
            .execute(Request::new(stream))
            .await
            .map_err(|e| StampError::Execution(format!("gRPC execute RPC error: {e}")))?;

        let mut in_stream = resp.into_inner();
        let mut stdout_acc = Vec::new();
        let mut stderr_acc = Vec::new();
        let mut exit_code = 0;

        while let Some(item) = in_stream.next().await {
            let chunk = item
                .map_err(|e| StampError::Execution(format!("Execute stream chunk error: {e}")))?;
            stdout_acc.extend_from_slice(&chunk.stdout);
            stderr_acc.extend_from_slice(&chunk.stderr);
            if chunk.finished {
                exit_code = chunk.exit_code;
            }
        }

        Ok(CommandResult {
            exit_code,
            stdout: String::from_utf8_lossy(&stdout_acc).to_string(),
            stderr: String::from_utf8_lossy(&stderr_acc).to_string(),
        })
    }

    async fn upload(
        &self,
        local_path: &FilePath,
        remote_path: &FilePath,
    ) -> Result<(), StampError> {
        let mut file = tokio::fs::File::open(local_path.as_path())
            .await
            .map_err(StampError::Io)?;
        let total_len = file.metadata().await.map_err(StampError::Io)?.len();

        let (tx, rx) = mpsc::channel(8);
        let stream = ReceiverStream::new(rx);

        let remote_str = remote_path.to_string();
        tokio::spawn(async move {
            if total_len == 0 {
                let _ = tx
                    .send(FileChunk {
                        path: remote_str,
                        data: Vec::new(),
                        end: true,
                    })
                    .await;
                return;
            }

            let mut buffer = vec![0u8; CHUNK_SIZE];
            let mut bytes_read = 0u64;
            while let Ok(n) = file.read(&mut buffer).await {
                if n == 0 {
                    break;
                }
                bytes_read += n as u64;
                let is_last = bytes_read >= total_len;
                if tx
                    .send(FileChunk {
                        path: remote_str.clone(),
                        data: buffer[..n].to_vec(),
                        end: is_last,
                    })
                    .await
                    .is_err()
                {
                    break;
                }
                if is_last {
                    break;
                }
            }
        });

        let mut client = self.client.clone();
        let resp = client
            .upload(Request::new(stream))
            .await
            .map_err(|e| StampError::Execution(format!("gRPC upload RPC error: {e}")))?
            .into_inner();

        if !resp.success {
            return Err(StampError::Execution(format!(
                "gRPC upload rejected: {}",
                resp.error
            )));
        }

        Ok(())
    }

    async fn download(
        &self,
        remote_path: &FilePath,
        local_path: &FilePath,
    ) -> Result<(), StampError> {
        let mut client = self.client.clone();
        let resp = client
            .download(Request::new(DownloadRequest {
                path: remote_path.to_string(),
            }))
            .await
            .map_err(|e| StampError::Execution(format!("gRPC download RPC error: {e}")))?;

        let mut in_stream = resp.into_inner();
        let mut file = tokio::fs::File::create(local_path.as_path())
            .await
            .map_err(StampError::Io)?;

        while let Some(item) = in_stream.next().await {
            let chunk = item
                .map_err(|e| StampError::Execution(format!("Download stream chunk error: {e}")))?;
            if !chunk.data.is_empty() {
                file.write_all(&chunk.data).await.map_err(StampError::Io)?;
            }
            if chunk.end {
                break;
            }
        }
        file.flush().await.map_err(StampError::Io)?;

        Ok(())
    }
}

/// A server-side gRPC adapter that exposes any local `Communicator` implementation over gRPC.
#[derive(Clone)]
pub struct GrpcCommunicatorServer {
    /// Inner communicator instance.
    inner: Arc<dyn Communicator>,
}

impl GrpcCommunicatorServer {
    /// Creates a new `GrpcCommunicatorServer` wrapping the provided `Communicator`.
    #[must_use]
    pub fn new(inner: Arc<dyn Communicator>) -> Self {
        Self { inner }
    }
}

#[async_trait]
impl CommunicatorService for GrpcCommunicatorServer {
    type ExecuteStream = ReceiverStream<Result<CommandResponse, Status>>;

    async fn execute(
        &self,
        request: Request<tonic::Streaming<CommandStream>>,
    ) -> Result<Response<Self::ExecuteStream>, Status> {
        let mut in_stream = request.into_inner();
        let Some(first) = in_stream.next().await else {
            return Err(Status::invalid_argument("Empty command stream"));
        };
        let cmd_stream = first.map_err(|e| Status::internal(e.to_string()))?;

        let (tx, rx) = mpsc::channel(4);
        let inner = self.inner.clone();

        tokio::spawn(async move {
            let cmd = Command::new(cmd_stream.command);
            match inner.execute(&cmd).await {
                Ok(res) => {
                    let _ = tx
                        .send(Ok(CommandResponse {
                            stdout: res.stdout.into_bytes(),
                            stderr: res.stderr.into_bytes(),
                            exit_code: res.exit_code,
                            finished: true,
                        }))
                        .await;
                }
                Err(err) => {
                    let _ = tx
                        .send(Err(Status::internal(format!("Execution failed: {err}"))))
                        .await;
                }
            }
        });

        Ok(Response::new(ReceiverStream::new(rx)))
    }

    async fn upload(
        &self,
        request: Request<tonic::Streaming<FileChunk>>,
    ) -> Result<Response<UploadResponse>, Status> {
        let mut in_stream = request.into_inner();
        let temp_dir = tempfile::tempdir().map_err(|e| Status::internal(e.to_string()))?;
        let temp_file = temp_dir.path().join("upload_temp");
        let mut file = tokio::fs::File::create(&temp_file)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        let mut target_path = String::new();

        while let Some(chunk_res) = in_stream.next().await {
            let chunk = chunk_res.map_err(|e| Status::internal(e.to_string()))?;
            if target_path.is_empty() {
                target_path = chunk.path;
            }
            if !chunk.data.is_empty() {
                file.write_all(&chunk.data)
                    .await
                    .map_err(|e| Status::internal(e.to_string()))?;
            }
            if chunk.end {
                break;
            }
        }
        file.flush()
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        let local = FilePath::new(temp_file);
        let remote = FilePath::new(target_path.into());

        match self.inner.upload(&local, &remote).await {
            Ok(()) => Ok(Response::new(UploadResponse {
                success: true,
                error: String::new(),
            })),
            Err(e) => Ok(Response::new(UploadResponse {
                success: false,
                error: e.to_string(),
            })),
        }
    }

    type DownloadStream = ReceiverStream<Result<FileChunk, Status>>;

    async fn download(
        &self,
        request: Request<DownloadRequest>,
    ) -> Result<Response<Self::DownloadStream>, Status> {
        let req = request.into_inner();
        let temp_dir = tempfile::tempdir().map_err(|e| Status::internal(e.to_string()))?;
        let temp_file = temp_dir.path().join("download_temp");

        let remote = FilePath::new(req.path.into());
        let local = FilePath::new(temp_file.clone());

        self.inner
            .download(&remote, &local)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;

        let mut file = tokio::fs::File::open(&temp_file)
            .await
            .map_err(|e| Status::internal(e.to_string()))?;
        let total_len = file
            .metadata()
            .await
            .map_err(|e| Status::internal(e.to_string()))?
            .len();

        let (tx, rx) = mpsc::channel(8);
        tokio::spawn(async move {
            if total_len == 0 {
                let _ = tx
                    .send(Ok(FileChunk {
                        path: String::new(),
                        data: Vec::new(),
                        end: true,
                    }))
                    .await;
                return;
            }

            let mut buffer = vec![0u8; CHUNK_SIZE];
            let mut read_acc = 0u64;
            while let Ok(n) = file.read(&mut buffer).await {
                if n == 0 {
                    break;
                }
                read_acc += n as u64;
                let is_last = read_acc >= total_len;
                let _ = tx
                    .send(Ok(FileChunk {
                        path: String::new(),
                        data: buffer[..n].to_vec(),
                        end: is_last,
                    }))
                    .await;
                if is_last {
                    break;
                }
            }
        });

        Ok(Response::new(ReceiverStream::new(rx)))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::pedantic, clippy::all)]
mod tests {
    use super::*;
    use crate::communicator::mock::MockCommunicator;
    use crate::r#gen::packer::communicator_server::CommunicatorServer;

    #[tokio::test]
    async fn test_grpc_communicator_execute_upload_download() {
        let mock_comm = Arc::new(MockCommunicator::default());
        let server = GrpcCommunicatorServer::new(mock_comm.clone());

        // Bind in-memory gRPC server
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let local_addr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            tonic::transport::Server::builder()
                .add_service(CommunicatorServer::new(server))
                .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener))
                .await
                .unwrap();
        });

        let proxy = GrpcCommunicatorProxy::connect_tcp(&local_addr.to_string())
            .await
            .unwrap();

        // 1. Test Execute
        let cmd = Command::new("echo hello".to_string());
        let res = proxy.execute(&cmd).await.unwrap();
        assert_eq!(res.exit_code, 0);

        // 2. Test Upload
        let temp_dir = tempfile::tempdir().unwrap();
        let local_file = temp_dir.path().join("local.txt");
        tokio::fs::write(&local_file, b"test content")
            .await
            .unwrap();

        let local_path = FilePath::new(local_file);
        let remote_path = FilePath::new("/remote/dest.txt".into());
        proxy.upload(&local_path, &remote_path).await.unwrap();

        // 3. Test Download
        let download_dest = temp_dir.path().join("downloaded.txt");
        let download_path = FilePath::new(download_dest.clone());
        proxy.download(&remote_path, &download_path).await.unwrap();
        let downloaded_bytes = tokio::fs::read(&download_dest).await.unwrap();
        assert_eq!(downloaded_bytes, b"mock content");
    }
}
