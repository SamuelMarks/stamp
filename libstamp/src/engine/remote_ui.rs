#![cfg_attr(coverage_nightly, coverage(off))]
//! Remote UI gRPC service and client adapter.
//!
//! Provides bidirectional streaming and remote invocation of `Ui` events
//! (say, message, error, ask, stream_events) between Stamp and out-of-process plugins.

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
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status};

/// Server-side adapter exposing a local `Ui` instance over gRPC.
#[derive(Clone)]
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
        let response = self
            .ui
            .ask("", &req.query)
            .map_err(|e| Status::internal(e.to_string()))?;
        Ok(Response::new(AskResponse { response }))
    }

    type StreamEventsStream = ReceiverStream<Result<UiResponse, Status>>;

    async fn stream_events(
        &self,
        request: Request<tonic::Streaming<UiEvent>>,
    ) -> Result<Response<Self::StreamEventsStream>, Status> {
        let mut in_stream = request.into_inner();
        let (tx, rx) = mpsc::channel(16);
        let ui = self.ui.clone();

        tokio::spawn(async move {
            while let Some(event_res) = in_stream.next().await {
                match event_res {
                    Ok(event) => {
                        if event.event_type == "error" {
                            ui.error(&event.target, &event.message);
                        } else {
                            ui.say(&event.target, &event.message);
                        }
                        let _ = tx.send(Ok(UiResponse {})).await;
                    }
                    Err(err) => {
                        let _ = tx.send(Err(Status::internal(err.to_string()))).await;
                        break;
                    }
                }
            }
        });

        Ok(Response::new(ReceiverStream::new(rx)))
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
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::pedantic, clippy::all)]
mod tests {
    use super::*;
    use crate::engine::packer::FeatureState;
    use crate::r#gen::packer::ui_server::UiServer;

    #[tokio::test]
    async fn test_remote_ui_say_error_stream() {
        let ui = Arc::new(Ui::new(
            FeatureState::Disabled,
            FeatureState::Disabled,
            FeatureState::Disabled,
        ));
        let server = RemoteUiServer::new(ui);

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let local_addr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            tonic::transport::Server::builder()
                .add_service(UiServer::new(server))
                .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener))
                .await
                .unwrap();
        });

        let mut client = RemoteUiClient::connect_tcp(&local_addr.to_string())
            .await
            .unwrap();

        client.say("builder", "compiling artifact").await.unwrap();
        client.error("builder", "something failed").await.unwrap();
    }
}
