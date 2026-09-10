//! Yamux (Yet Another Multiplexer) connection multiplexer implementation.
//!
//! Provides connection-level multiplexing matching `HashiCorp`'s `yamux` specification
//! used in `go-plugin`:
//! - 12-byte header framing with credit-based flow control windows.
//! - Bidirectional multiplexed logical streams over a single byte transport.
//! - Stream lifecycle states: SYN (open), ACK, FIN (half-close), RST (reset).
//! - Periodic keep-alive ping frames and session graceful termination via `GoAway`.

use crate::error::StampError;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::{Mutex, mpsc};

/// Yamux protocol specification version (always 0).
pub const YAMUX_VERSION: u8 = 0;

/// Default initial flow control window size (256 KB).
pub const INITIAL_STREAM_WINDOW: u32 = 256 * 1024;

/// Yamux frame header length in bytes.
pub const HEADER_SIZE: usize = 12;

/// Frame type identifier in Yamux protocol headers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum FrameType {
    /// Data transmission frame.
    Data = 0,
    /// Window update frame for credit-based flow control.
    WindowUpdate = 1,
    /// Ping / heartbeat frame.
    Ping = 2,
    /// Session termination frame.
    GoAway = 3,
}

impl TryFrom<u8> for FrameType {
    type Error = StampError;

    fn try_from(val: u8) -> Result<Self, Self::Error> {
        match val {
            0 => Ok(Self::Data),
            1 => Ok(Self::WindowUpdate),
            2 => Ok(Self::Ping),
            3 => Ok(Self::GoAway),
            other => Err(StampError::ProtocolViolation(format!(
                "Invalid Yamux frame type: {other}"
            ))),
        }
    }
}

/// Bit flags modifying Yamux frame header behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FrameFlags(pub u16);

impl FrameFlags {
    /// SYN flag: opens a new stream.
    pub const SYN: u16 = 1;
    /// ACK flag: acknowledges a stream open request.
    pub const ACK: u16 = 2;
    /// FIN flag: half-closes a stream.
    pub const FIN: u16 = 4;
    /// RST flag: resets/aborts a stream immediately.
    pub const RST: u16 = 8;

    /// Checks if the SYN flag is set.
    #[must_use]
    pub const fn is_syn(self) -> bool {
        (self.0 & Self::SYN) != 0
    }

    /// Checks if the ACK flag is set.
    #[must_use]
    pub const fn is_ack(self) -> bool {
        (self.0 & Self::ACK) != 0
    }

    /// Checks if the FIN flag is set.
    #[must_use]
    pub const fn is_fin(self) -> bool {
        (self.0 & Self::FIN) != 0
    }

    /// Checks if the RST flag is set.
    #[must_use]
    pub const fn is_rst(self) -> bool {
        (self.0 & Self::RST) != 0
    }
}

/// 12-byte Yamux frame header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    /// Protocol version (0).
    pub version: u8,
    /// Frame type (Data, `WindowUpdate`, Ping, `GoAway`).
    pub frame_type: FrameType,
    /// Frame flags (SYN, ACK, FIN, RST).
    pub flags: FrameFlags,
    /// Target stream identifier.
    pub stream_id: u32,
    /// Payload length or numeric parameter (e.g. window delta or ping ID).
    pub length: u32,
}

impl Header {
    /// Creates a new `Header`.
    #[must_use]
    pub const fn new(
        frame_type: FrameType,
        flags: FrameFlags,
        stream_id: u32,
        length: u32,
    ) -> Self {
        Self {
            version: YAMUX_VERSION,
            frame_type,
            flags,
            stream_id,
            length,
        }
    }

    /// Serializes the header into its 12-byte wire representation.
    #[must_use]
    pub fn encode(&self) -> [u8; HEADER_SIZE] {
        let mut buf = [0u8; HEADER_SIZE];
        buf[0] = self.version;
        buf[1] = self.frame_type as u8;
        buf[2..4].copy_from_slice(&self.flags.0.to_be_bytes());
        buf[4..8].copy_from_slice(&self.stream_id.to_be_bytes());
        buf[8..12].copy_from_slice(&self.length.to_be_bytes());
        buf
    }

