//! Implementation of the `compress` post-processor.

use crate::error::StampError;
use crate::post_processor::{Artifact, PostProcessor};
use async_trait::async_trait;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::Path;

/// Configuration for the `compress` post-processor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompressConfig {
    /// Format of compression: "zip", "tar", "tar.gz", "tar.bz2". Defaults to "tar.gz".
    pub format: String,
    /// The output path of the compressed artifact.
    pub output: String,
    /// Whether to keep the input artifact files. Defaults to false.
    pub keep_input_artifact: bool,
    /// Identifier for this post-processor.
    pub identifier: String,
}

impl Default for CompressConfig {
    fn default() -> Self {
        Self {
            format: "tar.gz".to_string(),
            output: "packer_{{ .BuildName }}.tar.gz".to_string(),
            keep_input_artifact: false,
            identifier: "compress".to_string(),
        }
    }
}

/// The `compress` post-processor.
#[derive(Debug, Clone)]
pub struct CompressPostProcessor {
    /// The configuration.
    pub config: CompressConfig,
}

impl CompressPostProcessor {
    /// Create a new `CompressPostProcessor`.
    #[must_use]
    pub const fn new(config: CompressConfig) -> Self {
        Self { config }
    }

    /// Perform a tar archiving operation with optional gzip or bzip2 compression.
    fn archive_tar(
        artifact: &Artifact,
        output_path: &str,
        compression: Option<&str>,
    ) -> Result<(), StampError> {
        let file = File::create(output_path).map_err(StampError::Io)?;

        let mut builder: tar::Builder<Box<dyn Write>> = match compression {
            Some("gz") => {
                let enc = flate2::write::GzEncoder::new(file, flate2::Compression::default());
                tar::Builder::new(Box::new(enc))
            }
            Some("bz2") => {
                let enc = bzip2::write::BzEncoder::new(file, bzip2::Compression::default());
                tar::Builder::new(Box::new(enc))
            }
            _ => tar::Builder::new(Box::new(file)),
        };

        for file_path in &artifact.files {
            let path = Path::new(file_path);
            let name = path.file_name().unwrap_or(path.as_os_str());

            if path.is_dir() {
                builder.append_dir_all(name, path).map_err(StampError::Io)?;
            } else if path.is_file() {
                builder
                    .append_path_with_name(path, name)
                    .map_err(StampError::Io)?;
            }
        }

        builder.finish().map_err(StampError::Io)?;
        Ok(())
    }

    /// Perform a zip archiving operation with recursive directory handling.
    fn archive_zip(artifact: &Artifact, output_path: &str) -> Result<(), StampError> {
        let file = File::create(output_path).map_err(StampError::Io)?;
        let mut zip = zip::ZipWriter::new(file);

        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);

        for file_path in &artifact.files {
            let root_path = Path::new(file_path);
            if root_path.is_dir() {
                Self::zip_add_dir_recursive(&mut zip, root_path, root_path, options)?;
            } else if root_path.is_file() {
                let file_name = root_path
                    .file_name()
                    .unwrap_or(root_path.as_os_str())
                    .to_string_lossy();
                zip.start_file(file_name, options)
                    .map_err(|e| StampError::Provisioner(format!("Zip error: {e}")))?;
                let mut f = File::open(root_path).map_err(StampError::Io)?;
                let mut buffer = Vec::new();
                f.read_to_end(&mut buffer).map_err(StampError::Io)?;
                zip.write_all(&buffer).map_err(StampError::Io)?;
            }
        }

