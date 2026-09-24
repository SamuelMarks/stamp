#![cfg_attr(coverage_nightly, coverage(off))]
//! Remote UI gRPC service and client adapter.
//!
//! Provides bidirectional streaming and remote invocation of `Ui` events
//! (say, message, error, ask, `stream_events`) between Stamp and out-of-process plugins.

use crate::engine::ui::Ui;
use crate::error::StampError;
use crate::r#gen::packer::ui_client::UiClient;
use crate::r#gen::packer::ui_server::Ui as UiService;
use crate::r#gen::packer::{
    AskRequest, AskResponse, ErrorRequest, MessageRequest, SayRequest, UiEvent, UiResponse,
};
use async_trait::async_trait;
use futures_util::StreamExt;
use std::sync::Arc;
use tonic::{Request, Response, Status};

/// Server-side adapter exposing a local `Ui` instance over gRPC.
#[derive(Debug, Clone)]
pub struct RemoteUiServer {
    /// Local UI instance.
    ui: Arc<Ui>,
}

impl RemoteUiServer {
    /// Creates a new `RemoteUiServer` wrapping the given `Ui` instance.
    #[must_use]
    pub fn new(ui: Arc<Ui>) -> Self {
        Self { ui }
    }
}

#[async_trait]
impl UiService for RemoteUiServer {
    async fn say(&self, request: Request<SayRequest>) -> Result<Response<UiResponse>, Status> {
        let req = request.into_inner();
        self.ui.say(&req.target, &req.message);
        Ok(Response::new(UiResponse {}))
    }

    async fn message(
        &self,
        request: Request<MessageRequest>,
    ) -> Result<Response<UiResponse>, Status> {
        let req = request.into_inner();
        self.ui.say(&req.target, &req.message);
        Ok(Response::new(UiResponse {}))
    }

    async fn error(&self, request: Request<ErrorRequest>) -> Result<Response<UiResponse>, Status> {
        let req = request.into_inner();
        self.ui.error(&req.target, &req.error);
        Ok(Response::new(UiResponse {}))
    }

    async fn ask(&self, request: Request<AskRequest>) -> Result<Response<AskResponse>, Status> {
        let req = request.into_inner();
        let response = self.ui.ask("", &req.query).unwrap_or_default();
        Ok(Response::new(AskResponse { response }))
    }

    type StreamEventsStream =
        std::pin::Pin<Box<dyn futures_util::Stream<Item = Result<UiResponse, Status>> + Send>>;

    #[allow(clippy::result_large_err)]
    async fn stream_events(
        &self,
        request: Request<tonic::Streaming<UiEvent>>,
    ) -> Result<Response<Self::StreamEventsStream>, Status> {
        let in_stream = request.into_inner();
        let ui = self.ui.clone();

        let mapped = in_stream.filter_map(move |event_res| {
            let ui = ui.clone();
            futures_util::future::ready(event_res.ok().map(|event| {
                if event.event_type == "error" {
                    ui.error(&event.target, &event.message);
                } else {
                    ui.say(&event.target, &event.message);
                }
                Ok(UiResponse {})
            }))
        });

        Ok(Response::new(Box::pin(mapped)))
    }
}

/// Client-side adapter for dispatching UI events to a remote `Ui` service over gRPC.
#[derive(Debug, Clone)]
pub struct RemoteUiClient {
    /// Endpoint address.
    pub endpoint: String,
    /// Inner tonic client.
    client: UiClient<tonic::transport::Channel>,
}

impl RemoteUiClient {
    /// Creates a new `RemoteUiClient` connected to the given channel.
    #[must_use]
    pub fn new(endpoint: String, channel: tonic::transport::Channel) -> Self {
        Self {
            endpoint,
            client: UiClient::new(channel),
        }
    }

    /// Connects to a remote UI gRPC server over TCP.
    ///
    /// # Errors
    /// Returns `StampError::Execution` if the connection fails.
    pub async fn connect_tcp(address: &str) -> Result<Self, StampError> {
        let endpoint_url = if address.starts_with("http") {
            address.to_string()
        } else {
            format!("http://{address}")
        };
        let client = UiClient::connect(endpoint_url.clone())
            .await
            .map_err(|e| StampError::Execution(format!("Remote UI connect error: {e}")))?;
        Ok(Self {
            endpoint: endpoint_url,
            client,
        })
    }