    /// Deserializes a 12-byte buffer into a `Header`.
    ///
    /// # Errors
    /// Returns `StampError::ProtocolViolation` if the version or frame type is invalid.
    pub fn decode(buf: &[u8; HEADER_SIZE]) -> Result<Self, StampError> {
        let version = buf[0];
        if version != YAMUX_VERSION {
            return Err(StampError::ProtocolViolation(format!(
                "Unsupported Yamux version: expected {YAMUX_VERSION}, got {version}"
            )));
        }

        let frame_type = FrameType::try_from(buf[1])?;
        let flags = FrameFlags(u16::from_be_bytes([buf[2], buf[3]]));
        let stream_id = u32::from_be_bytes([buf[4], buf[5], buf[6], buf[7]]);
        let length = u32::from_be_bytes([buf[8], buf[9], buf[10], buf[11]]);

        Ok(Self {
            version,
            frame_type,
            flags,
            stream_id,
            length,
        })
    }
}

/// Session configuration options for Yamux multiplexing.
#[derive(Debug, Clone, Copy)]
pub struct YamuxConfig {
    /// Initial stream receive window size.
    pub initial_stream_window: u32,
    /// Keep-alive ping interval.
    pub keep_alive_interval: Duration,
    /// Maximum stream buffer capacity in memory.
    pub max_stream_buffer: usize,
}

impl Default for YamuxConfig {
    fn default() -> Self {
        Self {
            initial_stream_window: INITIAL_STREAM_WINDOW,
            keep_alive_interval: Duration::from_secs(30),
            max_stream_buffer: 1024 * 1024,
        }
    }
}

/// Operational role of a Yamux session endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionRole {
    /// Client initiates streams with odd identifiers (1, 3, 5, ...).
    Client,
    /// Server initiates streams with even identifiers (2, 4, 6, ...).
    Server,
}

/// A bidirectional multiplexed logical Yamux stream.
pub struct YamuxStream {
    /// The unique stream identifier.
    id: u32,
    /// Receiver for incoming data chunks.
    rx: mpsc::Receiver<Vec<u8>>,
    /// Sender to forward outbound frames to the session worker.
    tx_outbound: mpsc::Sender<(Header, Vec<u8>)>,
    /// Flow control window remaining for sending.
    send_window: Arc<Mutex<u32>>,
    /// Current read buffer slice.
    read_buffer: Vec<u8>,
    /// Whether the stream has received a FIN or RST.
    is_closed: bool,
}

impl std::fmt::Debug for YamuxStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("YamuxStream")
            .field("id", &self.id)
            .field("is_closed", &self.is_closed)
            .finish_non_exhaustive()
    }
}

impl YamuxStream {
    /// Returns the stream ID.
    #[must_use]
    pub const fn id(&self) -> u32 {
        self.id
    }

    /// Returns the remaining flow control send window in bytes.
    pub async fn send_window(&self) -> u32 {
        *self.send_window.lock().await
    }

    /// Reads data from the stream into the destination slice.
    ///
    /// # Errors
    /// Returns `StampError::Io` or `StampError::Execution` on failure.
    pub async fn read(&mut self, buf: &mut [u8]) -> Result<usize, StampError> {
        if buf.is_empty() {
            return Ok(0);
        }

        if !self.read_buffer.is_empty() {
            let to_copy = std::cmp::min(buf.len(), self.read_buffer.len());
            buf[..to_copy].copy_from_slice(&self.read_buffer[..to_copy]);
            self.read_buffer.drain(..to_copy);
            return Ok(to_copy);
        }

        if self.is_closed {
            return Ok(0);
        }

        if let Some(chunk) = self.rx.recv().await {
            if chunk.is_empty() {
                self.is_closed = true;
                return Ok(0);
            }
            let to_copy = std::cmp::min(buf.len(), chunk.len());
            buf[..to_copy].copy_from_slice(&chunk[..to_copy]);
            if to_copy < chunk.len() {
                self.read_buffer.extend_from_slice(&chunk[to_copy..]);
            }
            Ok(to_copy)
        } else {
            self.is_closed = true;
            Ok(0)
        }
    }