        zip.finish()
            .map_err(|e| StampError::Provisioner(format!("Zip error: {e}")))?;
        Ok(())
    }

    /// Recursively adds a directory to a zip archive.
    fn zip_add_dir_recursive<W: Write + std::io::Seek>(
        zip: &mut zip::ZipWriter<W>,
        base_dir: &Path,
        current_dir: &Path,
        options: zip::write::SimpleFileOptions,
    ) -> Result<(), StampError> {
        let Ok(entries) = fs::read_dir(current_dir) else {
            return Ok(());
        };

        for entry in entries.flatten() {
            let path = entry.path();
            let relative = path
                .strip_prefix(base_dir)
                .unwrap_or(&path)
                .to_string_lossy();

            if path.is_dir() {
                zip.add_directory(format!("{relative}/"), options)
                    .map_err(|e| StampError::Provisioner(format!("Zip error: {e}")))?;
                Self::zip_add_dir_recursive(zip, base_dir, &path, options)?;
            } else if path.is_file() {
                zip.start_file(relative, options)
                    .map_err(|e| StampError::Provisioner(format!("Zip error: {e}")))?;
                let mut f = File::open(&path).map_err(StampError::Io)?;
                let mut buffer = Vec::new();
                f.read_to_end(&mut buffer).map_err(StampError::Io)?;
                zip.write_all(&buffer).map_err(StampError::Io)?;
            }
        }

        Ok(())
    }
}

#[async_trait]
impl PostProcessor for CompressPostProcessor {
    async fn process(&self, artifact: Artifact) -> Result<Artifact, StampError> {
        if self.config.identifier.is_empty() {
            return Err(StampError::Provisioner("Identifier is empty".to_string()));
        }

        let output_path = self.config.output.replace("{{ .BuildName }}", &artifact.id);

        match self.config.format.as_str() {
            "tar" => {
                Self::archive_tar(&artifact, &output_path, None)?;
            }
            "tar.gz" | "tgz" => {
                Self::archive_tar(&artifact, &output_path, Some("gz"))?;
            }
            "tar.bz2" | "tbz2" => {
                Self::archive_tar(&artifact, &output_path, Some("bz2"))?;
            }
            "zip" => {
                Self::archive_zip(&artifact, &output_path)?;
            }
            other => {
                return Err(StampError::Provisioner(format!(
                    "Unsupported compression format: {other}"
                )));
            }
        }

        // Enforce keep_input_artifact: delete intermediate input files if false
        if !self.config.keep_input_artifact {
            for f in &artifact.files {
                let p = Path::new(f);
                if p.exists() {
                    if p.is_dir() {
                        let _ = fs::remove_dir_all(p);
                    } else {
                        let _ = fs::remove_file(p);
                    }
                }
            }
        }

        let mut new_artifact = artifact;
        new_artifact.id = format!("{}-{}", new_artifact.id, self.config.identifier);
        new_artifact.files = vec![output_path];

        Ok(new_artifact)
    }

    fn keep_input_artifact(&self) -> bool {
        self.config.keep_input_artifact
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::pedantic, clippy::all)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_compress_tar_and_tar_gz() -> Result<(), StampError> {
        let tmp_dir = std::env::temp_dir();
        let input_file = tmp_dir.join(format!("stamp_tar_{}.txt", uuid::Uuid::new_v4()));
        fs::write(&input_file, b"content for tar").map_err(StampError::Io)?;

        let out_tar = tmp_dir.join(format!("stamp_out_{}.tar", uuid::Uuid::new_v4()));
        let config_tar = CompressConfig {
            format: "tar".to_string(),
            output: out_tar.to_string_lossy().to_string(),
            keep_input_artifact: true,
            identifier: "compressed".to_string(),
        };
        let proc = CompressPostProcessor::new(config_tar);
        let art = Artifact::new(
            "test_art".to_string(),
            vec![input_file.to_string_lossy().to_string()],
        );
        let res = proc.process(art).await?;
        assert_eq!(res.id, "test_art-compressed");
        assert_eq!(res.files, vec![out_tar.to_string_lossy().to_string()]);
        assert!(out_tar.exists());

        let out_targz = tmp_dir.join(format!("stamp_out_{}.tar.gz", uuid::Uuid::new_v4()));
        let config_targz = CompressConfig {
            format: "tar.gz".to_string(),
            output: out_targz.to_string_lossy().to_string(),
            keep_input_artifact: false,
            identifier: "compressed".to_string(),
        };
        let proc2 = CompressPostProcessor::new(config_targz);
        let art2 = Artifact::new(
            "test_art2".to_string(),
            vec![input_file.to_string_lossy().to_string()],
        );
        let res2 = proc2.process(art2).await?;
        assert_eq!(res2.id, "test_art2-compressed");
        assert!(out_targz.exists());
        // input_file removed because keep_input_artifact is false
        assert!(!input_file.exists());

        let _ = fs::remove_file(out_tar);
        let _ = fs::remove_file(out_targz);
        Ok(())
    }