    /// Sends a `Say` message over gRPC.
    ///
    /// # Errors
    /// Returns `StampError::Execution` if the RPC fails.
    pub async fn say(&mut self, target: &str, message: &str) -> Result<(), StampError> {
        self.client
            .say(Request::new(SayRequest {
                target: target.to_string(),
                message: message.to_string(),
            }))
            .await
            .map_err(|e| StampError::Execution(format!("Remote UI say error: {e}")))?;
        Ok(())
    }

    /// Sends a `Message` over gRPC.
    ///
    /// # Errors
    /// Returns `StampError::Execution` if the RPC fails.
    pub async fn message(&mut self, target: &str, message: &str) -> Result<(), StampError> {
        self.client
            .message(Request::new(MessageRequest {
                target: target.to_string(),
                message: message.to_string(),
            }))
            .await
            .map_err(|e| StampError::Execution(format!("Remote UI message error: {e}")))?;
        Ok(())
    }

    /// Sends an `Error` message over gRPC.
    ///
    /// # Errors
    /// Returns `StampError::Execution` if the RPC fails.
    pub async fn error(&mut self, target: &str, error: &str) -> Result<(), StampError> {
        self.client
            .error(Request::new(ErrorRequest {
                target: target.to_string(),
                error: error.to_string(),
            }))
            .await
            .map_err(|e| StampError::Execution(format!("Remote UI error error: {e}")))?;
        Ok(())
    }

    /// Sends an `Ask` prompt over gRPC and returns the response.
    ///
    /// # Errors
    /// Returns `StampError::Execution` if the RPC fails.
    pub async fn ask(&mut self, query: &str) -> Result<String, StampError> {
        let resp = self
            .client
            .ask(Request::new(AskRequest {
                query: query.to_string(),
            }))
            .await
            .map_err(|e| StampError::Execution(format!("Remote UI ask error: {e}")))?;
        Ok(resp.into_inner().response)
    }

    /// Streams UI events to a remote server.
    ///
    /// # Errors
    /// Returns `StampError::Execution` if initiating the streaming RPC fails.
    pub async fn stream_events<S>(
        &mut self,
        stream: S,
    ) -> Result<tonic::Streaming<UiResponse>, StampError>
    where
        S: futures_util::Stream<Item = UiEvent> + Send + 'static,
    {
        let resp = self
            .client
            .stream_events(Request::new(stream))
            .await
            .map_err(|e| StampError::Execution(format!("Remote UI stream_events error: {e}")))?;
        Ok(resp.into_inner())
    }
}

#[cfg(test)]
#[allow(clippy::all, clippy::pedantic)]
mod tests {
    use super::*;
    use crate::engine::packer::FeatureState;
    use crate::r#gen::packer::ui_server::UiServer;
    use std::collections::VecDeque;
    use std::sync::Mutex;
    use tokio::sync::oneshot;

    #[test]
    fn test_derived_traits() {
        let ui = Arc::new(Ui::new(
            FeatureState::Disabled,
            FeatureState::Disabled,
            FeatureState::Disabled,
        ));
        let server = RemoteUiServer::new(ui);
        assert!(format!("{server:?}").contains("RemoteUiServer"));
        let cloned_server = server.clone();
        assert!(format!("{cloned_server:?}").contains("RemoteUiServer"));
    }

    #[tokio::test]
    async fn test_remote_ui_say_message_error_ask_lifecycle() -> Result<(), StampError> {
        let mut mock_queue = VecDeque::new();
        mock_queue.push_back("user_input_answer".to_string());
        let mock_inputs = Arc::new(Mutex::new(mock_queue));

        let ui = Arc::new(Ui::with_scrubber_and_mock_inputs(
            FeatureState::Disabled,
            FeatureState::Disabled,
            FeatureState::Disabled,
            None,
            Some(mock_inputs),
        ));
        let server = RemoteUiServer::new(ui);

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(StampError::Io)?;
        let local_addr = listener.local_addr().map_err(StampError::Io)?;

        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
        let server_handle = tokio::spawn(async move {
            let _ = tonic::transport::Server::builder()
                .add_service(UiServer::new(server))
                .serve_with_incoming_shutdown(
                    tokio_stream::wrappers::TcpListenerStream::new(listener),
                    async {
                        let _ = shutdown_rx.await;
                    },
                )
                .await;
        });

        // Test connect with plain address (no http prefix)
        let mut client = RemoteUiClient::connect_tcp(&local_addr.to_string()).await?;
        assert!(format!("{client:?}").contains("RemoteUiClient"));
        let cloned_client = client.clone();
        assert_eq!(cloned_client.endpoint, client.endpoint);

        // Test say
        client.say("builder", "compiling artifact").await?;

        // Test message
        client.message("builder", "status update message").await?;

        // Test error
        client.error("builder", "something failed").await?;

        // Test ask
        let answer = client.ask("Please enter confirmation").await?;
        assert_eq!(answer, "user_input_answer");

        // Test connect with explicit http prefix
        let mut client_http = RemoteUiClient::connect_tcp(&format!("http://{local_addr}")).await?;
        client_http.say("builder", "via http url").await?;

        let _ = shutdown_tx.send(());
        let _ = server_handle.await;
        Ok(())
    }

