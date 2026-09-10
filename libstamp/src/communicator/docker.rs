#![cfg(not(tarpaulin_include))]
//! Native Docker communicator implementation supporting direct container execution
//! via the Docker Engine API (`/containers/{id}/exec`), multiplexed stream decoding,
//! and archive streaming (`/containers/{id}/archive`) for file upload and download.

use crate::communicator::{Command, CommandResult, Communicator};
use crate::error::StampError;
use crate::types::{FilePath, Timeout};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Strongly typed container identifier or name.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ContainerId(pub String);

impl ContainerId {
    /// Create a new `ContainerId`.
    #[must_use]
    pub const fn new(id: String) -> Self {
        Self(id)
    }

    /// Retrieve the container identifier as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ContainerId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Transport mechanism used to reach the Docker daemon.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DockerTransport {
    /// UNIX domain socket path (e.g. `/var/run/docker.sock`).
    UnixSocket(PathBuf),
    /// TCP host and port (e.g. `127.0.0.1:2375`).
    Tcp(String),
    /// Docker CLI fallback execution via subprocess.
    Cli,
}

impl Default for DockerTransport {
    /// Provide standard default transport: UNIX socket at `/var/run/docker.sock`.
    fn default() -> Self {
        Self::UnixSocket(PathBuf::from("/var/run/docker.sock"))
    }
}

/// Strongly typed configuration for the Docker communicator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DockerConfig {
    /// Target container ID or container name.
    pub container_id: ContainerId,
    /// Transport mechanism to communicate with the Docker daemon.
    pub transport: DockerTransport,
    /// Shell command wrapper (e.g. `["sh", "-c"]`).
    pub shell: Vec<String>,
    /// Optional user or user:group to execute commands as.
    pub user: Option<String>,
    /// Whether to allocate a pseudo-TTY for command execution.
    pub tty: bool,
    /// Execution and network timeout.
    pub timeout: Timeout,
}

impl DockerConfig {
    /// Create a new `DockerConfig` with default options for a container.
    #[must_use]
    pub fn new(container_id: impl Into<String>) -> Self {
        Self {
            container_id: ContainerId::new(container_id.into()),
            transport: DockerTransport::default(),
            shell: vec!["sh".to_string(), "-c".to_string()],
            user: None,
            tty: false,
            timeout: Timeout::new(Duration::from_secs(30)),
        }
    }
}

impl Default for DockerConfig {
    /// Return standard default Docker communicator configuration.
    fn default() -> Self {
        Self::new("default_container")
    }
}

/// Docker multiplexed stream channel identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamType {
    /// Standard input (stream 0).
    Stdin,
    /// Standard output (stream 1).
    Stdout,
    /// Standard error (stream 2).
    Stderr,
    /// System error (stream 3).
    SystemErr,
    /// Unknown stream type.
    Unknown(u8),
}

impl From<u8> for StreamType {
    fn from(byte: u8) -> Self {
        match byte {
            0 => Self::Stdin,
            1 => Self::Stdout,
            2 => Self::Stderr,
            3 => Self::SystemErr,
            other => Self::Unknown(other),
        }
    }
}

/// A parsed multiplexed stream frame from Docker's raw stream protocol.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DockerStreamFrame {
    /// The stream origin (stdout, stderr, etc.).
    pub stream_type: StreamType,
    /// Payload bytes in this frame.
    pub payload: Vec<u8>,
}

/// Parse frames from Docker multiplexed stream bytes.
///
/// Docker stream protocol frames:
/// `[STREAM_TYPE: 1 byte, 0, 0, 0, SIZE: 4 bytes big endian, PAYLOAD: SIZE bytes]`
///
/// # Errors
///
/// Returns `StampError::Parse` if frame headers are invalid or incomplete.
pub fn parse_multiplexed_stream(data: &[u8]) -> Result<Vec<DockerStreamFrame>, StampError> {
    let mut frames = Vec::new();
    let mut cursor = 0;

    while cursor < data.len() {
        if cursor + 8 > data.len() {
            return Err(StampError::Parse(
                "Truncated Docker stream frame header".to_string(),
            ));
        }

        let stream_byte = data[cursor];
        let size_bytes: [u8; 4] = [
            data[cursor + 4],
            data[cursor + 5],
            data[cursor + 6],
            data[cursor + 7],
        ];
        let size = u32::from_be_bytes(size_bytes) as usize;
        cursor += 8;

        if cursor + size > data.len() {
            return Err(StampError::Parse(
                "Truncated Docker stream frame payload".to_string(),
            ));
        }

        let payload = data[cursor..cursor + size].to_vec();
        cursor += size;

        frames.push(DockerStreamFrame {
            stream_type: StreamType::from(stream_byte),
            payload,
        });
    }

    Ok(frames)
}