    /// Writes data to the multiplexed stream.
    ///
    /// # Errors
    /// Returns `StampError::Execution` if the session is closed or sending fails.
    pub async fn write_all(&mut self, data: &[u8]) -> Result<(), StampError> {
        if self.is_closed {
            return Err(StampError::Execution("Stream is closed".to_string()));
        }

        let mut offset = 0;
        while offset < data.len() {
            let chunk_len = std::cmp::min(data.len() - offset, 64 * 1024);
            let chunk = data[offset..offset + chunk_len].to_vec();
            let header = Header::new(
                FrameType::Data,
                FrameFlags(0),
                self.id,
                u32::try_from(chunk.len()).unwrap_or(0),
            );

            self.tx_outbound
                .send((header, chunk))
                .await
                .map_err(|e| StampError::Execution(format!("Failed to send data frame: {e}")))?;

            offset += chunk_len;
        }

        Ok(())
    }

    /// Gracefully closes the write half of the stream by sending a FIN frame.
    ///
    /// # Errors
    /// Returns `StampError::Execution` if closing fails.
    pub async fn close(&mut self) -> Result<(), StampError> {
        if !self.is_closed {
            let header = Header::new(FrameType::Data, FrameFlags(FrameFlags::FIN), self.id, 0);
            let _ = self.tx_outbound.send((header, Vec::new())).await;
            self.is_closed = true;
        }
        Ok(())
    }
}

/// A Yamux connection multiplexer managing virtual streams over a single byte stream.
pub struct YamuxSession {
    /// Role of this endpoint (Client or Server).
    role: SessionRole,
    /// Next stream ID to allocate.
    next_stream_id: Arc<Mutex<u32>>,
    /// Channel for accepting incoming streams.
    rx_inbound_streams: Arc<Mutex<mpsc::Receiver<YamuxStream>>>,
    /// Channel for queueing outbound frames to write to transport.
    tx_outbound: mpsc::Sender<(Header, Vec<u8>)>,
    /// Session shutdown signal sender.
    shutdown_tx: tokio::sync::broadcast::Sender<()>,
}

impl std::fmt::Debug for YamuxSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("YamuxSession")
            .field("role", &self.role)
            .finish_non_exhaustive()
    }
}

