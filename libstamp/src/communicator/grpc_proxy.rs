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
    ///
    /// # Arguments
    /// * `endpoint` - The endpoint URL string.
    /// * `channel` - The underlying gRPC transport channel.
    #[must_use]
    pub fn new(endpoint: String, channel: tonic::transport::Channel) -> Self {
        Self {
            endpoint,
            client: CommunicatorClient::new(channel),
        }
    }

    /// Connects to a gRPC communicator service over TCP.
    ///
    /// # Arguments
    /// * `address` - Host:port or URL string to connect to.
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

impl std::fmt::Debug for GrpcCommunicatorServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GrpcCommunicatorServer")
            .finish_non_exhaustive()
    }
}

impl GrpcCommunicatorServer {
    /// Creates a new `GrpcCommunicatorServer` wrapping the provided `Communicator`.
    ///
    /// # Arguments
    /// * `inner` - Inner communicator implementation to expose over gRPC.
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
mod tests {
    use super::*;
    use crate::communicator::mock::MockCommunicator;
    use crate::r#gen::packer::communicator_server::CommunicatorServer;

    /// Failing mock communicator for testing error paths.
    #[derive(Debug, Default)]
    struct FailingCommunicator;

    #[async_trait]
    impl Communicator for FailingCommunicator {
        async fn execute(&self, _cmd: &Command) -> Result<CommandResult, StampError> {
            Err(StampError::Execution(
                "mock inner execution error".to_string(),
            ))
        }

        async fn upload(
            &self,
            _local_path: &FilePath,
            _remote_path: &FilePath,
        ) -> Result<(), StampError> {
            Err(StampError::Execution("mock inner upload error".to_string()))
        }

        async fn download(
            &self,
            _remote_path: &FilePath,
            _local_path: &FilePath,
        ) -> Result<(), StampError> {
            Err(StampError::Execution(
                "mock inner download error".to_string(),
            ))
        }
    }

    /// Configurable mock communicator for testing custom payload sizes and operations.
    #[derive(Debug, Default)]
    struct ConfigurableCommunicator {
        payload: Vec<u8>,
    }

    #[async_trait]
    impl Communicator for ConfigurableCommunicator {
        async fn execute(&self, cmd: &Command) -> Result<CommandResult, StampError> {
            Ok(CommandResult {
                exit_code: 0,
                stdout: cmd.command.clone(),
                stderr: String::new(),
            })
        }

        async fn upload(
            &self,
            _local_path: &FilePath,
            _remote_path: &FilePath,
        ) -> Result<(), StampError> {
            Ok(())
        }

        async fn download(
            &self,
            _remote_path: &FilePath,
            local_path: &FilePath,
        ) -> Result<(), StampError> {
            tokio::fs::write(local_path.as_path(), &self.payload)
                .await
                .map_err(StampError::Io)?;
            Ok(())
        }
    }

    #[tokio::test]
    async fn test_grpc_proxy_struct_traits() -> Result<(), Box<dyn std::error::Error>> {
        let endpoint = "http://127.0.0.1:50051".to_string();
        let channel =
            tonic::transport::Channel::from_static("http://127.0.0.1:50051").connect_lazy();
        let proxy = GrpcCommunicatorProxy::new(endpoint.clone(), channel);
        assert_eq!(proxy.endpoint, endpoint);
        let dbg = format!("{proxy:?}");
        assert!(dbg.contains("GrpcCommunicatorProxy"));

        let mock_comm = Arc::new(MockCommunicator::default());
        let server = GrpcCommunicatorServer::new(mock_comm);
        let server_dbg = format!("{server:?}");
        assert!(server_dbg.contains("GrpcCommunicatorServer"));
        let server_clone = server.clone();
        assert!(format!("{server_clone:?}").contains("GrpcCommunicatorServer"));
        Ok(())
    }

    #[tokio::test]
    async fn test_grpc_connect_tcp_error() {
        let res = GrpcCommunicatorProxy::connect_tcp("127.0.0.1:1").await;
        assert!(res.is_err());
    }