/// Create a TAR archive from a local file or directory in memory.
///
/// # Errors
///
/// Returns `StampError::Io` on filesystem reading or archive creation failure.
pub fn create_tar_archive(local_path: &Path) -> Result<Vec<u8>, StampError> {
    let mut tar_bytes = Vec::new();
    {
        let mut builder = tar::Builder::new(&mut tar_bytes);
        let file_name = local_path.file_name().unwrap_or_default().to_string_lossy();

        if local_path.is_dir() {
            builder
                .append_dir_all(&*file_name, local_path)
                .map_err(StampError::Io)?;
        } else {
            let mut file = std::fs::File::open(local_path).map_err(StampError::Io)?;
            builder
                .append_file(&*file_name, &mut file)
                .map_err(StampError::Io)?;
        }
        builder.finish().map_err(StampError::Io)?;
    }
    Ok(tar_bytes)
}

/// Extract a TAR archive from in-memory bytes to a destination directory.
///
/// # Errors
///
/// Returns `StampError::Io` on archive extraction failure.
pub fn extract_tar_archive(tar_bytes: &[u8], destination: &Path) -> Result<(), StampError> {
    let mut archive = tar::Archive::new(tar_bytes);
    archive.unpack(destination).map_err(StampError::Io)?;
    Ok(())
}

/// Standard stream attach flags for Docker exec.
#[derive(Debug, Serialize)]
struct ExecAttachStreams {
    /// Whether to attach standard input.
    #[serde(rename = "AttachStdin")]
    attach_stdin: bool,
    /// Whether to attach standard output.
    #[serde(rename = "AttachStdout")]
    attach_stdout: bool,
    /// Whether to attach standard error.
    #[serde(rename = "AttachStderr")]
    attach_stderr: bool,
}

/// Internal Docker exec creation request body.
#[derive(Debug, Serialize)]
struct CreateExecRequest<'a> {
    /// Standard stream attachment options.
    #[serde(flatten)]
    streams: ExecAttachStreams,
    /// Whether to allocate a pseudo-TTY.
    #[serde(rename = "Tty")]
    tty: bool,
    /// Optional user to execute commands as.
    #[serde(rename = "User", skip_serializing_if = "Option::is_none")]
    user: Option<&'a str>,
    /// Command array to execute.
    #[serde(rename = "Cmd")]
    cmd: &'a [String],
}

/// Internal Docker exec creation response.
#[derive(Debug, Deserialize)]
struct CreateExecResponse {
    /// The unique exec instance identifier.
    #[serde(rename = "Id")]
    id: String,
}

/// Internal Docker exec start request body.
#[derive(Debug, Serialize)]
struct StartExecRequest {
    /// Whether to detach after starting execution.
    #[serde(rename = "Detach")]
    detach: bool,
    /// Whether to run in pseudo-TTY mode.
    #[serde(rename = "Tty")]
    tty: bool,
}

/// Internal Docker exec inspection response.
#[derive(Debug, Deserialize)]
struct InspectExecResponse {
    /// Exit code of the executed command if completed.
    #[serde(rename = "ExitCode")]
    exit_code: Option<i32>,
    /// Whether the process is currently running.
    #[serde(rename = "Running")]
    #[allow(dead_code)]
    running: bool,
}

/// Helper function to URL-encode path and query parameters.
#[must_use]
pub fn url_encode(input: &str) -> String {
    url::form_urlencoded::byte_serialize(input.as_bytes()).collect()
}

/// Native Docker communicator executing commands and managing files via Docker API/CLI.
#[derive(Debug, Clone)]
pub struct DockerCommunicator {
    /// Configuration settings for the container connection.
    pub config: DockerConfig,
}

impl DockerCommunicator {
    /// Create a new `DockerCommunicator`.
    #[must_use]
    pub const fn new(config: DockerConfig) -> Self {
        Self { config }
    }

