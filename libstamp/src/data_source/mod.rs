//! Data Sources for template evaluation.

use crate::error::StampError;
use crate::template::DataSourceConfig;
use async_trait::async_trait;
use serde_json::Value;

/// The main trait for data sources.
#[async_trait]
pub trait DataSource: Send + Sync {
    /// Read data from the source.
    async fn read(&self) -> Result<Value, StampError>;
}

/// Create a data source from a configuration.
///
/// # Errors
/// Returns `StampError` if the data source type is unknown.
pub fn create_data_source(config: &DataSourceConfig) -> Result<Box<dyn DataSource>, StampError> {
    match config.source_type.as_str() {
        "git" => {
            let path = config.config.get("path").cloned();
            let git_binary = config.config.get("git_binary").cloned();
            Ok(Box::new(git::GitDataSource::new(
                git::GitDataSourceConfig { path, git_binary },
            )))
        }
        "file" | "local_file" | "local-file" => {
            let path = config
                .config
                .get("path")
                .or_else(|| config.config.get("filename"))
                .cloned()
                .unwrap_or_default();
            Ok(Box::new(local_file::LocalFileDataSource::new(
                local_file::LocalFileConfig { path },
            )))
        }
        "terraform" | "terraform_state" | "terraform-state" => {
            let state_path = config
                .config
                .get("state_path")
                .or_else(|| config.config.get("path"))
                .cloned();
            let output = config.config.get("output").cloned();
            Ok(Box::new(terraform::TerraformDataSource::new(
                terraform::TerraformDataSourceConfig { state_path, output },
            )))
        }
        "hcp_packer_iteration" | "hcp-packer-iteration" => {
            let bucket_name = config
                .config
                .get("bucket_name")
                .cloned()
                .unwrap_or_default();
            let channel = config.config.get("channel").cloned().unwrap_or_default();
            let allow_revoked = config
                .config
                .get("allow_revoked")
                .is_some_and(|v| v == "true");
            Ok(Box::new(hcp::HcpPackerIterationDataSource::new(
                hcp::HcpPackerIterationConfig {
                    organization: None,
                    project: None,
                    bucket_name,
                    channel,
                    allow_revoked,
                    ..Default::default()
                },
            )))
        }
        "hcp_packer_image" | "hcp-packer-image" => {
            let bucket_name = config
                .config
                .get("bucket_name")
                .cloned()
                .unwrap_or_default();
            let iteration_id = config
                .config
                .get("iteration_id")
                .cloned()
                .unwrap_or_default();
            let cloud_provider = config
                .config
                .get("cloud_provider")
                .cloned()
                .unwrap_or_default();
            let region = config.config.get("region").cloned().unwrap_or_default();
            let allow_revoked = config
                .config
                .get("allow_revoked")
                .is_some_and(|v| v == "true");
            Ok(Box::new(hcp::HcpPackerImageDataSource::new(
                hcp::HcpPackerImageConfig {
                    organization: None,
                    project: None,
                    bucket_name,
                    iteration_id,
                    cloud_provider,
                    region,
                    allow_revoked,
                    ..Default::default()
                },
            )))
        }
        "http" => {
            let url = config.config.get("url").cloned().unwrap_or_default();
            Ok(Box::new(http::HttpDataSource::new(http::HttpConfig {
                url,
                ..Default::default()
            })))
        }
        "alicloud" | "alicloud-image" | "alicloud_image" => Ok(Box::new(
            alicloud::AlicloudImageDataSource::new(alicloud::AlicloudImageConfig {
                filters: config.config.clone(),
            }),
        )),
        "vsphere" | "vsphere-virtual-machine" | "vsphere_virtual_machine" => Ok(Box::new(
            vsphere::VsphereVirtualMachineDataSource::new(vsphere::VsphereVirtualMachineConfig {
                filters: config.config.clone(),
            }),
        )),
        "external" => {
            let program = match config.config.get("program") {
                Some(p) => {
                    if let Ok(vec) = serde_json::from_str::<Vec<String>>(p) {
                        vec
                    } else if p.starts_with('[') && p.ends_with(']') {
                        p.trim_matches(|c| c == '[' || c == ']')
                            .split(',')
                            .map(|s| s.trim().trim_matches('"').to_string())
                            .collect()
                    } else {
                        vec![p.clone()]
                    }
                }
                None => Vec::new(),
            };
            let working_dir = config.config.get("working_dir").cloned();
            let mut query = config.config.clone();
            query.remove("program");
            query.remove("working_dir");
            Ok(Box::new(external::ExternalDataSource::new(
                external::ExternalConfig {
                    program,
                    query,
                    working_dir,
                },
            )))
        }
        "consul" | "consul-key" | "consul_key" => {
            let path = config.config.get("path").cloned().unwrap_or_default();
            let address = config.config.get("address").cloned();
            let token = config
                .config
                .get("token")
                .cloned()
                .map(crate::types::SecretString::new);
            let datacenter = config.config.get("datacenter").cloned();
            Ok(Box::new(consul_key::ConsulKeyDataSource::new(
                consul_key::ConsulKeyConfig {
                    path,
                    address,
                    token,
                    datacenter,
                },
            )))
        }
        "vault" | "vault-secret" | "vault_secret" => {
            let path = config.config.get("path").cloned().unwrap_or_default();
            let key = config.config.get("key").cloned();
            let address = config.config.get("address").cloned();
            let token = config
                .config
                .get("token")
                .cloned()
                .map(crate::types::SecretString::new);
            Ok(Box::new(vault_secret::VaultSecretDataSource::new(
                vault_secret::VaultSecretConfig {
                    path,
                    key,
                    address,
                    token,
                },
            )))
        }
        "plugin" | "go-plugin" => Ok(Box::new(go_plugin::GoPluginDataSource::new(
            go_plugin::GoPluginDataSourceConfig {
                source_type: config.source_type.clone(),
                plugin_path: config
                    .config
                    .get("plugin_path")
                    .cloned()
                    .unwrap_or_default(),
                endpoint: config.config.get("endpoint").cloned(),
            },
        ))),
        _ => Err(StampError::Parse(format!(
            "Unknown data source type: {}",
            config.source_type
        ))),
    }
}

