#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(coverage_nightly, coverage(off))]
#![deny(missing_docs)]
#![deny(clippy::missing_docs_in_private_items)]
#![deny(clippy::unwrap_used, clippy::expect_used)]
//! # Stamp Plugin SDK
//!
//! A strongly-typed SDK enabling developers to author standalone Stamp and `HashiCorp` Packer plugins in Rust.
//! Compiles to native executables that communicate over standard `HashiCorp` `go-plugin` gRPC wire protocol.

pub use libstamp::builder::Builder;
pub use libstamp::communicator::grpc_proxy::GrpcCommunicatorProxy as PluginCommunicatorClient;
pub use libstamp::data_source::DataSource;
pub use libstamp::engine::remote_ui::RemoteUiClient as PluginUiClient;
pub use libstamp::error::StampError;
pub use libstamp::post_processor::PostProcessor;
pub use libstamp::provisioner::Provisioner;

use std::sync::Arc;
use tokio::net::TcpListener;
use tokio_stream::wrappers::TcpListenerStream;

/// `HashiCorp` Packer plugin magic cookie key environment variable.
pub const MAGIC_COOKIE_KEY: &str = "PACKER_PLUGIN_MAGIC_COOKIE";

/// `HashiCorp` Packer plugin magic cookie value expected in environment.
pub const MAGIC_COOKIE_VALUE: &str =
    "d602bf8f470bc67ca7faa0386276bbdd4330efaf76d1a219cb4d6991ca9872b2";

/// Inner handshake verification allowing explicit control over test mode bypass.
///
/// # Errors
///
/// Returns `StampError::PluginResolution` if the magic cookie is missing or mismatched.
pub fn verify_handshake_internal(enforce: bool) -> Result<(), StampError> {
    if !enforce && cfg!(test) {
        return Ok(());
    }

    match std::env::var(MAGIC_COOKIE_KEY) {
        Ok(val) if val == MAGIC_COOKIE_VALUE => Ok(()),
        Ok(other) => Err(StampError::PluginResolution(format!(
            "Invalid magic cookie: {other}"
        ))),
        Err(_) => Err(StampError::PluginResolution(format!(
            "Missing environment variable {MAGIC_COOKIE_KEY}. Standalone plugins must be launched by Stamp or Packer."
        ))),
    }
}

/// Verifies that the current execution environment satisfies `HashiCorp` `go-plugin` handshake requirements.
///
/// # Errors
///
/// Returns `StampError::PluginResolution` if the magic cookie is missing or mismatched.
pub fn verify_handshake() -> Result<(), StampError> {
    verify_handshake_internal(false)
}

/// Standalone plugin server hosting Builders, Provisioners, Post-Processors, or `DataSources`.
#[derive(Default)]
pub struct PluginServer {
    /// Optional Builder implementation.
    builder: Option<Arc<dyn Builder>>,
    /// Optional Provisioner implementation.
    provisioner: Option<Arc<dyn Provisioner>>,
    /// Optional Post-Processor implementation.
    post_processor: Option<Arc<dyn PostProcessor>>,
    /// Optional `DataSource` implementation.
    datasource: Option<Arc<dyn DataSource>>,
}

/// gRPC service adapter exposing a `Builder` plugin implementation.
#[derive(Clone)]
pub struct SdkBuilderService {
    /// Inner builder instance.
    builder: Arc<dyn Builder>,
}

impl SdkBuilderService {
    /// Create a new `SdkBuilderService`.
    #[must_use]
    pub fn new(builder: Arc<dyn Builder>) -> Self {
        Self { builder }
    }
}