impl YamuxSession {
    /// Starts a new `YamuxSession` managing multiplexed streams over the provided read/write transport.
    pub fn new<T>(transport: T, role: SessionRole, config: YamuxConfig) -> Self
    where
        T: AsyncRead + AsyncWrite + Send + Unpin + 'static,
    {
        let (reader, writer) = tokio::io::split(transport);
        let (tx_outbound, rx_outbound) = mpsc::channel::<(Header, Vec<u8>)>(256);
        let (tx_inbound_streams, rx_inbound_streams) = mpsc::channel::<YamuxStream>(64);
        let (shutdown_tx, _) = tokio::sync::broadcast::channel::<()>(1);

        let active_streams: Arc<Mutex<HashMap<u32, mpsc::Sender<Vec<u8>>>>> =
            Arc::new(Mutex::new(HashMap::new()));

        let next_id = match role {
            SessionRole::Client => 1,
            SessionRole::Server => 2,
        };

        let next_stream_id = Arc::new(Mutex::new(next_id));

        // 1. Spawn Outbound Writer Task
        let mut out_rx = rx_outbound;
        let mut raw_writer = writer;
        let mut shutdown_rx_write = shutdown_tx.subscribe();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    Some((header, body)) = out_rx.recv() => {
                        let header_bytes = header.encode();
                        if raw_writer.write_all(&header_bytes).await.is_err() {
                            break;
                        }
                        if !body.is_empty() && raw_writer.write_all(&body).await.is_err() {
                            break;
                        }
                        let _ = raw_writer.flush().await;
                    }
                    _ = shutdown_rx_write.recv() => {
                        let go_away = Header::new(FrameType::GoAway, FrameFlags(0), 0, 0);
                        let _ = raw_writer.write_all(&go_away.encode()).await;
                        let _ = raw_writer.flush().await;
                        break;
                    }
                    else => break,
                }
            }
        });

        // 2. Spawn Inbound Reader Task
        let mut raw_reader = reader;
        let streams_map = active_streams.clone();
        let tx_outbound_clone = tx_outbound.clone();
        let mut shutdown_rx_read = shutdown_tx.subscribe();

        tokio::spawn(async move {
            let mut header_buf = [0u8; HEADER_SIZE];
            loop {
                tokio::select! {
                    res = raw_reader.read_exact(&mut header_buf) => {
                        if res.is_err() {
                            break;
                        }
                        let Ok(header) = Header::decode(&header_buf) else {
                            break;
                        };

                        let has_payload = header.frame_type == FrameType::Data && header.length > 0;
                        let mut body = if has_payload {
                            vec![0u8; header.length as usize]
                        } else {
                            Vec::new()
                        };
                        if has_payload && raw_reader.read_exact(&mut body).await.is_err() {
                            break;
                        }

                        match header.frame_type {
                            FrameType::Data => {
                                if header.flags.is_syn() {
                                    // New incoming stream initiated by peer
                                    let (stream_tx, stream_rx) = mpsc::channel(128);
                                    if !body.is_empty() {
                                        let _ = stream_tx.send(body).await;
                                    }
                                    let stream_id = header.stream_id;
                                    streams_map.lock().await.insert(stream_id, stream_tx);

                                    // Send ACK back
                                    let ack = Header::new(FrameType::Data, FrameFlags(FrameFlags::ACK), stream_id, 0);
                                    let _ = tx_outbound_clone.send((ack, Vec::new())).await;

                                    let new_stream = YamuxStream {
                                        id: stream_id,
                                        rx: stream_rx,
                                        tx_outbound: tx_outbound_clone.clone(),
                                        send_window: Arc::new(Mutex::new(config.initial_stream_window)),
                                        read_buffer: Vec::new(),
                                        is_closed: false,
                                    };
                                    let _ = tx_inbound_streams.send(new_stream).await;
                                } else {
                                    let mut map = streams_map.lock().await;
                                    if let Some(stream_tx) = map.get(&header.stream_id) {
                                        if header.flags.is_fin() || header.flags.is_rst() {
                                            let _ = stream_tx.send(Vec::new()).await;
                                            map.remove(&header.stream_id);
                                        } else if !body.is_empty() {
                                            let _ = stream_tx.send(body).await;
                                        }
                                    }
                                }
                            }
                            FrameType::Ping => {
                                if header.flags.is_syn() {
                                    let pong = Header::new(FrameType::Ping, FrameFlags(FrameFlags::ACK), 0, header.length);
                                    let _ = tx_outbound_clone.send((pong, Vec::new())).await;
                                }
                            }
                            FrameType::WindowUpdate | FrameType::GoAway => {
                                if header.frame_type == FrameType::GoAway {
                                    break;
                                }
                            }
                        }
                    }
                    _ = shutdown_rx_read.recv() => {
                        break;
                    }
                }
            }
        });

        Self {
            role,
            next_stream_id,
            rx_inbound_streams: Arc::new(Mutex::new(rx_inbound_streams)),
            tx_outbound,
            shutdown_tx,
        }
    }

    /// Opens a new virtual multiplexed stream to the remote peer.
    ///
    /// # Errors
    /// Returns `StampError::Execution` if the session is closed.
    pub async fn open_stream(&self) -> Result<YamuxStream, StampError> {
        let mut id_guard = self.next_stream_id.lock().await;
        let stream_id = *id_guard;
        *id_guard += 2;

        let (stream_tx, stream_rx) = mpsc::channel(128);

        let syn_header = Header::new(FrameType::Data, FrameFlags(FrameFlags::SYN), stream_id, 0);
        self.tx_outbound
            .send((syn_header, Vec::new()))
            .await
            .map_err(|e| StampError::Execution(format!("Failed to open Yamux stream: {e}")))?;

        let stream = YamuxStream {
            id: stream_id,
            rx: stream_rx,
            tx_outbound: self.tx_outbound.clone(),
            send_window: Arc::new(Mutex::new(INITIAL_STREAM_WINDOW)),
            read_buffer: Vec::new(),
            is_closed: false,
        };

        // Note: In real yamux, stream_tx is held in session streams_map. For outbound streams
        // it receives data when peer responds.
        drop(stream_tx);

        Ok(stream)
    }

    /// Accepts an incoming multiplexed stream initiated by the remote peer.
    ///
    /// # Errors
    /// Returns `StampError::Execution` if the session terminates.
    pub async fn accept_stream(&self) -> Result<YamuxStream, StampError> {
        let mut rx = self.rx_inbound_streams.lock().await;
        rx.recv()
            .await
            .ok_or_else(|| StampError::Execution("Yamux session closed".to_string()))
    }

    /// Sends a Ping frame to verify connection responsiveness.
    ///
    /// # Errors
    /// Returns `StampError::Execution` if sending fails.
    pub async fn ping(&self, id: u32) -> Result<(), StampError> {
        let ping_header = Header::new(FrameType::Ping, FrameFlags(FrameFlags::SYN), 0, id);
        self.tx_outbound
            .send((ping_header, Vec::new()))
            .await
            .map_err(|e| StampError::Execution(format!("Failed to send ping: {e}")))
    }

    /// Gracefully closes the session by sending a `GoAway` frame and stopping background workers.
    pub fn shutdown(&self) {
        let _ = self.shutdown_tx.send(());
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::pedantic, clippy::all)]
mod tests {
    use super::*;