/// Alibaba Cloud image data source.
pub mod alicloud;
/// Consul KV data source.
pub mod consul_key;
/// External subprocess data source.
pub mod external;
/// Git repository metadata data source.
pub mod git;
/// Out-of-process Go plugin data source RPC client.
pub mod go_plugin;
/// HCP Packer registry data sources.
pub mod hcp;
/// HTTP GET/POST data source.
pub mod http;
/// Local file content data source.
pub mod local_file;
/// Terraform state output data source.
pub mod terraform;
/// HashiCorp Vault secret data source.
pub mod vault_secret;
/// VMware vSphere virtual machine data source.
pub mod vsphere;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_data_source_hcp_packer_iteration() -> Result<(), StampError> {
        let mut config = DataSourceConfig::default();
        config.source_type = "hcp_packer_iteration".to_string();
        let _ = create_data_source(&config)?;

        let mut config2 = DataSourceConfig::default();
        config2.source_type = "hcp_packer_iteration".to_string();
        config2
            .config
            .insert("bucket_name".to_string(), "b".to_string());
        config2
            .config
            .insert("channel".to_string(), "c".to_string());
        config2
            .config
            .insert("allow_revoked".to_string(), "true".to_string());
        let _ = create_data_source(&config2)?;

        let mut config_hyphen = DataSourceConfig::default();
        config_hyphen.source_type = "hcp-packer-iteration".to_string();
        let _ = create_data_source(&config_hyphen)?;
        Ok(())
    }

    #[test]
    fn test_create_data_source_hcp_packer_image() -> Result<(), StampError> {
        let mut config = DataSourceConfig::default();
        config.source_type = "hcp_packer_image".to_string();
        let _ = create_data_source(&config)?;

        let mut config2 = DataSourceConfig::default();
        config2.source_type = "hcp_packer_image".to_string();
        config2
            .config
            .insert("bucket_name".to_string(), "b".to_string());
        config2
            .config
            .insert("iteration_id".to_string(), "i".to_string());
        config2
            .config
            .insert("cloud_provider".to_string(), "c".to_string());
        config2.config.insert("region".to_string(), "r".to_string());
        config2
            .config
            .insert("allow_revoked".to_string(), "true".to_string());
        let _ = create_data_source(&config2)?;

        let mut config_hyphen = DataSourceConfig::default();
        config_hyphen.source_type = "hcp-packer-image".to_string();
        let _ = create_data_source(&config_hyphen)?;
        Ok(())
    }

    #[test]
    fn test_create_data_source_http() -> Result<(), StampError> {
        let mut config = DataSourceConfig::default();
        config.source_type = "http".to_string();
        let _ = create_data_source(&config)?;

        let mut config2 = DataSourceConfig::default();
        config2.source_type = "http".to_string();
        config2.config.insert("url".to_string(), "u".to_string());
        let _ = create_data_source(&config2)?;
        Ok(())
    }

    #[test]
    fn test_create_data_source_alicloud() -> Result<(), StampError> {
        let mut config = DataSourceConfig::default();
        config.source_type = "alicloud".to_string();
        let _ = create_data_source(&config)?;

        let mut config2 = DataSourceConfig::default();
        config2.source_type = "alicloud-image".to_string();
        let _ = create_data_source(&config2)?;
        Ok(())
    }

    #[test]
    fn test_create_data_source_vsphere() -> Result<(), StampError> {
        let mut config = DataSourceConfig::default();
        config.source_type = "vsphere".to_string();
        let _ = create_data_source(&config)?;

        let mut config2 = DataSourceConfig::default();
        config2.source_type = "vsphere-virtual-machine".to_string();
        let _ = create_data_source(&config2)?;
        Ok(())
    }

    #[test]
    fn test_create_data_source_plugin() -> Result<(), StampError> {
        let mut config = DataSourceConfig::default();
        config.source_type = "plugin".to_string();
        config
            .config
            .insert("plugin_path".to_string(), "/bin/plugin".to_string());
        config
            .config
            .insert("endpoint".to_string(), "127.0.0.1:1234".to_string());
        let _ = create_data_source(&config)?;

        let mut config2 = DataSourceConfig::default();
        config2.source_type = "go-plugin".to_string();
        let _ = create_data_source(&config2)?;
        Ok(())
    }
    #[test]
    fn test_create_data_source_external() -> Result<(), StampError> {
        let mut config = DataSourceConfig::default();
        config.source_type = "external".to_string();
        config
            .config
            .insert("program".to_string(), r#"["echo", "hello"]"#.to_string());
        config
            .config
            .insert("working_dir".to_string(), ".".to_string());
        let _ = create_data_source(&config)?;

        let mut config2 = DataSourceConfig::default();
        config2.source_type = "external".to_string();
        config2
            .config
            .insert("program".to_string(), "echo".to_string());
        let _ = create_data_source(&config2)?;

        let mut config3 = DataSourceConfig::default();
        config3.source_type = "external".to_string();
        config3
            .config
            .insert("program".to_string(), "[bash, -c, echo]".to_string());
        let _ = create_data_source(&config3)?;

        let mut config4 = DataSourceConfig::default();
        config4.source_type = "external".to_string();
        let _ = create_data_source(&config4)?;

        Ok(())
    }

    #[tokio::test]
    async fn test_create_data_source_consul_and_vault() -> Result<(), StampError> {
        let mut consul_cfg = DataSourceConfig::default();
        consul_cfg.source_type = "consul-key".to_string();
        consul_cfg
            .config
            .insert("path".to_string(), "my/key".to_string());
        let consul_ds = create_data_source(&consul_cfg)?;
        let val = consul_ds.read().await?;
        assert_eq!(val["key"], "my/key");

        let mut vault_cfg = DataSourceConfig::default();
        vault_cfg.source_type = "vault-secret".to_string();
        vault_cfg
            .config
            .insert("path".to_string(), "secret/data/my-secret".to_string());
        vault_cfg
            .config
            .insert("key".to_string(), "token".to_string());
        let vault_ds = create_data_source(&vault_cfg)?;
        let val2 = vault_ds.read().await?;
        assert_eq!(val2["token"], "mock-val-for-token");

        Ok(())
    }

    #[tokio::test]
    async fn test_create_data_source_new_sources() -> Result<(), StampError> {
        let mut git_cfg = DataSourceConfig::default();
        git_cfg.source_type = "git".to_string();
        git_cfg.config.insert("path".to_string(), ".".to_string());
        let git_ds = create_data_source(&git_cfg)?;
        let val_git = git_ds.read().await?;
        assert!(val_git.is_object());

        let temp_dir = tempfile::tempdir()?;
        let file_path = temp_dir.path().join("test_ds_file_creation.txt");
        tokio::fs::write(&file_path, "ds file content").await?;
        let mut file_cfg = DataSourceConfig::default();
        file_cfg.source_type = "local-file".to_string();
        file_cfg
            .config
            .insert("path".to_string(), file_path.to_string_lossy().to_string());
        let file_ds = create_data_source(&file_cfg)?;
        let val_file = file_ds.read().await?;
        assert_eq!(val_file["content"], "ds file content");

        let state_path = temp_dir.path().join("test_ds_terraform.tfstate");
        tokio::fs::write(&state_path, "{\"outputs\": {\"out\": {\"value\": 42}}}").await?;
        let mut tf_cfg = DataSourceConfig::default();
        tf_cfg.source_type = "terraform-state".to_string();
        tf_cfg.config.insert(
            "state_path".to_string(),
            state_path.to_string_lossy().to_string(),
        );
        tf_cfg
            .config
            .insert("output".to_string(), "out".to_string());
        let tf_ds = create_data_source(&tf_cfg)?;
        let val_tf = tf_ds.read().await?;
        assert_eq!(val_tf, 42);

        Ok(())
    }

    #[test]
    fn test_create_data_source_unknown() {
        let mut config = DataSourceConfig::default();
        config.source_type = "unknown".to_string();
        assert!(create_data_source(&config).is_err());
    }
}