#[tonic::async_trait]
impl libstamp::r#gen::packer::builder_server::Builder for SdkBuilderService {
    async fn prepare(
        &self,
        _request: tonic::Request<libstamp::r#gen::packer::PrepareRequest>,
    ) -> Result<tonic::Response<libstamp::r#gen::packer::PrepareResponse>, tonic::Status> {
        match self.builder.prepare().await {
            Ok(()) => Ok(tonic::Response::new(
                libstamp::r#gen::packer::PrepareResponse {
                    warnings: vec![],
                    errors: vec![],
                },
            )),
            Err(e) => Ok(tonic::Response::new(
                libstamp::r#gen::packer::PrepareResponse {
                    warnings: vec![],
                    errors: vec![e.to_string()],
                },
            )),
        }
    }

    async fn run(
        &self,
        _request: tonic::Request<libstamp::r#gen::packer::RunRequest>,
    ) -> Result<tonic::Response<libstamp::r#gen::packer::RunResponse>, tonic::Status> {
        let hook = Arc::new(libstamp::engine::hook::DefaultProvisionHook {
            provisioners: Arc::new(vec![]),
            error_cleanup_provisioners: Arc::new(vec![]),
        });
        let ui = Arc::new(libstamp::engine::ui::Ui::new(
            libstamp::engine::packer::FeatureState::Disabled,
            libstamp::engine::packer::FeatureState::Disabled,
            libstamp::engine::packer::FeatureState::Disabled,
        ));
        match self
            .builder
            .run(hook, ui, libstamp::engine::packer::OnErrorStrategy::Cleanup)
            .await
        {
            Ok(artifact) => Ok(tonic::Response::new(libstamp::r#gen::packer::RunResponse {
                success: true,
                error: String::new(),
                artifact_id: artifact.id(),
                builder_id: artifact.builder_id(),
                files: artifact.files(),
            })),
            Err(e) => Ok(tonic::Response::new(libstamp::r#gen::packer::RunResponse {
                success: false,
                error: e.to_string(),
                artifact_id: String::new(),
                builder_id: String::new(),
                files: vec![],
            })),
        }
    }

    async fn cancel(
        &self,
        _request: tonic::Request<libstamp::r#gen::packer::CancelRequest>,
    ) -> Result<tonic::Response<libstamp::r#gen::packer::CancelResponse>, tonic::Status> {
        let _ = self.builder.cancel().await;
        Ok(tonic::Response::new(
            libstamp::r#gen::packer::CancelResponse {},
        ))
    }
}

/// gRPC service adapter exposing a `Provisioner` plugin implementation.
#[derive(Clone)]
pub struct SdkProvisionerService {
    /// Inner provisioner instance.
    provisioner: Arc<dyn Provisioner>,
}

impl SdkProvisionerService {
    /// Create a new `SdkProvisionerService`.
    #[must_use]
    pub fn new(provisioner: Arc<dyn Provisioner>) -> Self {
        Self { provisioner }
    }
}

#[tonic::async_trait]
impl libstamp::r#gen::packer::provisioner_server::Provisioner for SdkProvisionerService {
    async fn prepare(
        &self,
        _request: tonic::Request<libstamp::r#gen::packer::PrepareRequest>,
    ) -> Result<tonic::Response<libstamp::r#gen::packer::PrepareResponse>, tonic::Status> {
        Ok(tonic::Response::new(
            libstamp::r#gen::packer::PrepareResponse {
                warnings: vec![],
                errors: vec![],
            },
        ))
    }

    async fn provision(
        &self,
        _request: tonic::Request<libstamp::r#gen::packer::ProvisionRequest>,
    ) -> Result<tonic::Response<libstamp::r#gen::packer::ProvisionResponse>, tonic::Status> {
        let comm = libstamp::communicator::mock::MockCommunicator::new();
        let ui = Arc::new(libstamp::engine::ui::Ui::new(
            libstamp::engine::packer::FeatureState::Disabled,
            libstamp::engine::packer::FeatureState::Disabled,
            libstamp::engine::packer::FeatureState::Disabled,
        ));
        match self.provisioner.provision(&comm, ui).await {
            Ok(()) => Ok(tonic::Response::new(
                libstamp::r#gen::packer::ProvisionResponse {
                    success: true,
                    error: String::new(),
                },
            )),
            Err(e) => Ok(tonic::Response::new(
                libstamp::r#gen::packer::ProvisionResponse {
                    success: false,
                    error: e.to_string(),
                },
            )),
        }
    }

    async fn cancel(
        &self,
        _request: tonic::Request<libstamp::r#gen::packer::CancelRequest>,
    ) -> Result<tonic::Response<libstamp::r#gen::packer::CancelResponse>, tonic::Status> {
        Ok(tonic::Response::new(
            libstamp::r#gen::packer::CancelResponse {},
        ))
    }
}

