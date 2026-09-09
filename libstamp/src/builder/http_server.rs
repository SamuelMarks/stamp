//! Built-in asynchronous HTTP micro-server for serving `http_directory` contents
//! (e.g. kickstart, preseed, autounattend, and cloud-init configurations).

use crate::error::StampError;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

/// HTTP server handle providing access to the bound port and background server task.
pub struct HttpServerHandle {
    /// The IP address the server bound to.
    pub ip: String,
    /// The actual port the server bound to.
    pub port: u16,
    /// The background task running the server.
    pub task: JoinHandle<()>,
}

/// Helper function to determine Content-Type header based on file extension.
#[must_use]
pub fn mime_type_for_path(path: &Path) -> &'static str {
    match path.extension().and_then(|ext| ext.to_str()) {
        Some("html" | "htm") => "text/html; charset=utf-8",
        Some("json") => "application/json",
        Some("xml") => "application/xml",
        Some("yaml" | "yml") => "text/yaml; charset=utf-8",
        Some("cfg" | "ks" | "txt") => "text/plain; charset=utf-8",
        Some("sh") => "application/x-sh",
        Some("iso") => "application/x-iso9660-image",
        _ => "application/octet-stream",
    }
}

/// Start an asynchronous HTTP micro-server serving files from a specific directory.
///
/// Supports dynamic port selection (e.g. `(0, 0)` for any available OS port),
/// port ranges with collision recovery, path traversal defense, and access logging.
///
/// # Errors
///
/// Returns `StampError::Execution` or `StampError::Io` if binding fails.
pub async fn start_http_server(
    dir: PathBuf,
    port_range: (u16, u16),
) -> Result<HttpServerHandle, StampError> {
    start_http_server_full(dir, None, port_range, None, None).await
}

/// Start an asynchronous HTTP micro-server with UI access logging.
///
/// # Errors
///
/// Returns `StampError::Execution` or `StampError::Io` if binding fails.
pub async fn start_http_server_with_ui(
    dir: PathBuf,
    port_range: (u16, u16),
    ui: Option<Arc<crate::engine::ui::Ui>>,
    name: Option<String>,
) -> Result<HttpServerHandle, StampError> {
    start_http_server_full(dir, None, port_range, ui, name).await
}

