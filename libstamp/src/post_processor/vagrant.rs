//! Implementation of the `vagrant` post-processor.
//!
//! Packages builder artifacts into Vagrant `.box` archives (gzip-compressed tar)
//! containing `metadata.json`, `Vagrantfile`, and virtual disk/machine definitions.

use crate::error::StampError;
use crate::post_processor::{Artifact, PostProcessor};
use async_trait::async_trait;
use flate2::Compression;
use flate2::write::GzEncoder;
use std::fs::File;
use std::path::Path;

/// Configuration for the `vagrant` post-processor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VagrantConfig {
    /// Output file path for the generated `.box` archive. Defaults to `package.box`.
    pub output: String,
    /// Explicit provider override (e.g. `virtualbox`, `vmware_desktop`, `qemu`, `hyperv`, `docker`, `parallels`).
    pub provider_override: Option<String>,
    /// Optional path to a custom Ruby `Vagrantfile` template to package inside the box.
    pub vagrantfile_template: Option<String>,
    /// Additional files or directories to include inside the `.box` archive.
    pub include: Vec<String>,
    /// Compression level from 0 (none) to 9 (best). Defaults to 6.
    pub compression_level: u32,
    /// Whether to keep the input artifact files after bundling into the box. Defaults to true.
    pub keep_input_artifact: bool,
    /// Whether to operate in passthrough mode, keeping raw uncompressed machine images for external packagers.
    pub passthrough: bool,
    /// Custom path for writing `_metadata.json` build metadata records.
    pub metadata_output: Option<String>,
    /// Optional list of builders this post-processor only applies to.
    pub only: Option<Vec<String>>,
    /// Optional list of builders this post-processor does not apply to.
    pub except: Option<Vec<String>>,
}

impl Default for VagrantConfig {
    fn default() -> Self {
        Self {
            output: "package.box".to_string(),
            provider_override: None,
            vagrantfile_template: None,
            include: Vec::new(),
            compression_level: 6,
            keep_input_artifact: true,
            passthrough: false,
            metadata_output: None,
            only: None,
            except: None,
        }
    }
}

/// The `vagrant` post-processor.
#[derive(Debug, Clone)]
pub struct VagrantPostProcessor {
    /// Post-processor configuration.
    pub config: VagrantConfig,
}

impl VagrantPostProcessor {
    /// Create a new `VagrantPostProcessor`.
    #[must_use]
    pub const fn new(config: VagrantConfig) -> Self {
        Self { config }
    }

    /// Creates a new `VagrantPostProcessor` from a `PostProcessorConfig`.
    #[must_use]
    pub fn from_post_processor_config(config: &crate::template::PostProcessorConfig) -> Self {
        let output = config
            .config
            .get("output")
            .cloned()
            .unwrap_or_else(|| "package.box".to_string());
        let provider_override = config.config.get("provider_override").cloned().or_else(|| {
            if config.post_processor_type == "utm-vagrant" {
                Some("utm".to_string())
            } else {
                None
            }
        });
        let vagrantfile_template = config.config.get("vagrantfile_template").cloned();
        let include = config
            .config
            .get("include")
            .and_then(|s| serde_json::from_str::<Vec<String>>(s).ok())
            .unwrap_or_default();
        let compression_level = config
            .config
            .get("compression_level")
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(6);
        let keep_input_artifact = config
            .config
            .get("keep_input_artifact")
            .map_or(true, |v| v != "false");
        let passthrough = config
            .config
            .get("passthrough")
            .is_some_and(|v| v == "true");
        let metadata_output = config.config.get("metadata_output").cloned();

        let only = if config.only.is_empty() {
            None
        } else {
            Some(config.only.clone())
        };
        let except = if config.except.is_empty() {
            None
        } else {
            Some(config.except.clone())
        };

        Self::new(VagrantConfig {
            output,
            provider_override,
            vagrantfile_template,
            include,
            compression_level,
            keep_input_artifact,
            passthrough,
            metadata_output,
            only,
            except,
        })
    }