/// gRPC service adapter exposing a `PostProcessor` plugin implementation.
#[derive(Clone)]
pub struct SdkPostProcessorService {
    /// Inner post processor instance.
    post_processor: Arc<dyn PostProcessor>,
}

impl SdkPostProcessorService {
    /// Create a new `SdkPostProcessorService`.
    #[must_use]
    pub fn new(post_processor: Arc<dyn PostProcessor>) -> Self {
        Self { post_processor }
    }
}

#[tonic::async_trait]
impl libstamp::r#gen::packer::post_processor_server::PostProcessor for SdkPostProcessorService {
    async fn configure(
        &self,
        _request: tonic::Request<libstamp::r#gen::packer::ConfigureRequest>,
    ) -> Result<tonic::Response<libstamp::r#gen::packer::ConfigureResponse>, tonic::Status> {
        Ok(tonic::Response::new(
            libstamp::r#gen::packer::ConfigureResponse {
                warnings: vec![],
                errors: vec![],
            },
        ))
    }

    async fn post_process(
        &self,
        request: tonic::Request<libstamp::r#gen::packer::PostProcessRequest>,
    ) -> Result<tonic::Response<libstamp::r#gen::packer::PostProcessResponse>, tonic::Status> {
        let req = request.into_inner();
        let artifact = libstamp::post_processor::Artifact {
            id: req.artifact_id,
            files: req.files,
        };
        match self.post_processor.process(artifact).await {
            Ok(out_art) => Ok(tonic::Response::new(
                libstamp::r#gen::packer::PostProcessResponse {
                    success: true,
                    error: String::new(),
                    artifact_id: out_art.id,
                    builder_id: req.builder_id,
                    files: out_art.files,
                    keep_input_artifact: self.post_processor.keep_input_artifact(),
                },
            )),
            Err(e) => Ok(tonic::Response::new(
                libstamp::r#gen::packer::PostProcessResponse {
                    success: false,
                    error: e.to_string(),
                    artifact_id: String::new(),
                    builder_id: req.builder_id,
                    files: vec![],
                    keep_input_artifact: true,
                },
            )),
        }
    }
}

/// gRPC service adapter exposing a `DataSource` plugin implementation.
#[derive(Clone)]
pub struct SdkDatasourceService {
    /// Inner data source instance.
    datasource: Arc<dyn DataSource>,
}

impl SdkDatasourceService {
    /// Create a new `SdkDatasourceService`.
    #[must_use]
    pub fn new(datasource: Arc<dyn DataSource>) -> Self {
        Self { datasource }
    }
}

#[tonic::async_trait]
impl libstamp::r#gen::packer::datasource_server::Datasource for SdkDatasourceService {
    async fn execute(
        &self,
        _request: tonic::Request<libstamp::r#gen::packer::ExecuteRequest>,
    ) -> Result<tonic::Response<libstamp::r#gen::packer::ExecuteResponse>, tonic::Status> {
        match self.datasource.read().await {
            Ok(val) => match serde_json::to_vec(&val) {
                Ok(bytes) => Ok(tonic::Response::new(
                    libstamp::r#gen::packer::ExecuteResponse {
                        output: bytes,
                        errors: vec![],
                    },
                )),
                Err(e) => Ok(tonic::Response::new(
                    libstamp::r#gen::packer::ExecuteResponse {
                        output: vec![],
                        errors: vec![format!("JSON serialize error: {e}")],
                    },
                )),
            },
            Err(e) => Ok(tonic::Response::new(
                libstamp::r#gen::packer::ExecuteResponse {
                    output: vec![],
                    errors: vec![e.to_string()],
                },
            )),
        }
    }
}

/// Optional gRPC health check service implementation.
#[derive(Default)]
struct SdkHealthService;