    #[tokio::test]
    async fn test_grpc_dead_channel_errors() -> Result<(), Box<dyn std::error::Error>> {
        let channel = tonic::transport::Channel::from_static("http://127.0.0.1:1").connect_lazy();
        let proxy = GrpcCommunicatorProxy::new("http://127.0.0.1:1".to_string(), channel);
        let cmd = Command::new("ls".to_string());
        let res_exec = proxy.execute(&cmd).await;
        assert!(matches!(res_exec, Err(StampError::Execution(_))));

        let temp_dir = tempfile::tempdir()?;
        let local_file = temp_dir.path().join("local.txt");
        tokio::fs::write(&local_file, b"test").await?;
        let res_upload = proxy
            .upload(&FilePath::new(local_file), &FilePath::new("/remote".into()))
            .await;
        assert!(matches!(res_upload, Err(StampError::Execution(_))));
        Ok(())
    }

    #[tokio::test]
    async fn test_grpc_server_empty_command_stream() -> Result<(), Box<dyn std::error::Error>> {
        let mock_comm = Arc::new(MockCommunicator::default());
        let server = GrpcCommunicatorServer::new(mock_comm);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let local_addr = listener.local_addr()?;

        tokio::spawn(async move {
            let _ = tonic::transport::Server::builder()
                .add_service(CommunicatorServer::new(server))
                .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener))
                .await;
        });