    /// Infers the Vagrant provider string based on explicit configuration or input file extensions.
    #[must_use]
    pub fn infer_provider(&self, artifact: &Artifact) -> String {
        if let Some(ref p) = self.config.provider_override {
            return p.clone();
        }

        let id_lower = artifact.id.to_lowercase();
        if id_lower.contains("vmware") {
            return "vmware_desktop".to_string();
        }
        if id_lower.contains("qemu") {
            return "qemu".to_string();
        }
        if id_lower.contains("hyperv") {
            return "hyperv".to_string();
        }
        if id_lower.contains("docker") {
            return "docker".to_string();
        }
        if id_lower.contains("parallels") {
            return "parallels".to_string();
        }
        if id_lower.contains("utm") {
            return "utm".to_string();
        }

        for file in &artifact.files {
            let path = Path::new(file);
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            let ext_lower = ext.to_ascii_lowercase();
            let f_lower = file.to_lowercase();
            if ext_lower == "vmx" || (ext_lower == "vmdk" && f_lower.contains("vmware")) {
                return "vmware_desktop".to_string();
            }
            if ext_lower == "qcow2" || ext_lower == "img" {
                return "qemu".to_string();
            }
            if ext_lower == "vhdx" || ext_lower == "vhd" {
                return "hyperv".to_string();
            }
            if ext_lower == "pvm" {
                return "parallels".to_string();
            }
            if ext_lower == "utm" || f_lower.ends_with(".utm") {
                return "utm".to_string();
            }
        }

        "virtualbox".to_string()
    }

    /// Generates Bento-compatible `_metadata.json` record content.
    #[must_use]
    pub fn generate_bento_metadata(&self, provider: &str, artifact: &Artifact) -> String {
        let now = chrono::Utc::now().to_rfc3339();
        let box_basename = Path::new(&self.config.output)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("box");

        serde_json::to_string_pretty(&serde_json::json!({
            "name": box_basename,
            "version": "0.1.0",
            "box_basename": box_basename,
            "builder": artifact.id,
            "provider": provider,
            "files": artifact.files,
            "timestamp": now,
        }))
        .unwrap_or_default()
    }

    /// Generates the standard `metadata.json` content for the target provider.
    #[must_use]
    pub fn generate_metadata(&self, provider: &str) -> String {
        serde_json::json!({
            "provider": provider
        })
        .to_string()
    }

    /// Produces the `Vagrantfile` content to embed in the box.
    ///
    /// # Errors
    /// Returns `StampError::Io` if reading the specified template file fails.
    pub fn generate_vagrantfile(&self) -> Result<String, StampError> {
        if let Some(ref tpl_path) = self.config.vagrantfile_template {
            std::fs::read_to_string(tpl_path).map_err(StampError::Io)
        } else {
            Ok(concat!(
                "# Generated by Stamp (Packer alternative)\n",
                "Vagrant.configure(\"2\") do |config|\n",
                "  config.vm.base_mac = \"nil\"\n",
                "end\n"
            )
            .to_string())
        }
    }
}

#[async_trait]
impl PostProcessor for VagrantPostProcessor {
    fn only(&self) -> Option<&[String]> {
        self.config.only.as_deref()
    }

    fn except(&self) -> Option<&[String]> {
        self.config.except.as_deref()
    }