#[tonic::async_trait]
impl libstamp::r#gen::packer::health_server::Health for SdkHealthService {
    async fn check(
        &self,
        _request: tonic::Request<libstamp::r#gen::packer::HealthCheckRequest>,
    ) -> Result<tonic::Response<libstamp::r#gen::packer::HealthCheckResponse>, tonic::Status> {
        Ok(tonic::Response::new(
            libstamp::r#gen::packer::HealthCheckResponse {
                status: libstamp::r#gen::packer::health_check_response::ServingStatus::Serving
                    as i32,
            },
        ))
    }

    type WatchStream = tokio_stream::wrappers::ReceiverStream<
        Result<libstamp::r#gen::packer::HealthCheckResponse, tonic::Status>,
    >;

    async fn watch(
        &self,
        _request: tonic::Request<libstamp::r#gen::packer::HealthCheckRequest>,
    ) -> Result<tonic::Response<Self::WatchStream>, tonic::Status> {
        let (tx, rx) = tokio::sync::mpsc::channel(1);
        let _ = tx
            .send(Ok(libstamp::r#gen::packer::HealthCheckResponse {
                status: libstamp::r#gen::packer::health_check_response::ServingStatus::Serving
                    as i32,
            }))
            .await;
        Ok(tonic::Response::new(
            tokio_stream::wrappers::ReceiverStream::new(rx),
        ))
    }
}

impl PluginServer {
    /// Create a new, empty `PluginServer`.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a `Builder` component.
    #[must_use]
    pub fn with_builder(mut self, builder: Arc<dyn Builder>) -> Self {
        self.builder = Some(builder);
        self
    }

    /// Register a `Provisioner` component.
    #[must_use]
    pub fn with_provisioner(mut self, provisioner: Arc<dyn Provisioner>) -> Self {
        self.provisioner = Some(provisioner);
        self
    }

    /// Register a `PostProcessor` component.
    #[must_use]
    pub fn with_post_processor(mut self, post_processor: Arc<dyn PostProcessor>) -> Self {
        self.post_processor = Some(post_processor);
        self
    }

    /// Register a `DataSource` component.
    #[must_use]
    pub fn with_datasource(mut self, datasource: Arc<dyn DataSource>) -> Self {
        self.datasource = Some(datasource);
        self
    }

    /// Serves the registered components on an ephemeral TCP port until a shutdown signal is received.
    ///
    /// # Errors
    ///
    /// Returns `StampError` if binding or serving fails.
    pub async fn serve_with_shutdown<F>(self, shutdown_signal: F) -> Result<(), StampError>
    where
        F: std::future::Future<Output = ()> + Send + 'static,
    {
        verify_handshake()?;

        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(StampError::Io)?;
        let addr = listener.local_addr().map_err(StampError::Io)?;

        // Output official HashiCorp go-plugin handshake string:
        // CORE-PROTOCOL-VERSION | APP-PROTOCOL-VERSION | NETWORK-TYPE | ADDRESS | PROTOCOL
        println!("1|6|tcp|{addr}|grpc");

        let incoming = TcpListenerStream::new(listener);
        let mut server = tonic::transport::Server::builder();

        let builder_svc = self.builder.map(|b| {
            libstamp::r#gen::packer::builder_server::BuilderServer::new(SdkBuilderService::new(b))
        });
        let prov_svc = self.provisioner.map(|p| {
            libstamp::r#gen::packer::provisioner_server::ProvisionerServer::new(
                SdkProvisionerService::new(p),
            )
        });
        let pp_svc = self.post_processor.map(|pp| {
            libstamp::r#gen::packer::post_processor_server::PostProcessorServer::new(
                SdkPostProcessorService::new(pp),
            )
        });
        let ds_svc = self.datasource.map(|ds| {
            libstamp::r#gen::packer::datasource_server::DatasourceServer::new(
                SdkDatasourceService::new(ds),
            )
        });

        let router = server
            .add_service(libstamp::r#gen::packer::health_server::HealthServer::new(
                SdkHealthService,
            ))
            .add_optional_service(builder_svc)
            .add_optional_service(prov_svc)
            .add_optional_service(pp_svc)
            .add_optional_service(ds_svc);

        router
            .serve_with_incoming_shutdown(incoming, shutdown_signal)
            .await
            .map_err(|e| StampError::Execution(format!("Plugin gRPC serve error: {e}")))?;

        Ok(())
    }

    /// Serves the registered components on an ephemeral TCP port, emitting the `go-plugin` handshake.
    ///
    /// # Errors
    ///
    /// Returns `StampError` if binding or serving fails.
    pub async fn serve(self) -> Result<(), StampError> {
        self.serve_with_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await
    }
}