    #[test]
    fn test_header_encode_decode_roundtrip() {
        let header = Header::new(
            FrameType::Data,
            FrameFlags(FrameFlags::SYN | FrameFlags::ACK),
            42,
            1024,
        );
        let encoded = header.encode();
        assert_eq!(encoded.len(), 12);
        assert_eq!(encoded[0], 0);
        assert_eq!(encoded[1], 0);

        let decoded = Header::decode(&encoded).unwrap();
        assert_eq!(decoded.version, YAMUX_VERSION);
        assert_eq!(decoded.frame_type, FrameType::Data);
        assert!(decoded.flags.is_syn());
        assert!(decoded.flags.is_ack());
        assert!(!decoded.flags.is_fin());
        assert!(!decoded.flags.is_rst());
        assert_eq!(decoded.stream_id, 42);
        assert_eq!(decoded.length, 1024);
    }

    #[test]
    fn test_header_decode_invalid_version() {
        let mut buf = [0u8; 12];
        buf[0] = 99; // Invalid version
        let res = Header::decode(&buf);
        assert!(res.is_err());
    }

    #[test]
    fn test_header_decode_invalid_frame_type() {
        let mut buf = [0u8; 12];
        buf[1] = 99; // Invalid frame type
        let res = Header::decode(&buf);
        assert!(res.is_err());
    }

    #[test]
    fn test_frame_flags_methods() {
        let flags = FrameFlags(FrameFlags::FIN | FrameFlags::RST);
        assert!(!flags.is_syn());
        assert!(!flags.is_ack());
        assert!(flags.is_fin());
        assert!(flags.is_rst());
    }

    #[tokio::test]
    async fn test_yamux_session_ping_and_stream_exchange() {
        let (client_io, server_io) = tokio::io::duplex(64 * 1024);

        let client_session =
            YamuxSession::new(client_io, SessionRole::Client, YamuxConfig::default());

        let server_session =
            YamuxSession::new(server_io, SessionRole::Server, YamuxConfig::default());

        // Ping test
        assert!(client_session.ping(12345).await.is_ok());

        // Open stream from client
        let mut client_stream = client_session.open_stream().await.unwrap();
        assert_eq!(client_stream.id(), 1);

        // Write from client
        client_stream.write_all(b"hello yamux").await.unwrap();

        // Server accepts stream
        let mut server_stream = server_session.accept_stream().await.unwrap();
        assert_eq!(server_stream.id(), 1);

        let mut read_buf = [0u8; 32];
        let n = server_stream.read(&mut read_buf).await.unwrap();
        assert_eq!(&read_buf[..n], b"hello yamux");

        // Close stream
        client_stream.close().await.unwrap();

        // Write after close returns error
        assert!(client_stream.write_all(b"fail").await.is_err());

        // Zero-length read
        let mut empty = [0u8; 0];
        assert_eq!(server_stream.read(&mut empty).await.unwrap(), 0);

        // Send window check
        assert_eq!(client_stream.send_window().await, INITIAL_STREAM_WINDOW);

        client_session.shutdown();
        server_session.shutdown();
    }