    async fn process(&self, artifact: Artifact) -> Result<Artifact, StampError> {
        let provider = self.infer_provider(&artifact);

        if self.config.passthrough {
            let metadata = self.generate_bento_metadata(&provider, &artifact);
            let meta_path = if let Some(ref p) = self.config.metadata_output {
                std::path::PathBuf::from(p)
            } else {
                let out_path = Path::new(&self.config.output);
                let parent = out_path.parent().unwrap_or_else(|| Path::new("."));
                parent.join("_metadata.json")
            };

            if let Some(parent) = meta_path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(&meta_path, metadata);

            return Ok(Artifact::new(
                format!("vagrant-passthrough-{provider}"),
                artifact.files,
            ));
        }

        let metadata_content = self.generate_metadata(&provider);
        let vagrantfile_content = self.generate_vagrantfile()?;

        let output_file = File::create(&self.config.output).map_err(StampError::Io)?;
        let level = Compression::new(self.config.compression_level.min(9));
        let enc = GzEncoder::new(output_file, level);
        let mut tar_builder = tar::Builder::new(enc);

        // 1. Append metadata.json
        let mut header = tar::Header::new_gnu();
        header.set_size(metadata_content.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        tar_builder
            .append_data(&mut header, "metadata.json", metadata_content.as_bytes())
            .map_err(StampError::Io)?;

        // 2. Append Vagrantfile
        let mut vf_header = tar::Header::new_gnu();
        vf_header.set_size(vagrantfile_content.len() as u64);
        vf_header.set_mode(0o644);
        vf_header.set_cksum();
        tar_builder
            .append_data(
                &mut vf_header,
                "Vagrantfile",
                vagrantfile_content.as_bytes(),
            )
            .map_err(StampError::Io)?;

        // 3. Append input artifact files
        for file_path_str in &artifact.files {
            let path = Path::new(file_path_str);
            let name = path.file_name().unwrap_or(path.as_os_str());

            if path.is_dir() {
                tar_builder
                    .append_dir_all(name, path)
                    .map_err(StampError::Io)?;
            } else if path.is_file() {
                tar_builder
                    .append_path_with_name(path, name)
                    .map_err(StampError::Io)?;
            }
        }

        // 4. Append additional included files
        for inc_str in &self.config.include {
            let path = Path::new(inc_str);
            let name = path.file_name().unwrap_or(path.as_os_str());

            if path.is_dir() {
                tar_builder
                    .append_dir_all(name, path)
                    .map_err(StampError::Io)?;
            } else if path.is_file() {
                tar_builder
                    .append_path_with_name(path, name)
                    .map_err(StampError::Io)?;
            }
        }

        tar_builder.finish().map_err(StampError::Io)?;

        Ok(Artifact::new(
            "vagrant".to_string(),
            vec![self.config.output.clone()],
        ))
    }

    fn keep_input_artifact(&self) -> bool {
        self.config.keep_input_artifact
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::pedantic, clippy::all)]
mod tests {
    use super::*;

    #[test]
    fn test_vagrant_config_defaults() {
        let config = VagrantConfig::default();
        assert_eq!(config.output, "package.box");
        assert!(config.keep_input_artifact);
        assert_eq!(config.compression_level, 6);
    }

    #[test]
    fn test_vagrant_infer_provider() {
        let pp = VagrantPostProcessor::new(VagrantConfig::default());

        let a_vb = Artifact::new("virtualbox".to_string(), vec!["box.ovf".to_string()]);
        assert_eq!(pp.infer_provider(&a_vb), "virtualbox");

        let a_vmw = Artifact::new("vmware".to_string(), vec!["disk.vmdk".to_string()]);
        assert_eq!(pp.infer_provider(&a_vmw), "vmware_desktop");

        let a_qemu = Artifact::new("qemu".to_string(), vec!["disk.qcow2".to_string()]);
        assert_eq!(pp.infer_provider(&a_qemu), "qemu");

        let a_hyperv = Artifact::new("hyperv".to_string(), vec!["disk.vhdx".to_string()]);
        assert_eq!(pp.infer_provider(&a_hyperv), "hyperv");

        let a_docker = Artifact::new("docker".to_string(), vec![]);
        assert_eq!(pp.infer_provider(&a_docker), "docker");

        let a_parallels = Artifact::new("parallels".to_string(), vec!["vm.pvm".to_string()]);
        assert_eq!(pp.infer_provider(&a_parallels), "parallels");

        let pp_override = VagrantPostProcessor::new(VagrantConfig {
            provider_override: Some("custom_provider".to_string()),
            ..Default::default()
        });
        assert_eq!(pp_override.infer_provider(&a_vb), "custom_provider");
    }

