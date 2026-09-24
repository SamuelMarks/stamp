#![cfg_attr(coverage_nightly, coverage(off))]
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
    /// Shutdown channel trigger.
    pub(crate) shutdown: Option<tokio::sync::oneshot::Sender<()>>,
}

impl HttpServerHandle {
    /// Stop the background HTTP server task.
    pub fn abort(&self) {
        self.task.abort();
    }

    /// Gracefully shut down the HTTP server.
    pub async fn shutdown(mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
        let _ = self.task.await;
    }
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
        if let Ok(l) = TcpListener::bind(format!("{bind_ip}:0")).await
            && let Ok(local_addr) = l.local_addr()
        {
            listener = Some((l, local_addr.port()));
        }
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
    let (shutdown_tx, mut shutdown_rx) = tokio::sync::oneshot::channel::<()>();

    let task = tokio::spawn(async move {
        loop {
            let (mut socket, peer_addr) = tokio::select! {
                _ = &mut shutdown_rx => break,
                Ok(conn) = listener.accept() => conn,
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
        shutdown: Some(shutdown_tx),
    })
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::all, clippy::pedantic, for_loops_over_fallibles)]
mod tests {
    use super::*;
    use tokio::io::AsyncWriteExt;

    #[tokio::test]
    async fn test_start_http_server_success() {
        let dir = std::env::temp_dir().join("stamp_test_http_success");
        let _ = std::fs::create_dir_all(&dir);
        let file_path = dir.join("preseed.cfg");
        let _ = std::fs::write(&file_path, b"d-i debian-installer/locale string en_US");

        let ui = Arc::new(crate::engine::ui::Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));

        let res = start_http_server_with_ui(
            dir.clone(),
            (29100, 29150),
            Some(ui.clone()),
            Some("test-http".to_string()),
        )
        .await;
        assert!(res.is_ok());

        for handle in res {
            assert!(handle.port >= 29100);

            // 1. GET with leading slash and query string
            let url = format!("http://127.0.0.1:{}/preseed.cfg?param=1", handle.port);
            let resp = reqwest::get(&url).await;
            assert!(resp.as_ref().map(|r| r.status().is_success()).ok() == Some(true));
            for r in resp {
                let body = r.text().await;
                assert!(
                    body.as_ref()
                        .map(|b| b.contains("d-i debian-installer"))
                        .ok()
                        == Some(true)
                );
            }

            // 2. 404 test with UI logging
            let bad_url = format!("http://127.0.0.1:{}/missing.cfg", handle.port);
            let resp_bad = reqwest::get(&bad_url).await;
            assert!(resp_bad.as_ref().map(|r| r.status().as_u16()).ok() == Some(404));

            // 3. Raw TCP: non-GET request (POST) -> 404
            let stream = tokio::net::TcpStream::connect(format!("127.0.0.1:{}", handle.port)).await;
            assert!(stream.is_ok());
            for mut s in stream {
                let _ = s
                    .write_all(b"POST /preseed.cfg HTTP/1.1\r\nHost: localhost\r\n\r\n")
                    .await;
                let mut response_buf = [0u8; 512];
                let n = s.read(&mut response_buf).await.unwrap_or_default();
                let resp_str = String::from_utf8_lossy(&response_buf[..n]);
                assert!(resp_str.contains("404 Not Found"));
            }

            // 4. Raw TCP: path without leading slash
            let stream2 =
                tokio::net::TcpStream::connect(format!("127.0.0.1:{}", handle.port)).await;
            assert!(stream2.is_ok());
            for mut s2 in stream2 {
                let _ = s2
                    .write_all(b"GET preseed.cfg HTTP/1.1\r\nHost: localhost\r\n\r\n")
                    .await;
                let mut response_buf2 = [0u8; 512];
                let n2 = s2.read(&mut response_buf2).await.unwrap_or_default();
                let resp_str2 = String::from_utf8_lossy(&response_buf2[..n2]);
                assert!(resp_str2.contains("200 OK"));
            }

            // 5. Raw TCP: empty request (immediate EOF / zero bytes read)
            let stream3 =
                tokio::net::TcpStream::connect(format!("127.0.0.1:{}", handle.port)).await;
            drop(stream3);

            // 6. Path traversal attempt (e.g. "../")
            let traversal_url = format!("http://127.0.0.1:{}/../test.txt", handle.port);
            let resp_traversal = reqwest::get(&traversal_url).await;
            assert!(resp_traversal.as_ref().map(|r| r.status().as_u16()).ok() == Some(404));

            // 7. Request a directory rather than a file -> 404
            let sub_dir = dir.join("subdir");
            let _ = std::fs::create_dir_all(&sub_dir);
            let dir_url = format!("http://127.0.0.1:{}/subdir", handle.port);
            let resp_dir = reqwest::get(&dir_url).await;
            assert!(resp_dir.as_ref().map(|r| r.status().as_u16()).ok() == Some(404));

            handle.shutdown().await;
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_mime_types() {
        assert_eq!(
            mime_type_for_path(Path::new("test.html")),
            "text/html; charset=utf-8"
        );
        assert_eq!(
            mime_type_for_path(Path::new("test.htm")),
            "text/html; charset=utf-8"
        );
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
            mime_type_for_path(Path::new("test.yml")),
            "text/yaml; charset=utf-8"
        );
        assert_eq!(
            mime_type_for_path(Path::new("test.cfg")),
            "text/plain; charset=utf-8"
        );
        assert_eq!(
            mime_type_for_path(Path::new("test.ks")),
            "text/plain; charset=utf-8"
        );
        assert_eq!(
            mime_type_for_path(Path::new("test.txt")),
            "text/plain; charset=utf-8"
        );
        assert_eq!(mime_type_for_path(Path::new("test.sh")), "application/x-sh");
        assert_eq!(
            mime_type_for_path(Path::new("test.iso")),
            "application/x-iso9660-image"
        );
        assert_eq!(
            mime_type_for_path(Path::new("test.bin")),
            "application/octet-stream"
        );
        assert_eq!(
            mime_type_for_path(Path::new("no_extension")),
            "application/octet-stream"
        );
    }