        let mut client = CommunicatorClient::connect(format!("http://{local_addr}")).await?;
        let (tx, rx) = mpsc::channel::<CommandStream>(1);
        drop(tx);
        let stream = ReceiverStream::new(rx);
        let res = client.execute(Request::new(stream)).await;
        assert!(matches!(res, Err(s) if s.code() == tonic::Code::InvalidArgument));
        Ok(())
    }

    #[tokio::test]
    async fn test_grpc_communicator_execute_upload_download()
    -> Result<(), Box<dyn std::error::Error>> {
        let mock_comm = Arc::new(MockCommunicator::default());
        let server = GrpcCommunicatorServer::new(mock_comm.clone());

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let local_addr = listener.local_addr()?;

        tokio::spawn(async move {
            let _ = tonic::transport::Server::builder()
                .add_service(CommunicatorServer::new(server))
                .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener))
                .await;
        });

        // Test connect with and without "http://"
        let proxy_http =
            GrpcCommunicatorProxy::connect_tcp(&format!("http://{local_addr}")).await?;
        assert!(proxy_http.endpoint.starts_with("http://"));

        let proxy = GrpcCommunicatorProxy::connect_tcp(&local_addr.to_string()).await?;

        // 1. Test Execute
        let cmd = Command::new("echo hello".to_string());
        let res = proxy.execute(&cmd).await?;
        assert_eq!(res.exit_code, 0);

        // 2. Test Upload standard file
        let temp_dir = tempfile::tempdir()?;
        let local_file = temp_dir.path().join("local.txt");
        tokio::fs::write(&local_file, b"test content").await?;

        let local_path = FilePath::new(local_file);
        let remote_path = FilePath::new("/remote/dest.txt".into());
        proxy.upload(&local_path, &remote_path).await?;

        // 2b. Test Upload empty file (0 bytes)
        let empty_file = temp_dir.path().join("empty.txt");
        tokio::fs::write(&empty_file, b"").await?;
        let empty_local_path = FilePath::new(empty_file);
        let empty_remote_path = FilePath::new("/remote/empty.txt".into());
        proxy.upload(&empty_local_path, &empty_remote_path).await?;

        // 2c. Test Upload multi-chunk file (> 64 KiB)
        let large_file = temp_dir.path().join("large_upload.bin");
        let large_payload = vec![b'Q'; CHUNK_SIZE * 2 + 512];
        tokio::fs::write(&large_file, &large_payload).await?;
        let large_local_path = FilePath::new(large_file);
        let large_remote_path = FilePath::new("/remote/large.bin".into());
        proxy.upload(&large_local_path, &large_remote_path).await?;

        // 2d. Test Upload missing file
        let missing_path = FilePath::new(temp_dir.path().join("nonexistent_file.txt"));
        let missing_res = proxy.upload(&missing_path, &remote_path).await;
        assert!(matches!(missing_res, Err(StampError::Io(_))));

        // 3. Test Download standard file
        let download_dest = temp_dir.path().join("downloaded.txt");
        let download_path = FilePath::new(download_dest.clone());
        proxy.download(&remote_path, &download_path).await?;
        let downloaded_bytes = tokio::fs::read(&download_dest).await?;
        assert_eq!(downloaded_bytes, b"mock content");

        Ok(())
    }

    #[tokio::test]
    async fn test_grpc_communicator_empty_and_large_download()
    -> Result<(), Box<dyn std::error::Error>> {
        // Test empty download
        let empty_comm = Arc::new(ConfigurableCommunicator {
            payload: Vec::new(),
        });
        let server_empty = GrpcCommunicatorServer::new(empty_comm);
        let listener_empty = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let addr_empty = listener_empty.local_addr()?;
        tokio::spawn(async move {
            let _ = tonic::transport::Server::builder()
                .add_service(CommunicatorServer::new(server_empty))
                .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(
                    listener_empty,
                ))
                .await;
        });

        let proxy_empty = GrpcCommunicatorProxy::connect_tcp(&addr_empty.to_string()).await?;
        let temp_dir = tempfile::tempdir()?;
        let dest_empty = temp_dir.path().join("empty_dl.txt");
        let remote_path = FilePath::new("/remote/dl.txt".into());
        proxy_empty
            .download(&remote_path, &FilePath::new(dest_empty.clone()))
            .await?;
        let dl_empty_bytes = tokio::fs::read(&dest_empty).await?;
        assert_eq!(dl_empty_bytes.len(), 0);

        // Also exercise execute and upload on ConfigurableCommunicator
        let cmd = Command::new("configured".to_string());
        let res_cmd = proxy_empty.execute(&cmd).await?;
        assert_eq!(res_cmd.stdout, "configured");
        proxy_empty
            .upload(&FilePath::new(dest_empty), &remote_path)
            .await?;

        // Test large download
        let large_comm = Arc::new(ConfigurableCommunicator {
            payload: vec![b'Z'; CHUNK_SIZE * 2 + 1024],
        });
        let server_large = GrpcCommunicatorServer::new(large_comm);
        let listener_large = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let addr_large = listener_large.local_addr()?;
        tokio::spawn(async move {
            let _ = tonic::transport::Server::builder()
                .add_service(CommunicatorServer::new(server_large))
                .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(
                    listener_large,
                ))
                .await;
        });

        let proxy_large = GrpcCommunicatorProxy::connect_tcp(&addr_large.to_string()).await?;
        let dest_large = temp_dir.path().join("large_dl.bin");
        proxy_large
            .download(&remote_path, &FilePath::new(dest_large.clone()))
            .await?;
        let dl_large_bytes = tokio::fs::read(&dest_large).await?;
        assert_eq!(dl_large_bytes.len(), CHUNK_SIZE * 2 + 1024);

        Ok(())
    }

    #[tokio::test]
    async fn test_grpc_communicator_failing_paths() -> Result<(), Box<dyn std::error::Error>> {
        let failing_comm = Arc::new(FailingCommunicator);
        let server = GrpcCommunicatorServer::new(failing_comm);

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let local_addr = listener.local_addr()?;

        tokio::spawn(async move {
            let _ = tonic::transport::Server::builder()
                .add_service(CommunicatorServer::new(server))
                .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener))
                .await;
        });

        let proxy = GrpcCommunicatorProxy::connect_tcp(&local_addr.to_string()).await?;

        // 1. Failing execute
        let cmd = Command::new("fail".to_string());
        let res_exec = proxy.execute(&cmd).await;
        assert!(matches!(res_exec, Err(StampError::Execution(_))));

        // 2. Failing upload
        let temp_dir = tempfile::tempdir()?;
        let local_file = temp_dir.path().join("failing_upload.txt");
        tokio::fs::write(&local_file, b"data").await?;
        let res_upload = proxy
            .upload(&FilePath::new(local_file), &FilePath::new("/remote".into()))
            .await;
        assert!(
            matches!(res_upload, Err(StampError::Execution(msg)) if msg.contains("gRPC upload rejected"))
        );

        // 3. Failing download
        let res_download = proxy
            .download(
                &FilePath::new("/remote".into()),
                &FilePath::new(temp_dir.path().join("dl.txt")),
            )
            .await;
        assert!(matches!(res_download, Err(StampError::Execution(_))));

        Ok(())
    }
}