    #[tokio::test]
    async fn test_vagrant_process_bundle() -> Result<(), StampError> {
        let tmp_dir = tempfile::tempdir().map_err(StampError::Io)?;
        let dummy_file = tmp_dir.path().join("disk.vmdk");
        std::fs::write(&dummy_file, b"disk image content").map_err(StampError::Io)?;

        let custom_vf = tmp_dir.path().join("Vagrantfile.custom");
        std::fs::write(&custom_vf, b"# custom vagrantfile").map_err(StampError::Io)?;

        let box_out = tmp_dir.path().join("output.box");

        let pp = VagrantPostProcessor::new(VagrantConfig {
            output: box_out.to_string_lossy().to_string(),
            provider_override: Some("vmware_desktop".to_string()),
            vagrantfile_template: Some(custom_vf.to_string_lossy().to_string()),
            include: vec![],
            compression_level: 1,
            keep_input_artifact: true,
            ..Default::default()
        });

        let input_art = Artifact::new(
            "vmware".to_string(),
            vec![dummy_file.to_string_lossy().to_string()],
        );

        let out_art = pp.process(input_art).await?;
        assert_eq!(out_art.id, "vagrant");
        assert_eq!(out_art.files.len(), 1);
        assert!(box_out.exists());

        // Verify contents of the tar.gz box
        let file = File::open(&box_out).map_err(StampError::Io)?;
        let gz = flate2::read::GzDecoder::new(file);
        let mut archive = tar::Archive::new(gz);

        let mut entry_names = Vec::new();
        for entry in archive.entries().map_err(StampError::Io)? {
            let e = entry.map_err(StampError::Io)?;
            let p = e
                .path()
                .map_err(StampError::Io)?
                .to_string_lossy()
                .to_string();
            entry_names.push(p);
        }

        assert!(entry_names.contains(&"metadata.json".to_string()));
        assert!(entry_names.contains(&"Vagrantfile".to_string()));
        assert!(entry_names.contains(&"disk.vmdk".to_string()));

        Ok(())
    }

    #[tokio::test]
    async fn test_vagrant_passthrough_mode() -> Result<(), StampError> {
        let tmp_dir = tempfile::tempdir().map_err(StampError::Io)?;
        let raw_disk = tmp_dir.path().join("disk.qcow2");
        std::fs::write(&raw_disk, b"raw qemu disk").map_err(StampError::Io)?;
        let meta_file = tmp_dir.path().join("_metadata.json");

        let pp = VagrantPostProcessor::new(VagrantConfig {
            output: tmp_dir
                .path()
                .join("bento.box")
                .to_string_lossy()
                .to_string(),
            passthrough: true,
            metadata_output: Some(meta_file.to_string_lossy().to_string()),
            only: Some(vec!["source.qemu.vm".to_string()]),
            except: None,
            ..Default::default()
        });

        assert_eq!(pp.only(), Some(&["source.qemu.vm".to_string()][..]));
        assert_eq!(pp.except(), None);

        let input_art = Artifact::new(
            "qemu".to_string(),
            vec![raw_disk.to_string_lossy().to_string()],
        );

        let out_art = pp.process(input_art).await?;
        assert_eq!(out_art.id, "vagrant-passthrough-qemu");
        assert_eq!(out_art.files.len(), 1);
        assert_eq!(out_art.files[0], raw_disk.to_string_lossy().to_string());
        assert!(meta_file.exists());

        let meta_str = std::fs::read_to_string(&meta_file).map_err(StampError::Io)?;
        assert!(meta_str.contains("\"builder\": \"qemu\""));
        assert!(meta_str.contains("\"provider\": \"qemu\""));

        Ok(())
    }

    #[test]
    fn test_vagrant_utm_and_from_config() {
        let a_utm = Artifact::new("utm".to_string(), vec!["bundle.utm".to_string()]);
        let pp = VagrantPostProcessor::new(VagrantConfig::default());
        assert_eq!(pp.infer_provider(&a_utm), "utm");

        let mut cfg = crate::template::PostProcessorConfig {
            post_processor_type: "utm-vagrant".to_string(),
            only: vec!["utm-iso.vm".to_string()],
            ..Default::default()
        };
        cfg.config
            .insert("passthrough".to_string(), "true".to_string());
        cfg.config
            .insert("output".to_string(), "builds/utm.box".to_string());

        let pp_cfg = VagrantPostProcessor::from_post_processor_config(&cfg);
        assert_eq!(pp_cfg.config.provider_override.as_deref(), Some("utm"));
        assert_eq!(pp_cfg.config.passthrough, true);
        assert_eq!(pp_cfg.only(), Some(&["utm-iso.vm".to_string()][..]));
    }
}
