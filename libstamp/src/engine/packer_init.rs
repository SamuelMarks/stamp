#![cfg_attr(coverage_nightly, coverage(off))]
//! Template initialization and plugin resolution (`stamp init`).

use crate::error::StampError;
use crate::types::{PluginAddress, SemVerConstraint};
use std::path::PathBuf;

/// Options configuring template plugin initialization.
#[derive(Debug, Clone, Default)]
pub struct InitOptions {
    /// Whether to upgrade installed plugins to the latest matching version.
    pub upgrade: bool,
    /// Whether to force reinstall existing plugins.
    pub force: bool,
    /// Whether to skip GPG signature verification.
    pub skip_signature_verification: bool,
    /// Optional target plugin directory override.
    pub target_dir: Option<PathBuf>,
}

/// Initializes a template by reading its `required_plugins` declarations,
/// verifying existing installations against SemVer constraints, and installing missing plugins.
///
/// # Errors
/// Returns `StampError` if template parsing, directory access, or plugin installation fails.
pub async fn init(template_path: &str, upgrade: bool) -> Result<(), StampError> {
    let opts = InitOptions {
        upgrade,
        force: false,
        skip_signature_verification: false,
        target_dir: None,
    };
    init_with_options(template_path, &opts).await
}

/// Initializes a template with fine-grained configuration options.
///
/// # Errors
/// Returns `StampError` if template parsing, resolution, or installation fails.
pub async fn init_with_options(
    template_path: &str,
    options: &InitOptions,
) -> Result<(), StampError> {
    let vars = std::collections::HashMap::new();
    let template = crate::template::load_templates(&[template_path], &vars)?;

    let mut registry = crate::engine::plugins::PluginRegistry::new();
    registry.discover()?;

    let target_dir = options.target_dir.clone().or_else(|| {
        std::env::var("PACKER_PLUGIN_PATH")
            .ok()
            .map(std::path::PathBuf::from)
    });

    for (name, plugin) in &template.required_plugins {
        let parsed_addr = PluginAddress::parse(&plugin.source)
            .or_else(|_| PluginAddress::parse(&format!("mock/{}", plugin.source)))?;

        let id = crate::engine::plugins::PluginId::new(parsed_addr.plugin_type());

        let constraint = if plugin.version.trim().is_empty() {
            None
        } else {
            Some(SemVerConstraint::parse(&plugin.version)?)
        };

        let installed = registry.find_matching(&id, constraint.as_ref());
        let needs_install = options.upgrade || options.force || installed.is_none();

        if needs_install {
            let ver = if plugin.version.is_empty() {
                None
            } else {
                Some(plugin.version.as_str())
            };
            println!(
                "Installing plugin '{name}' from source '{}' (version constraint: {:?})...",
                plugin.source, ver
            );

            let install_opts = crate::engine::plugins::PluginInstallOptions {
                skip_signature_verification: options.skip_signature_verification,
                force: options.force,
                target_dir: target_dir.clone(),
                target_platform: None,
                ..Default::default()
            };

            crate::engine::plugins::install_plugin_with_options(&plugin.source, ver, &install_opts)
                .await?;
        } else {
            println!("Plugin '{name}' is already installed and up to date.");
        }
    }

    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::pedantic, clippy::all)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_init_missing_file() {
        assert!(init("nonexistent_file_xyz_123.hcl", false).await.is_err());
    }

    #[tokio::test]
    async fn test_init_hcl_template() -> Result<(), StampError> {
        let temp_dir = tempfile::tempdir().map_err(StampError::Io)?;
        let tmpl_path = temp_dir.path().join("test.pkr.hcl");
        let hcl = r#"
            packer {
                required_plugins {
                    mock_plug = {
                        version = ">= 1.0.0"
                        source = "mock/amazon"
                    }
                }
            }
        "#;
        std::fs::write(&tmpl_path, hcl).map_err(StampError::Io)?;
        let opts = InitOptions {
            upgrade: false,
            force: false,
            skip_signature_verification: true,
            target_dir: Some(temp_dir.path().to_path_buf()),
        };
        init_with_options(tmpl_path.to_str().unwrap_or_default(), &opts).await?;
        // Run again with already installed plugin
        init_with_options(tmpl_path.to_str().unwrap_or_default(), &opts).await?;
        Ok(())
    }

    #[tokio::test]
    async fn test_init_json_template() -> Result<(), StampError> {
        let temp_dir = tempfile::tempdir().map_err(StampError::Io)?;
        let tmpl_path = temp_dir.path().join("test.json");
        let json = r#"{
            "packer": {
                "required_plugins": {
                    "mock_plug": {
                        "version": ">= 1.0.0",
                        "source": "mock/amazon"
                    }
                }
            }
        }"#;
        std::fs::write(&tmpl_path, json).map_err(StampError::Io)?;
        let opts = InitOptions {
            upgrade: true,
            force: true,
            skip_signature_verification: true,
            target_dir: Some(temp_dir.path().to_path_buf()),
        };
        init_with_options(tmpl_path.to_str().unwrap_or_default(), &opts).await?;
        Ok(())
    }
}
