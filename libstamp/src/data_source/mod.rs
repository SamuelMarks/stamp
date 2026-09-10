#![cfg_attr(coverage_nightly, coverage(off))]
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
        "amazon-ami" => {
            let filters = config.config.clone();
            Ok(Box::new(amazon_ami::AmazonAmiDataSource::new(
                amazon_ami::AmazonAmiConfig { filters },
            )))
        }
        "amazon-secretsmanager" => {
            let secret_id = config.config.get("secret_id").cloned().unwrap_or_default();
            let key = config.config.get("key").cloned();
            Ok(Box::new(
                amazon_secretsmanager::AmazonSecretsManagerDataSource::new(
                    amazon_secretsmanager::AmazonSecretsManagerConfig { secret_id, key },
                ),
            ))
        }
        "amazon-parameterstore" | "amazon_parameterstore" | "aws-parameterstore" => {
            let name = config.config.get("name").cloned().unwrap_or_default();
            let with_decryption = config
                .config
                .get("with_decryption")
                .is_none_or(|v| v != "false");
            Ok(Box::new(
                amazon_parameterstore::AmazonParameterStoreDataSource::new(
                    amazon_parameterstore::AmazonParameterStoreConfig {
                        name,
                        with_decryption,
                    },
                ),
            ))
        }
        "git" => {
            let path = config.config.get("path").cloned();
            Ok(Box::new(git::GitDataSource::new(
                git::GitDataSourceConfig { path },
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
        "azure" | "azure-arm" | "azure-image" | "azure_image" => Ok(Box::new(
            azure::AzureImageDataSource::new(azure::AzureImageConfig {
                filters: config.config.clone(),
            }),
        )),
        "googlecompute" | "googlecompute-image" | "googlecompute_image" => {
            Ok(Box::new(googlecompute::GooglecomputeImageDataSource::new(
                googlecompute::GooglecomputeImageConfig {
                    filters: config.config.clone(),
                },
            )))
        }
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
            let token = config.config.get("token").cloned();
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
            let token = config.config.get("token").cloned();
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

pub mod alicloud;
pub mod amazon_ami;
pub mod amazon_parameterstore;
pub mod amazon_secretsmanager;
pub mod azure;
pub mod consul_key;
pub mod external;
pub mod git;
pub mod go_plugin;
pub mod googlecompute;
pub mod hcp;
pub mod http;
pub mod local_file;
pub mod terraform;
pub mod vault_secret;
pub mod vsphere;

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::pedantic, clippy::all)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_create_data_source_amazon_ami() -> Result<(), StampError> {
        let mut config = DataSourceConfig::default();
        config.source_type = "amazon-ami".to_string();
        let ds = create_data_source(&config)?;
        let _ = ds.read().await; // dummy check
        Ok(())
    }

    #[test]
    fn test_create_data_source_secretsmanager() -> Result<(), StampError> {
        let mut config = DataSourceConfig::default();
        config.source_type = "amazon-secretsmanager".to_string();
        let _ = create_data_source(&config)?;

        let mut config2 = DataSourceConfig::default();
        config2.source_type = "amazon-secretsmanager".to_string();
        config2
            .config
            .insert("secret_id".to_string(), "foo".to_string());
        config2.config.insert("key".to_string(), "bar".to_string());
        let _ = create_data_source(&config2)?;
        Ok(())
    }

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
    fn test_create_data_source_azure() -> Result<(), StampError> {
        let mut config = DataSourceConfig::default();
        config.source_type = "azure".to_string();
        let _ = create_data_source(&config)?;

        let mut config2 = DataSourceConfig::default();
        config2.source_type = "azure-arm".to_string();
        let _ = create_data_source(&config2)?;
        Ok(())
    }

    #[test]
    fn test_create_data_source_googlecompute() -> Result<(), StampError> {
        let mut config = DataSourceConfig::default();
        config.source_type = "googlecompute".to_string();
        let _ = create_data_source(&config)?;

        let mut config2 = DataSourceConfig::default();
        config2.source_type = "googlecompute-image".to_string();
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
            .insert("program".to_string(), "[\"echo\", \"hi\"]".to_string());
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
        let mut ps_cfg = DataSourceConfig::default();
        ps_cfg.source_type = "amazon-parameterstore".to_string();
        ps_cfg
            .config
            .insert("name".to_string(), "/app/param".to_string());
        ps_cfg
            .config
            .insert("with_decryption".to_string(), "false".to_string());
        let ps_ds = create_data_source(&ps_cfg)?;
        let val_ps = ps_ds.read().await?;
        assert_eq!(val_ps["name"], "/app/param");

        let mut git_cfg = DataSourceConfig::default();
        git_cfg.source_type = "git".to_string();
        git_cfg.config.insert("path".to_string(), ".".to_string());
        let git_ds = create_data_source(&git_cfg)?;
        let val_git = git_ds.read().await?;
        assert!(val_git.is_object());

        let temp_dir = std::env::temp_dir();
        let file_path = temp_dir.join("test_ds_file_creation.txt");
        tokio::fs::write(&file_path, "ds file content")
            .await
            .map_err(|e| StampError::Execution(e.to_string()))?;
        let mut file_cfg = DataSourceConfig::default();
        file_cfg.source_type = "local-file".to_string();
        file_cfg
            .config
            .insert("path".to_string(), file_path.to_string_lossy().to_string());
        let file_ds = create_data_source(&file_cfg)?;
        let val_file = file_ds.read().await?;
        assert_eq!(val_file["content"], "ds file content");
        let _ = tokio::fs::remove_file(file_path).await;

        let state_path = temp_dir.join("test_ds_terraform.tfstate");
        tokio::fs::write(&state_path, "{\"outputs\": {\"out\": {\"value\": 42}}}")
            .await
            .map_err(|e| StampError::Execution(e.to_string()))?;
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
        let _ = tokio::fs::remove_file(state_path).await;

        Ok(())
    }

    #[test]
    fn test_create_data_source_unknown() -> Result<(), StampError> {
        let mut config = DataSourceConfig::default();
        config.source_type = "unknown".to_string();
        assert!(create_data_source(&config).is_err());
        Ok(())
    }
}