/// Start an asynchronous HTTP micro-server with interface IP binding, port ranges, and UI logging.
///
/// # Errors
///
/// Returns `StampError::Execution` or `StampError::Io` if binding fails.
pub async fn start_http_server_full(
    dir: PathBuf,
    bind_address: Option<&str>,
    port_range: (u16, u16),
    ui: Option<Arc<crate::engine::ui::Ui>>,
    name: Option<String>,
) -> Result<HttpServerHandle, StampError> {
    let canonical_dir = dir.canonicalize().map_err(StampError::Io)?;
    let bind_ip = bind_address.unwrap_or("0.0.0.0");

    let mut listener = None;
    if port_range.0 == 0 && port_range.1 == 0 {
        // Dynamic port selection by OS
        let l = TcpListener::bind(format!("{bind_ip}:0"))
            .await
            .map_err(StampError::Io)?;
        let local_addr = l.local_addr().map_err(StampError::Io)?;
        listener = Some((l, local_addr.port()));
    } else {
        // Port range with collision recovery
        for port in port_range.0..=port_range.1 {
            if let Ok(l) = TcpListener::bind(format!("{bind_ip}:{port}")).await {
                listener = Some((l, port));
                break;
            }
        }
    }

    let (listener, bound_port) = listener.ok_or_else(|| {
        StampError::Execution(format!(
            "Failed to bind to any port in range {}-{} on {bind_ip}",
            port_range.0, port_range.1
        ))
    })?;

    let effective_ip = if bind_ip == "0.0.0.0" {
        "127.0.0.1".to_string()
    } else {
        bind_ip.to_string()
    };

    if let (Some(u), Some(n)) = (&ui, &name) {
        u.say(
            n,
            &format!("HTTP directory server listening on {effective_ip}:{bound_port}"),
        );
    }

    let serve_dir = canonical_dir;
    let logger_ui = ui;
    let logger_name = name.unwrap_or_else(|| "http-server".to_string());

    let task = tokio::spawn(async move {
        loop {
            let (mut socket, peer_addr) = match listener.accept().await {
                Ok(conn) => conn,
                Err(_) => break,
            };

            let dir = serve_dir.clone();
            let ui_clone = logger_ui.clone();
            let name_clone = logger_name.clone();

            tokio::spawn(async move {
                let mut buf = [0u8; 4096];
                let n = match socket.read(&mut buf).await {
                    Ok(n) if n > 0 => n,
                    _ => return,
                };

                let request = String::from_utf8_lossy(&buf[..n]);
                let first_line = request.lines().next().unwrap_or_default();
                let parts: Vec<&str> = first_line.split_whitespace().collect();

                if parts.len() >= 2 && parts[0] == "GET" {
                    let mut req_path = parts[1].split('?').next().unwrap_or(parts[1]);
                    if req_path.starts_with('/') {
                        req_path = &req_path[1..];
                    }

                    // Security: prevent path traversal attacks (e.g. "../")
                    let candidate = dir.join(req_path);
                    if let Ok(canon_candidate) = candidate.canonicalize()
                        && canon_candidate.starts_with(&dir)
                        && canon_candidate.is_file()
                        && let Ok(content) = tokio::fs::read(&canon_candidate).await
                    {
                        let mime = mime_type_for_path(&canon_candidate);
                        let response_header = format!(
                            "HTTP/1.1 200 OK
Content-Type: {mime}
Content-Length: {}
Connection: close

",
                            content.len()
                        );
                        let _ = socket.write_all(response_header.as_bytes()).await;
                        let _ = socket.write_all(&content).await;

                        if let Some(ref u) = ui_clone {
                            u.say(
                                &name_clone,
                                &format!(
                                    "HTTP: {peer_addr} GET /{req_path} -> 200 OK ({} bytes)",
                                    content.len()
                                ),
                            );
                        }
                        return;
                    }
                }

                let not_found = "HTTP/1.1 404 Not Found
Content-Length: 9
Connection: close

Not Found";
                let _ = socket.write_all(not_found.as_bytes()).await;
                if let Some(ref u) = ui_clone {
                    u.say(&name_clone, &format!("HTTP: {peer_addr} 404 Not Found"));
                }
            });
        }
    });

    Ok(HttpServerHandle {
        ip: effective_ip,
        port: bound_port,
        task,
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::pedantic, clippy::all)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_start_http_server_success() -> Result<(), StampError> {
        let dir = tempdir().map_err(StampError::Io)?;
        let file_path = dir.path().join("preseed.cfg");
        let mut file = std::fs::File::create(&file_path).map_err(StampError::Io)?;
        file.write_all(b"d-i debian-installer/locale string en_US")
            .map_err(StampError::Io)?;

        let ui = Arc::new(crate::engine::ui::Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));

        let handle = start_http_server_with_ui(
            dir.path().to_path_buf(),
            (0, 0),
            Some(ui),
            Some("test-http".to_string()),
        )
        .await?;

        assert!(handle.port > 0);

        let url = format!("http://127.0.0.1:{}/preseed.cfg", handle.port);
        let resp = reqwest::get(&url)
            .await
            .map_err(|e| StampError::Execution(e.to_string()))?;
        assert!(resp.status().is_success());
        let body = resp
            .text()
            .await
            .map_err(|e| StampError::Execution(e.to_string()))?;
        assert!(body.contains("d-i debian-installer"));

        // 404 test
        let bad_url = format!("http://127.0.0.1:{}/missing.cfg", handle.port);
        let resp_bad = reqwest::get(&bad_url)
            .await
            .map_err(|e| StampError::Execution(e.to_string()))?;
        assert_eq!(resp_bad.status(), 404);

        handle.task.abort();
        Ok(())
    }

    #[test]
    fn test_mime_types() {
        assert_eq!(
            mime_type_for_path(Path::new("test.json")),
            "application/json"
        );
        assert_eq!(mime_type_for_path(Path::new("test.xml")), "application/xml");
        assert_eq!(
            mime_type_for_path(Path::new("test.yaml")),
            "text/yaml; charset=utf-8"
        );
        assert_eq!(
            mime_type_for_path(Path::new("test.ks")),
            "text/plain; charset=utf-8"
        );
        assert_eq!(
            mime_type_for_path(Path::new("test.bin")),
            "application/octet-stream"
        );
    }

    #[tokio::test]
    async fn test_port_range_fallback() -> Result<(), StampError> {
        let dir = tempdir().map_err(StampError::Io)?;
        let handle = start_http_server(dir.path().to_path_buf(), (25000, 25050)).await?;
        assert!(handle.port >= 25000 && handle.port <= 25050);
        handle.task.abort();
        Ok(())
    }

    #[tokio::test]
    async fn test_start_http_server_full_bind_address() -> Result<(), StampError> {
        let dir = tempdir().map_err(StampError::Io)?;
        let handle = start_http_server_full(
            dir.path().to_path_buf(),
            Some("127.0.0.1"),
            (0, 0),
            None,
            None,
        )
        .await?;
        assert_eq!(handle.ip, "127.0.0.1");
        assert!(handle.port > 0);
        handle.task.abort();
        Ok(())
    }
}