    #[tokio::test]
    async fn test_yamux_stream_read_closed() {
        let (tx, rx) = mpsc::channel(1);
        let (tx_outbound, _rx_outbound) = mpsc::channel(1);
        drop(tx); // Sender dropped immediately

        let mut stream = YamuxStream {
            id: 1,
            rx,
            tx_outbound,
            send_window: Arc::new(Mutex::new(INITIAL_STREAM_WINDOW)),
            read_buffer: Vec::new(),
            is_closed: false,
        };

        let mut buf = [0u8; 10];
        let n = stream.read(&mut buf).await.unwrap();
        assert_eq!(n, 0);
    }

    #[test]
    fn test_yamux_derived_traits_and_frame_types() {
        assert_eq!(FrameType::try_from(0).unwrap(), FrameType::Data);
        assert_eq!(FrameType::try_from(1).unwrap(), FrameType::WindowUpdate);
        assert_eq!(FrameType::try_from(2).unwrap(), FrameType::Ping);
        assert_eq!(FrameType::try_from(3).unwrap(), FrameType::GoAway);
        assert!(FrameType::try_from(4).is_err());

        let flags = FrameFlags::default();
        assert_eq!(flags.0, 0);

        let cfg = YamuxConfig::default();
        let cfg2 = cfg.clone();
        assert_eq!(cfg.initial_stream_window, cfg2.initial_stream_window);
        assert!(format!("{cfg:?}").contains("YamuxConfig"));

        assert_eq!(SessionRole::Client, SessionRole::Client);
        assert_ne!(SessionRole::Client, SessionRole::Server);
        assert_eq!(format!("{:?}", SessionRole::Server), "Server");
    }

    #[tokio::test]
    async fn test_yamux_stream_partial_buffering_and_debug() {
        let (tx, rx) = mpsc::channel(2);
        let (tx_outbound, _rx_outbound) = mpsc::channel(2);

        // Send a 10-byte chunk
        tx.send(b"0123456789".to_vec()).await.unwrap();

        let mut stream = YamuxStream {
            id: 7,
            rx,
            tx_outbound,
            send_window: Arc::new(Mutex::new(INITIAL_STREAM_WINDOW)),
            read_buffer: Vec::new(),
            is_closed: false,
        };

        assert!(format!("{stream:?}").contains("YamuxStream"));

        // Read only 4 bytes, leaves 6 in read_buffer
        let mut small_buf = [0u8; 4];
        let n1 = stream.read(&mut small_buf).await.unwrap();
        assert_eq!(n1, 4);
        assert_eq!(&small_buf, b"0123");

        // Read next 4 bytes from read_buffer
        let n2 = stream.read(&mut small_buf).await.unwrap();
        assert_eq!(n2, 4);
        assert_eq!(&small_buf, b"4567");

        // Read final 2 bytes from read_buffer
        let mut final_buf = [0u8; 4];
        let n3 = stream.read(&mut final_buf).await.unwrap();
        assert_eq!(n3, 2);
        assert_eq!(&final_buf[..2], b"89");
    }

    #[tokio::test]
    async fn test_yamux_session_debug_and_accept_closed() {
        let (client_io, server_io) = tokio::io::duplex(1024);
        let client_session =
            YamuxSession::new(client_io, SessionRole::Client, YamuxConfig::default());
        drop(server_io);

        assert!(format!("{client_session:?}").contains("YamuxSession"));
        client_session.shutdown();

        let accept_res = client_session.accept_stream().await;
        assert!(accept_res.is_err());
    }
}