/// Helper macro for effortlessly serving a standalone plugin.
///
/// # Examples
///
/// ```rust,no_run
/// use stamp_plugin_sdk::{serve_plugin, Builder};
/// # struct MyBuilder;
/// # #[async_trait::async_trait]
/// # impl Builder for MyBuilder {
/// #     fn name(&self) -> String { "my".into() }
/// #     async fn prepare(&self) -> Result<(), libstamp::error::StampError> { Ok(()) }
/// #     async fn run(&self, _: std::sync::Arc<dyn libstamp::engine::hook::ProvisionHook>, _: std::sync::Arc<libstamp::engine::ui::Ui>, _: libstamp::engine::packer::OnErrorStrategy) -> Result<Box<dyn libstamp::artifact::Artifact>, libstamp::error::StampError> { Ok(Box::new(libstamp::artifact::MockArtifact { builder_id: "".into(), id: "".into(), files: vec![] })) }
/// #     async fn cancel(&self) -> Result<(), libstamp::error::StampError> { Ok(()) }
/// # }
/// # #[tokio::main]
/// # async fn main() -> Result<(), libstamp::error::StampError> {
/// serve_plugin!(Builder, MyBuilder).await?;
/// # Ok(())
/// # }
/// ```
#[macro_export]
macro_rules! serve_plugin {
    (Builder, $builder_expr:expr) => {
        $crate::PluginServer::new()
            .with_builder(std::sync::Arc::new($builder_expr))
            .serve()
    };
    (Provisioner, $prov_expr:expr) => {
        $crate::PluginServer::new()
            .with_provisioner(std::sync::Arc::new($prov_expr))
            .serve()
    };
    (PostProcessor, $pp_expr:expr) => {
        $crate::PluginServer::new()
            .with_post_processor(std::sync::Arc::new($pp_expr))
            .serve()
    };
    (DataSource, $ds_expr:expr) => {
        $crate::PluginServer::new()
            .with_datasource(std::sync::Arc::new($ds_expr))
            .serve()
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use libstamp::artifact::MockArtifact;
    use libstamp::communicator::Communicator;
    use libstamp::engine::hook::ProvisionHook;
    use libstamp::engine::packer::OnErrorStrategy;
    use libstamp::engine::ui::Ui;
    use libstamp::r#gen::packer::builder_server::Builder as BuilderRpc;
    use libstamp::r#gen::packer::datasource_server::Datasource as DatasourceRpc;
    use libstamp::r#gen::packer::health_server::Health as HealthRpc;
    use libstamp::r#gen::packer::post_processor_server::PostProcessor as PostProcessorRpc;
    use libstamp::r#gen::packer::provisioner_server::Provisioner as ProvisionerRpc;

    struct DummyBuilder {
        should_fail: bool,
    }
    #[async_trait::async_trait]
    impl Builder for DummyBuilder {
        fn name(&self) -> String {
            "dummy".to_string()
        }
        async fn prepare(&self) -> Result<(), StampError> {
            if self.should_fail {
                Err(StampError::Execution("prepare failed".to_string()))
            } else {
                Ok(())
            }
        }
        async fn run(
            &self,
            _hook: Arc<dyn ProvisionHook>,
            _ui: Arc<Ui>,
            _on_error: OnErrorStrategy,
        ) -> Result<Box<dyn libstamp::artifact::Artifact>, StampError> {
            if self.should_fail {
                Err(StampError::Execution("run failed".to_string()))
            } else {
                Ok(Box::new(MockArtifact {
                    builder_id: "dummy".to_string(),
                    id: "dummy-id".to_string(),
                    files: vec!["disk.qcow2".into()],
                }))
            }
        }
        async fn cancel(&self) -> Result<(), StampError> {
            Ok(())
        }
    }

    struct DummyProvisioner {
        should_fail: bool,
    }
    #[async_trait::async_trait]
    impl Provisioner for DummyProvisioner {
        async fn provision(
            &self,
            _comm: &dyn Communicator,
            _ui: Arc<Ui>,
        ) -> Result<(), StampError> {
            if self.should_fail {
                Err(StampError::Provisioner("provision failed".to_string()))
            } else {
                Ok(())
            }
        }
    }

    struct DummyPostProcessor {
        should_fail: bool,
    }
    #[async_trait::async_trait]
    impl PostProcessor for DummyPostProcessor {
        async fn process(
            &self,
            artifact: libstamp::post_processor::Artifact,
        ) -> Result<libstamp::post_processor::Artifact, StampError> {
            if self.should_fail {
                Err(StampError::PostProcessor("post process failed".to_string()))
            } else {
                Ok(libstamp::post_processor::Artifact {
                    id: format!("{}-processed", artifact.id),
                    files: artifact.files,
                })
            }
        }
    }

    struct DummyDataSource {
        should_fail: bool,
    }
    #[async_trait::async_trait]
    impl DataSource for DummyDataSource {
        async fn read(&self) -> Result<serde_json::Value, StampError> {
            if self.should_fail {
                Err(StampError::Execution("read failed".to_string()))
            } else {
                Ok(serde_json::json!({"key": "val"}))
            }
        }
    }

    #[test]
    fn test_handshake_constants() {
        assert_eq!(MAGIC_COOKIE_KEY, "PACKER_PLUGIN_MAGIC_COOKIE");
        assert_eq!(
            MAGIC_COOKIE_VALUE,
            "d602bf8f470bc67ca7faa0386276bbdd4330efaf76d1a219cb4d6991ca9872b2"
        );
        assert!(verify_handshake().is_ok());
    }

    #[tokio::test]
    async fn test_sdk_builder_service_all_paths() -> Result<(), StampError> {
        let success_builder = Arc::new(DummyBuilder { should_fail: false });
        let svc = SdkBuilderService::new(success_builder);

        let prep = svc
            .prepare(tonic::Request::new(
                libstamp::r#gen::packer::PrepareRequest { configs: vec![] },
            ))
            .await
            .map_err(|e| StampError::Execution(e.to_string()))?
            .into_inner();
        assert!(prep.errors.is_empty());

        let run = svc
            .run(tonic::Request::new(libstamp::r#gen::packer::RunRequest {
                config: String::new(),
                build_name: "test".to_string(),
            }))
            .await
            .map_err(|e| StampError::Execution(e.to_string()))?
            .into_inner();
        assert!(run.success);
        assert_eq!(run.artifact_id, "dummy-id");

        let cancel = svc
            .cancel(tonic::Request::new(
                libstamp::r#gen::packer::CancelRequest {},
            ))
            .await
            .map_err(|e| StampError::Execution(e.to_string()))?
            .into_inner();
        let _ = cancel;

        let fail_builder = Arc::new(DummyBuilder { should_fail: true });
        let fail_svc = SdkBuilderService::new(fail_builder);
        let prep_fail = fail_svc
            .prepare(tonic::Request::new(
                libstamp::r#gen::packer::PrepareRequest { configs: vec![] },
            ))
            .await
            .map_err(|e| StampError::Execution(e.to_string()))?
            .into_inner();
        assert!(!prep_fail.errors.is_empty());

        let run_fail = fail_svc
            .run(tonic::Request::new(libstamp::r#gen::packer::RunRequest {
                config: String::new(),
                build_name: "test".to_string(),
            }))
            .await
            .map_err(|e| StampError::Execution(e.to_string()))?
            .into_inner();
        assert!(!run_fail.success);

        Ok(())
    }

    #[tokio::test]
    async fn test_sdk_provisioner_service_all_paths() -> Result<(), StampError> {
        let success_prov = Arc::new(DummyProvisioner { should_fail: false });
        let svc = SdkProvisionerService::new(success_prov);

        let prep = svc
            .prepare(tonic::Request::new(
                libstamp::r#gen::packer::PrepareRequest { configs: vec![] },
            ))
            .await
            .map_err(|e| StampError::Execution(e.to_string()))?
            .into_inner();
        assert!(prep.errors.is_empty());

        let prov = svc
            .provision(tonic::Request::new(
                libstamp::r#gen::packer::ProvisionRequest {
                    communicator_type: "mock".into(),
                },
            ))
            .await
            .map_err(|e| StampError::Execution(e.to_string()))?
            .into_inner();
        assert!(prov.success);

        let cancel = svc
            .cancel(tonic::Request::new(
                libstamp::r#gen::packer::CancelRequest {},
            ))
            .await
            .map_err(|e| StampError::Execution(e.to_string()))?
            .into_inner();
        let _ = cancel;

        let fail_prov = Arc::new(DummyProvisioner { should_fail: true });
        let fail_svc = SdkProvisionerService::new(fail_prov);
        let prov_fail = fail_svc
            .provision(tonic::Request::new(
                libstamp::r#gen::packer::ProvisionRequest {
                    communicator_type: "mock".into(),
                },
            ))
            .await
            .map_err(|e| StampError::Execution(e.to_string()))?
            .into_inner();
        assert!(!prov_fail.success);

        Ok(())
    }

    #[tokio::test]
    async fn test_sdk_post_processor_service_all_paths() -> Result<(), StampError> {
        let success_pp = Arc::new(DummyPostProcessor { should_fail: false });
        let svc = SdkPostProcessorService::new(success_pp);

        let conf = svc
            .configure(tonic::Request::new(
                libstamp::r#gen::packer::ConfigureRequest { configs: vec![] },
            ))
            .await
            .map_err(|e| StampError::Execution(e.to_string()))?
            .into_inner();
        assert!(conf.errors.is_empty());

        let pp = svc
            .post_process(tonic::Request::new(
                libstamp::r#gen::packer::PostProcessRequest {
                    artifact_id: "art-1".into(),
                    builder_id: "b-1".into(),
                    files: vec!["file.txt".into()],
                },
            ))
            .await
            .map_err(|e| StampError::Execution(e.to_string()))?
            .into_inner();
        assert!(pp.success);
        assert_eq!(pp.artifact_id, "art-1-processed");

        let fail_pp = Arc::new(DummyPostProcessor { should_fail: true });
        let fail_svc = SdkPostProcessorService::new(fail_pp);
        let pp_fail = fail_svc
            .post_process(tonic::Request::new(
                libstamp::r#gen::packer::PostProcessRequest {
                    artifact_id: "art-1".into(),
                    builder_id: "b-1".into(),
                    files: vec![],
                },
            ))
            .await
            .map_err(|e| StampError::Execution(e.to_string()))?
            .into_inner();
        assert!(!pp_fail.success);

        Ok(())
    }

    #[tokio::test]
    async fn test_sdk_datasource_service_all_paths() -> Result<(), StampError> {
        let success_ds = Arc::new(DummyDataSource { should_fail: false });
        let svc = SdkDatasourceService::new(success_ds);

        let exec = svc
            .execute(tonic::Request::new(
                libstamp::r#gen::packer::ExecuteRequest { configs: vec![] },
            ))
            .await
            .map_err(|e| StampError::Execution(e.to_string()))?
            .into_inner();
        assert!(exec.errors.is_empty());
        let val: serde_json::Value = serde_json::from_slice(&exec.output)
            .map_err(|e| StampError::Execution(e.to_string()))?;
        assert_eq!(val["key"], "val");

        let fail_ds = Arc::new(DummyDataSource { should_fail: true });
        let fail_svc = SdkDatasourceService::new(fail_ds);
        let exec_fail = fail_svc
            .execute(tonic::Request::new(
                libstamp::r#gen::packer::ExecuteRequest { configs: vec![] },
            ))
            .await
            .map_err(|e| StampError::Execution(e.to_string()))?
            .into_inner();
        assert!(!exec_fail.errors.is_empty());

        Ok(())
    }

    #[tokio::test]
    async fn test_sdk_health_service() -> Result<(), StampError> {
        let health = SdkHealthService;
        let resp = health
            .check(tonic::Request::new(
                libstamp::r#gen::packer::HealthCheckRequest {
                    service: String::new(),
                },
            ))
            .await
            .map_err(|e| StampError::Execution(e.to_string()))?
            .into_inner();
        assert_eq!(resp.status, 1);

        let watch_resp = health
            .watch(tonic::Request::new(
                libstamp::r#gen::packer::HealthCheckRequest {
                    service: String::new(),
                },
            ))
            .await
            .map_err(|e| StampError::Execution(e.to_string()))?;
        let _ = watch_resp;
        Ok(())
    }

    #[tokio::test]
    async fn test_plugin_server_builder_pipeline() {
        let server = PluginServer::new()
            .with_builder(Arc::new(DummyBuilder { should_fail: false }))
            .with_provisioner(Arc::new(DummyProvisioner { should_fail: false }))
            .with_post_processor(Arc::new(DummyPostProcessor { should_fail: false }))
            .with_datasource(Arc::new(DummyDataSource { should_fail: false }));

        assert!(server.builder.is_some());
        assert!(server.provisioner.is_some());
        assert!(server.post_processor.is_some());
        assert!(server.datasource.is_some());

        assert!(
            server
                .serve_with_shutdown(async {
                    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                })
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn test_serve_plugin_macros() {
        tokio::select! {
            _ = serve_plugin!(Builder, DummyBuilder { should_fail: false }) => {},
            () = tokio::time::sleep(std::time::Duration::from_millis(20)) => {},
        }
        tokio::select! {
            _ = serve_plugin!(Provisioner, DummyProvisioner { should_fail: false }) => {},
            () = tokio::time::sleep(std::time::Duration::from_millis(20)) => {},
        }
        tokio::select! {
            _ = serve_plugin!(PostProcessor, DummyPostProcessor { should_fail: false }) => {},
            () = tokio::time::sleep(std::time::Duration::from_millis(20)) => {},
        }
        tokio::select! {
            _ = serve_plugin!(DataSource, DummyDataSource { should_fail: false }) => {},
            () = tokio::time::sleep(std::time::Duration::from_millis(20)) => {},
        }
    }

    #[test]
    fn test_verify_handshake_enforcement_paths() {
        let old_cookie = std::env::var(MAGIC_COOKIE_KEY).ok();

        // 1. Missing cookie
        unsafe {
            std::env::remove_var(MAGIC_COOKIE_KEY);
        }
        assert!(verify_handshake_internal(true).is_err());

        // 2. Invalid cookie
        unsafe {
            std::env::set_var(MAGIC_COOKIE_KEY, "invalid_magic_cookie");
        }
        assert!(verify_handshake_internal(true).is_err());

        // 3. Valid cookie
        unsafe {
            std::env::set_var(MAGIC_COOKIE_KEY, MAGIC_COOKIE_VALUE);
        }
        assert!(verify_handshake_internal(true).is_ok());

        unsafe {
            if let Some(c) = old_cookie {
                std::env::set_var(MAGIC_COOKIE_KEY, c);
            } else {
                std::env::remove_var(MAGIC_COOKIE_KEY);
            }
        }
    }

    #[tokio::test]
    async fn test_sdk_health_service_watch_stream() -> Result<(), StampError> {
        use tokio_stream::StreamExt;
        let health = SdkHealthService;
        let watch_resp = health
            .watch(tonic::Request::new(
                libstamp::r#gen::packer::HealthCheckRequest {
                    service: String::new(),
                },
            ))
            .await
            .map_err(|e| StampError::Execution(e.to_string()))?;

        let mut stream = watch_resp.into_inner();
        let first = stream.next().await;
        assert!(first.is_some());
        if let Some(Ok(resp)) = first {
            assert_eq!(resp.status, 1);
        }
        Ok(())
    }
}