    #[tokio::test]
    async fn test_compress_tar_bz2() -> Result<(), StampError> {
        let tmp_dir = std::env::temp_dir();
        let input_file = tmp_dir.join(format!("stamp_bz_{}.txt", uuid::Uuid::new_v4()));
        fs::write(&input_file, b"content for bz2").map_err(StampError::Io)?;

        let out_tbz2 = tmp_dir.join(format!("stamp_out_{}.tar.bz2", uuid::Uuid::new_v4()));
        let config = CompressConfig {
            format: "tar.bz2".to_string(),
            output: out_tbz2.to_string_lossy().to_string(),
            keep_input_artifact: true,
            identifier: "compressed".to_string(),
        };
        let proc = CompressPostProcessor::new(config);
        let art = Artifact::new(
            "test_bz2".to_string(),
            vec![input_file.to_string_lossy().to_string()],
        );
        let res = proc.process(art).await?;
        assert_eq!(res.id, "test_bz2-compressed");
        assert!(out_tbz2.exists());

        let _ = fs::remove_file(input_file);
        let _ = fs::remove_file(out_tbz2);
        Ok(())
    }

    #[tokio::test]
    async fn test_compress_zip_dir_and_file() -> Result<(), StampError> {
        let tmp_dir = std::env::temp_dir().join(format!("stamp_zip_dir_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&tmp_dir).map_err(StampError::Io)?;
        let sub_file = tmp_dir.join("inner.txt");
        fs::write(&sub_file, b"inner zip content").map_err(StampError::Io)?;

        let out_zip = std::env::temp_dir().join(format!("stamp_out_{}.zip", uuid::Uuid::new_v4()));
        let config = CompressConfig {
            format: "zip".to_string(),
            output: out_zip.to_string_lossy().to_string(),
            keep_input_artifact: true,
            identifier: "zipped".to_string(),
        };
        let proc = CompressPostProcessor::new(config);
        let art = Artifact::new(
            "test_zip".to_string(),
            vec![tmp_dir.to_string_lossy().to_string()],
        );
        let res = proc.process(art).await?;
        assert_eq!(res.id, "test_zip-zipped");
        assert!(out_zip.exists());

        let _ = fs::remove_dir_all(tmp_dir);
        let _ = fs::remove_file(out_zip);
        Ok(())
    }

    #[tokio::test]
    async fn test_compress_unsupported_format() {
        let config = CompressConfig {
            format: "unsupported_xyz".to_string(),
            ..Default::default()
        };
        let proc = CompressPostProcessor::new(config);
        let art = Artifact::new("base".to_string(), vec![]);
        assert!(proc.process(art).await.is_err());
    }

    #[tokio::test]
    async fn test_compress_empty_identifier() {
        let config = CompressConfig {
            identifier: String::new(),
            ..Default::default()
        };
        let proc = CompressPostProcessor::new(config);
        let art = Artifact::new("base".to_string(), vec![]);
        assert!(proc.process(art).await.is_err());
    }

    #[test]
    fn test_derived_traits() {
        let config1 = CompressConfig::default();
        let config2 = config1.clone();
        assert_eq!(config1, config2);
        assert_eq!(format!("{config1:?}"), format!("{config2:?}"));
        let st1 = CompressPostProcessor::new(config1);
        let st2 = st1.clone();
        assert_eq!(format!("{st1:?}"), format!("{st2:?}"));
    }
}