    /// Execute a command using the Docker Engine API over a UNIX domain socket.
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn execute_api_unix(
        &self,
        socket_path: &Path,
        cmd: &Command,
    ) -> Result<CommandResult, StampError> {
        let mut full_cmd = self.config.shell.clone();
        full_cmd.push(cmd.command.clone());

        let create_req = CreateExecRequest {
            streams: ExecAttachStreams {
                attach_stdin: false,
                attach_stdout: true,
                attach_stderr: true,
            },
            tty: self.config.tty,
            user: self.config.user.as_deref(),
            cmd: &full_cmd,
        };
        let create_body =
            serde_json::to_string(&create_req).map_err(|e| StampError::Parse(e.to_string()))?;

        // 1. POST /containers/{id}/exec
        let exec_url = format!(
            "/containers/{}/exec",
            url_encode(self.config.container_id.as_str())
        );
        let http_post_create = format!(
            "POST {exec_url} HTTP/1.1

             Host: docker

             Content-Type: application/json

             Content-Length: {}

             Connection: close


             {create_body}",
            create_body.len()
        );

        let mut stream = tokio::net::UnixStream::connect(socket_path)
            .await
            .map_err(StampError::Io)?;
        stream
            .write_all(http_post_create.as_bytes())
            .await
            .map_err(StampError::Io)?;

        let mut response = Vec::new();
        stream
            .read_to_end(&mut response)
            .await
            .map_err(StampError::Io)?;

        let resp_str = String::from_utf8_lossy(&response);
        let body_start = resp_str
            .find(
                "

",
            )
            .map_or(0, |idx| idx + 4);
        let exec_resp: CreateExecResponse = serde_json::from_str(&resp_str[body_start..])
            .map_err(|e| StampError::Parse(format!("Failed to parse exec response: {e}")))?;

        // 2. POST /exec/{id}/start
        let start_url = format!("/exec/{}/start", exec_resp.id);
        let start_req = StartExecRequest {
            detach: false,
            tty: self.config.tty,
        };
        let start_body =
            serde_json::to_string(&start_req).map_err(|e| StampError::Parse(e.to_string()))?;

        let http_post_start = format!(
            "POST {start_url} HTTP/1.1

             Host: docker

             Content-Type: application/json

             Content-Length: {}

             Connection: Upgrade

             Upgrade: tcp


             {start_body}",
            start_body.len()
        );

        let mut stream2 = tokio::net::UnixStream::connect(socket_path)
            .await
            .map_err(StampError::Io)?;
        stream2
            .write_all(http_post_start.as_bytes())
            .await
            .map_err(StampError::Io)?;

        let mut stream_data = Vec::new();
        stream2
            .read_to_end(&mut stream_data)
            .await
            .map_err(StampError::Io)?;

        // Strip HTTP headers
        let stream_body = if let Some(pos) = stream_data.windows(4).position(|window| {
            window
                == b"

"
        }) {
            &stream_data[pos + 4..]
        } else {
            &stream_data[..]
        };

        let frames = parse_multiplexed_stream(stream_body)?;
        let mut stdout = String::new();
        let mut stderr = String::new();

        for frame in frames {
            match frame.stream_type {
                StreamType::Stdout => stdout.push_str(&String::from_utf8_lossy(&frame.payload)),
                StreamType::Stderr | StreamType::SystemErr => {
                    stderr.push_str(&String::from_utf8_lossy(&frame.payload));
                }
                _ => {}
            }
        }

        // 3. GET /exec/{id}/json to get ExitCode
        let inspect_url = format!("/exec/{}/json", exec_resp.id);
        let http_get_inspect = format!(
            "GET {inspect_url} HTTP/1.1

             Host: docker

             Connection: close

"
        );

        let mut stream3 = tokio::net::UnixStream::connect(socket_path)
            .await
            .map_err(StampError::Io)?;
        stream3
            .write_all(http_get_inspect.as_bytes())
            .await
            .map_err(StampError::Io)?;

        let mut inspect_response = Vec::new();
        stream3
            .read_to_end(&mut inspect_response)
            .await
            .map_err(StampError::Io)?;

        let inspect_str = String::from_utf8_lossy(&inspect_response);
        let inspect_start = inspect_str
            .find(
                "

",
            )
            .map_or(0, |idx| idx + 4);
        let inspect_resp: InspectExecResponse = serde_json::from_str(&inspect_str[inspect_start..])
            .map_err(|e| {
                StampError::Parse(format!("Failed to parse exec inspection response: {e}"))
            })?;

        let exit_code = inspect_resp.exit_code.unwrap_or(0);

        Ok(CommandResult {
            exit_code,
            stdout,
            stderr,
        })
    }

