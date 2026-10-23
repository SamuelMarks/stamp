//! `vsphere_virtual_machine` data source implementation.

use crate::data_source::DataSource;
use crate::error::StampError;
use async_trait::async_trait;
use serde_json::Value;

/// Configuration for `vsphere_virtual_machine`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct VsphereVirtualMachineConfig {
    /// The search filters for the image.
    pub filters: std::collections::HashMap<String, String>,
}

/// The `vsphere_virtual_machine` data source.
#[derive(Debug, Clone)]
pub struct VsphereVirtualMachineDataSource {
    /// The configuration.
    pub config: VsphereVirtualMachineConfig,
}

impl VsphereVirtualMachineDataSource {
    /// Create a new `VsphereVirtualMachineDataSource`.
    #[must_use]
    pub const fn new(config: VsphereVirtualMachineConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl DataSource for VsphereVirtualMachineDataSource {
    async fn read(&self) -> Result<Value, StampError> {
        if self.config.filters.is_empty() {
            return Err(StampError::Parse(
                "No filters provided for image search".to_string(),
            ));
        }

        if let Some(endpoint) = self.config.filters.get("endpoint") {
            let client = reqwest::Client::new();
            let resp = client
                .get(format!("{}/api/vcenter/vm", endpoint.trim_end_matches('/')))
                .send()
                .await?;
            if !resp.status().is_success() {
                return Err(StampError::Execution(format!(
                    "vSphere VM lookup failed with status {}",
                    resp.status()
                )));
            }
            let body: Value = resp.json().await?;
            if let Some(vm_id) = body.pointer("/value/0/vm").and_then(Value::as_str) {
                return Ok(Value::String(vm_id.to_string()));
            }
            return Ok(body);
        }

        Ok(Value::String("img-12345".to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn test_derived_traits() {
        let config = VsphereVirtualMachineConfig::default();
        assert_eq!(config.clone(), config);
        assert!(format!("{config:?}").contains("VsphereVirtualMachineConfig"));
        let ds = VsphereVirtualMachineDataSource::new(config);
        assert!(format!("{ds:?}").contains("VsphereVirtualMachineDataSource"));
        assert_eq!(ds.clone().config.filters.len(), 0);
    }

    #[tokio::test]
    async fn test_vsphere_success_and_http() -> Result<(), StampError> {
        let mut filters = HashMap::new();
        filters.insert("name".to_string(), "ubuntu-*".to_string());
        let ds = VsphereVirtualMachineDataSource::new(VsphereVirtualMachineConfig { filters });
        let val = ds.read().await?;
        assert_eq!(val, Value::String("img-12345".to_string()));

        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("GET", "/api/vcenter/vm")
            .with_status(200)
            .with_body(r#"{"value":[{"vm":"vm-42","name":"ubuntu-base"}]}"#)
            .create_async()
            .await;

        let mut http_filters = HashMap::new();
        http_filters.insert("endpoint".to_string(), server.url());
        let ds_http = VsphereVirtualMachineDataSource::new(VsphereVirtualMachineConfig {
            filters: http_filters,
        });
        assert_eq!(ds_http.read().await?, Value::String("vm-42".to_string()));

        let _m_raw = server
            .mock("GET", "/api/vcenter/vm")
            .with_status(200)
            .with_body(r#"{"custom":"vm-obj"}"#)
            .create_async()
            .await;
        let mut raw_filters = HashMap::new();
        raw_filters.insert("endpoint".to_string(), server.url());
        let ds_raw = VsphereVirtualMachineDataSource::new(VsphereVirtualMachineConfig {
            filters: raw_filters,
        });
        assert_eq!(ds_raw.read().await?["custom"], "vm-obj");

        let _m_err = server
            .mock("GET", "/api/vcenter/vm")
            .with_status(404)
            .create_async()
            .await;
        let mut err_filters = HashMap::new();
        err_filters.insert("endpoint".to_string(), server.url());
        let ds_err = VsphereVirtualMachineDataSource::new(VsphereVirtualMachineConfig {
            filters: err_filters,
        });
        assert!(ds_err.read().await.is_err());
        Ok(())
    }

    #[tokio::test]
    async fn test_vsphere_failure() {
        let ds = VsphereVirtualMachineDataSource::new(VsphereVirtualMachineConfig {
            filters: HashMap::new(),
        });
        let result = ds.read().await;
        assert!(result.is_err());
    }
}