    #[tokio::test]
    async fn test_remote_ui_stream_events_say_and_error() -> Result<(), StampError> {
        let ui = Arc::new(Ui::new(
            FeatureState::Disabled,
            FeatureState::Disabled,
            FeatureState::Disabled,
        ));
        let server = RemoteUiServer::new(ui);

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(StampError::Io)?;
        let local_addr = listener.local_addr().map_err(StampError::Io)?;

        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
        let server_handle = tokio::spawn(async move {
            let _ = tonic::transport::Server::builder()
                .add_service(UiServer::new(server))
                .serve_with_incoming_shutdown(
                    tokio_stream::wrappers::TcpListenerStream::new(listener),
                    async {
                        let _ = shutdown_rx.await;
                    },
                )
                .await;
        });

        let mut client = RemoteUiClient::connect_tcp(&local_addr.to_string()).await?;

        let events = vec![
            UiEvent {
                event_type: "say".to_string(),
                target: "target_say".to_string(),
                message: "say message".to_string(),
            },
            UiEvent {
                event_type: "error".to_string(),
                target: "target_err".to_string(),
                message: "error message".to_string(),
            },
        ];

        let in_stream = tokio_stream::iter(events);
        let mut response_stream = client.stream_events(in_stream).await?;

        let r1 = response_stream.next().await;
        assert!(matches!(r1, Some(Ok(_))));
        let r2 = response_stream.next().await;
        assert!(matches!(r2, Some(Ok(_))));
        let r3 = response_stream.next().await;
        assert!(r3.is_none());

        let _ = shutdown_tx.send(());
        let _ = server_handle.await;
        Ok(())
    }

    #[tokio::test]
    async fn test_remote_ui_ask_empty_fallback() -> Result<(), StampError> {
        // UI without mock inputs returns empty string in test mode
        let ui = Arc::new(Ui::new(
            FeatureState::Disabled,
            FeatureState::Disabled,
            FeatureState::Disabled,
        ));
        let server = RemoteUiServer::new(ui);

        let req = Request::new(AskRequest {
            query: "What is your name?".to_string(),
        });

        let res = server.ask(req).await;
        assert_eq!(
            res.ok().map(|r| r.into_inner().response),
            Some(String::new())
        );
        Ok(())
    }

    #[tokio::test]
    async fn test_remote_ui_client_new_constructor() -> Result<(), StampError> {
        let channel =
            tonic::transport::Channel::from_static("http://127.0.0.1:9999").connect_lazy();
        let client = RemoteUiClient::new("http://127.0.0.1:9999".to_string(), channel);
        assert_eq!(client.endpoint, "http://127.0.0.1:9999");
        Ok(())
    }

    #[tokio::test]
    async fn test_remote_ui_connect_tcp_invalid() {
        let res = RemoteUiClient::connect_tcp("invalid-address-format-:::").await;
        assert!(matches!(res, Err(StampError::Execution(_))));
    }

    #[tokio::test]
    async fn test_remote_ui_client_rpc_failures_when_disconnected() -> Result<(), StampError> {
        let channel =
            tonic::transport::Channel::from_static("http://127.0.0.1:9999").connect_lazy();
        let mut client = RemoteUiClient::new("http://127.0.0.1:9999".to_string(), channel);

        assert!(matches!(
            client.say("target", "msg").await,
            Err(StampError::Execution(_))
        ));
        assert!(matches!(
            client.message("target", "msg").await,
            Err(StampError::Execution(_))
        ));
        assert!(matches!(
            client.error("target", "err").await,
            Err(StampError::Execution(_))
        ));
        assert!(matches!(
            client.ask("query").await,
            Err(StampError::Execution(_))
        ));
        let in_stream = tokio_stream::iter(Vec::<UiEvent>::new());
        assert!(matches!(
            client.stream_events(in_stream).await,
            Err(StampError::Execution(_))
        ));
        Ok(())
    }
}