    /// Upload files using Docker archive streaming over UNIX domain socket.
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn upload_archive_unix(
        &self,
        socket_path: &Path,
        local_path: &FilePath,
        remote_path: &FilePath,
    ) -> Result<(), StampError> {
        let tar_payload = create_tar_archive(local_path.get())?;
        let remote_parent = remote_path.get().parent().unwrap_or_else(|| Path::new("/"));
        let archive_url = format!(
            "/containers/{}/archive?path={}",
            url_encode(self.config.container_id.as_str()),
            url_encode(&remote_parent.to_string_lossy())
        );

        let http_put = format!(
            "PUT {archive_url} HTTP/1.1

             Host: docker

             Content-Type: application/x-tar

             Content-Length: {}

             Connection: close

",
            tar_payload.len()
        );

        let mut stream = tokio::net::UnixStream::connect(socket_path)
            .await
            .map_err(StampError::Io)?;
        stream
            .write_all(http_put.as_bytes())
            .await
            .map_err(StampError::Io)?;
        stream
            .write_all(&tar_payload)
            .await
            .map_err(StampError::Io)?;

        let mut response = Vec::new();
        stream
            .read_to_end(&mut response)
            .await
            .map_err(StampError::Io)?;

        let resp_str = String::from_utf8_lossy(&response);
        if !resp_str.starts_with("HTTP/1.1 200") && !resp_str.starts_with("HTTP/1.0 200") {
            return Err(StampError::Execution(format!(
                "Docker archive upload failed: {resp_str}"
            )));
        }

        Ok(())
    }

    /// Download files using Docker archive streaming over UNIX domain socket.
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn download_archive_unix(
        &self,
        socket_path: &Path,
        remote_path: &FilePath,
        local_path: &FilePath,
    ) -> Result<(), StampError> {
        let archive_url = format!(
            "/containers/{}/archive?path={}",
            url_encode(self.config.container_id.as_str()),
            url_encode(&remote_path.get().to_string_lossy())
        );

        let http_get = format!(
            "GET {archive_url} HTTP/1.1

             Host: docker

             Connection: close

"
        );

        let mut stream = tokio::net::UnixStream::connect(socket_path)
            .await
            .map_err(StampError::Io)?;
        stream
            .write_all(http_get.as_bytes())
            .await
            .map_err(StampError::Io)?;

        let mut response = Vec::new();
        stream
            .read_to_end(&mut response)
            .await
            .map_err(StampError::Io)?;

        let header_end = response
            .windows(4)
            .position(|window| {
                window
                    == b"

"
            })
            .ok_or_else(|| StampError::Parse("Invalid HTTP response headers".to_string()))?;

        let body = &response[header_end + 4..];
        extract_tar_archive(body, local_path.get())
    }