    #[tokio::test]
    async fn test_port_range_fallback() {
        let dir = std::env::temp_dir().join("stamp_test_http_fallback");
        let _ = std::fs::create_dir_all(&dir);
        let res = start_http_server(dir.clone(), (29200, 29250)).await;
        assert!(res.is_ok());
        for handle in res {
            assert!(handle.port >= 29200 && handle.port <= 29250);
            handle.abort();
            let _ = handle.task.await;
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_start_http_server_full_bind_address_and_no_ui() {
        let dir = std::env::temp_dir().join("stamp_test_http_no_ui");
        let _ = std::fs::create_dir_all(&dir);
        let file_path = dir.join("data.txt");
        let _ = std::fs::write(&file_path, b"hello");

        let res = start_http_server_full(dir.clone(), Some("127.0.0.1"), (0, 0), None, None).await;
        assert!(res.is_ok());

        for handle in res {
            assert_eq!(handle.ip, "127.0.0.1");
            assert!(handle.port > 0);

            // Fetch without UI logger to cover None branches
            let url = format!("http://127.0.0.1:{}/data.txt", handle.port);
            let resp = reqwest::get(&url).await;
            assert!(resp.as_ref().map(|r| r.status().as_u16()).ok() == Some(200));

            let bad_url = format!("http://127.0.0.1:{}/nonexistent.txt", handle.port);
            let resp_404 = reqwest::get(&bad_url).await;
            assert!(resp_404.as_ref().map(|r| r.status().as_u16()).ok() == Some(404));

            handle.shutdown().await;
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_start_http_server_nonexistent_directory() {
        let res =
            start_http_server(PathBuf::from("/path/that/does/not/exist/998877"), (0, 0)).await;
        assert!(res.is_err());
    }

    #[tokio::test]
    async fn test_start_http_server_port_bind_failure() {
        let dir = std::env::temp_dir();
        let occupied = tokio::net::TcpListener::bind("127.0.0.1:0").await;
        assert!(occupied.is_ok());
        for occ in occupied {
            let port = occ.local_addr().map(|a| a.port()).unwrap_or_default();
            let res =
                start_http_server_full(dir.clone(), Some("127.0.0.1"), (port, port), None, None)
                    .await;

            assert!(res.is_err());
        }
    }

    #[tokio::test]
    async fn test_start_http_server_invalid_bind_address() {
        let dir = std::env::temp_dir();
        let res = start_http_server_full(dir, Some("999.999.999.999"), (0, 0), None, None).await;
        assert!(res.is_err());
    }

    #[tokio::test]
    async fn test_shutdown_without_sender() {
        let handle = HttpServerHandle {
            ip: "127.0.0.1".to_string(),
            port: 8080,
            task: tokio::spawn(async {}),
            shutdown: None,
        };
        handle.shutdown().await;
    }
}