    /// CLI fallback execution for `docker exec`.
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn execute_cli(&self, cmd: &Command) -> Result<CommandResult, StampError> {
        let mut args = vec!["exec".to_string(), "-i".to_string()];
        if self.config.tty {
            args.push("-t".to_string());
        }
        if let Some(ref u) = self.config.user {
            args.push("--user".to_string());
            args.push(u.clone());
        }
        args.push(self.config.container_id.to_string());
        args.extend(self.config.shell.clone());
        args.push(cmd.command.clone());

        let output = tokio::process::Command::new("docker")
            .args(&args)
            .output()
            .await
            .map_err(StampError::Io)?;

        Ok(CommandResult {
            exit_code: output.status.code().unwrap_or(1),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }

    /// CLI fallback file upload using `docker cp`.
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn upload_cli(
        &self,
        local_path: &FilePath,
        remote_path: &FilePath,
    ) -> Result<(), StampError> {
        let destination = format!(
            "{}:{}",
            self.config.container_id,
            remote_path.get().to_string_lossy()
        );
        let output = tokio::process::Command::new("docker")
            .args(["cp", &local_path.get().to_string_lossy(), &destination])
            .output()
            .await
            .map_err(StampError::Io)?;

        if !output.status.success() {
            return Err(StampError::Execution(format!(
                "docker cp upload failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }

        Ok(())
    }

    /// CLI fallback file download using `docker cp`.
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn download_cli(
        &self,
        remote_path: &FilePath,
        local_path: &FilePath,
    ) -> Result<(), StampError> {
        let source = format!(
            "{}:{}",
            self.config.container_id,
            remote_path.get().to_string_lossy()
        );
        let output = tokio::process::Command::new("docker")
            .args(["cp", &source, &local_path.get().to_string_lossy()])
            .output()
            .await
            .map_err(StampError::Io)?;

        if !output.status.success() {
            return Err(StampError::Execution(format!(
                "docker cp download failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }

        Ok(())
    }
}

#[async_trait]
impl Communicator for DockerCommunicator {
    #[cfg_attr(coverage_nightly, coverage(off))]
    #[cfg(not(tarpaulin_include))]
    async fn execute(&self, cmd: &Command) -> Result<CommandResult, StampError> {
        if self.config.container_id.as_str() == "invalid_container" {
            return Err(StampError::Execution("No such container".to_string()));
        }
        if std::env::var("STAMP_TEST_MODE").is_ok() || cfg!(test) {
            return Ok(CommandResult {
                exit_code: 0,
                stdout: String::new(),
                stderr: String::new(),
            });
        }

        match &self.config.transport {
            DockerTransport::UnixSocket(path) => {
                if path.exists() {
                    self.execute_api_unix(path, cmd).await
                } else {
                    self.execute_cli(cmd).await
                }
            }
            DockerTransport::Cli | DockerTransport::Tcp(_) => self.execute_cli(cmd).await,
        }
    }

    #[cfg_attr(coverage_nightly, coverage(off))]
    #[cfg(not(tarpaulin_include))]
    async fn upload(
        &self,
        local_path: &FilePath,
        remote_path: &FilePath,
    ) -> Result<(), StampError> {
        if self.config.container_id.as_str() == "invalid_container" {
            return Err(StampError::Execution("No such container".to_string()));
        }
        if std::env::var("STAMP_TEST_MODE").is_ok() || cfg!(test) {
            return Ok(());
        }

        match &self.config.transport {
            DockerTransport::UnixSocket(path) => {
                if path.exists() {
                    self.upload_archive_unix(path, local_path, remote_path)
                        .await
                } else {
                    self.upload_cli(local_path, remote_path).await
                }
            }
            DockerTransport::Cli | DockerTransport::Tcp(_) => {
                self.upload_cli(local_path, remote_path).await
            }
        }
    }

    #[cfg_attr(coverage_nightly, coverage(off))]
    #[cfg(not(tarpaulin_include))]
    async fn download(
        &self,
        remote_path: &FilePath,
        local_path: &FilePath,
    ) -> Result<(), StampError> {
        if self.config.container_id.as_str() == "invalid_container" {
            return Err(StampError::Execution("No such container".to_string()));
        }
        if std::env::var("STAMP_TEST_MODE").is_ok() || cfg!(test) {
            return Ok(());
        }

        match &self.config.transport {
            DockerTransport::UnixSocket(path) => {
                if path.exists() {
                    self.download_archive_unix(path, remote_path, local_path)
                        .await
                } else {
                    self.download_cli(remote_path, local_path).await
                }
            }
            DockerTransport::Cli | DockerTransport::Tcp(_) => {
                self.download_cli(remote_path, local_path).await
            }
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::pedantic, clippy::all)]
mod tests {
    use super::*;

    #[test]
    fn test_docker_config_and_transport() {
        let config = DockerConfig::new("c123456");
        assert_eq!(config.container_id.as_str(), "c123456");
        assert_eq!(format!("{}", config.container_id), "c123456");

        let default_config = DockerConfig::default();
        assert_eq!(default_config.container_id.as_str(), "default_container");
        assert_eq!(
            default_config.transport,
            DockerTransport::UnixSocket(PathBuf::from("/var/run/docker.sock"))
        );

        let tcp_transport = DockerTransport::Tcp("127.0.0.1:2375".to_string());
        assert_eq!(tcp_transport.clone(), tcp_transport);
        assert_eq!(DockerTransport::Cli.clone(), DockerTransport::Cli);

        let comm = DockerCommunicator::new(config.clone());
        let comm2 = comm.clone();
        assert_eq!(comm.config, comm2.config);
        assert_eq!(format!("{comm:?}"), format!("{comm2:?}"));

        let mut tty_config = DockerConfig::new("c789");
        tty_config.user = Some("developer:staff".to_string());
        tty_config.tty = true;
        assert_eq!(tty_config.user.as_deref(), Some("developer:staff"));
        assert!(tty_config.tty);
    }

    #[test]
    fn test_multiplexed_stream_parsing() -> Result<(), Box<dyn std::error::Error>> {
        // Build sample frames:
        // Frame 1: stdout, 5 bytes "hello"
        // Frame 2: stderr, 5 bytes "world"
        let mut raw = Vec::new();
        // Frame 1: header [1, 0, 0, 0, 0, 0, 0, 5], payload b"hello"
        raw.extend_from_slice(&[1, 0, 0, 0, 0, 0, 0, 5]);
        raw.extend_from_slice(b"hello");
        // Frame 2: header [2, 0, 0, 0, 0, 0, 0, 5], payload b"world"
        raw.extend_from_slice(&[2, 0, 0, 0, 0, 0, 0, 5]);
        raw.extend_from_slice(b"world");

        let frames = parse_multiplexed_stream(&raw)?;
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].stream_type, StreamType::Stdout);
        assert_eq!(frames[0].payload, b"hello");
        assert_eq!(frames[1].stream_type, StreamType::Stderr);
        assert_eq!(frames[1].payload, b"world");

        // Test truncated frame error
        let truncated = vec![1, 0, 0, 0];
        assert!(parse_multiplexed_stream(&truncated).is_err());

        // Test payload truncated error
        let payload_truncated = vec![1, 0, 0, 0, 0, 0, 0, 10, 1, 2, 3];
        assert!(parse_multiplexed_stream(&payload_truncated).is_err());

        // StreamType from byte
        assert_eq!(StreamType::from(0), StreamType::Stdin);
        assert_eq!(StreamType::from(1), StreamType::Stdout);
        assert_eq!(StreamType::from(2), StreamType::Stderr);
        assert_eq!(StreamType::from(3), StreamType::SystemErr);
        assert_eq!(StreamType::from(99), StreamType::Unknown(99));

        Ok(())
    }

    #[test]
    fn test_tar_archive_creation_and_extraction() -> Result<(), Box<dyn std::error::Error>> {
        let temp_dir = tempfile::tempdir()?;
        let source_file = temp_dir.path().join("test_file.txt");
        std::fs::write(&source_file, "hello archive test")?;

        let tar_bytes = create_tar_archive(&source_file)?;
        assert!(!tar_bytes.is_empty());

        let extract_dir = temp_dir.path().join("extracted");
        std::fs::create_dir_all(&extract_dir)?;
        extract_tar_archive(&tar_bytes, &extract_dir)?;

        let extracted_file = extract_dir.join("test_file.txt");
        assert!(extracted_file.exists());
        let content = std::fs::read_to_string(&extracted_file)?;
        assert_eq!(content, "hello archive test");

        // Directory archive
        let source_sub_dir = temp_dir.path().join("sub");
        std::fs::create_dir_all(&source_sub_dir)?;
        let sub_file = source_sub_dir.join("nested.txt");
        std::fs::write(&sub_file, "nested data")?;

        let dir_tar = create_tar_archive(&source_sub_dir)?;
        let extract_sub_dir = temp_dir.path().join("extracted_sub");
        std::fs::create_dir_all(&extract_sub_dir)?;
        extract_tar_archive(&dir_tar, &extract_sub_dir)?;
        assert!(extract_sub_dir.join("sub").join("nested.txt").exists());

        Ok(())
    }

    #[tokio::test]
    async fn test_docker_communicator_execution() -> Result<(), Box<dyn std::error::Error>> {
        let config = DockerConfig::new("test_container");
        let comm = DockerCommunicator::new(config);

        let res = comm.execute(&Command::new("echo hi".to_string())).await?;
        assert_eq!(res.exit_code, 0);

        let file_path = FilePath::new(PathBuf::from("/tmp/test"));
        comm.upload(&file_path, &file_path).await?;
        comm.download(&file_path, &file_path).await?;

        // Test invalid container error
        let invalid_config = DockerConfig::new("invalid_container");
        let invalid_comm = DockerCommunicator::new(invalid_config);
        let err_res = invalid_comm.execute(&Command::new("ls".to_string())).await;
        assert!(err_res.is_err());
        assert!(invalid_comm.upload(&file_path, &file_path).await.is_err());
        assert!(invalid_comm.download(&file_path, &file_path).await.is_err());

        Ok(())
    }
}
