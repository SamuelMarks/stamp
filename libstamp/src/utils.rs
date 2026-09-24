#![cfg_attr(coverage_nightly, coverage(off))]
//! General utilities for Stamp.

use std::collections::HashMap;
use std::io::Write as _;
use std::path::Path;

/// Resolves the Docker executable path. Checks the `DOCKER_EXECUTABLE` environment variable.
#[must_use]
pub fn docker_executable() -> String {
    std::env::var("DOCKER_EXECUTABLE").unwrap_or_else(|_| "docker".to_string())
}

/// Mutex for synchronizing tests that mutate environment variables.
pub static ENV_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Loads and merges variables according to `HashiCorp` Packer's strict precedence rules:
/// 1. Auto-loaded `.pkrvars.hcl` / `.pkrvars.json` in directory (alphabetical)
/// 2. Auto-loaded `.auto.pkrvars.hcl` / `.auto.pkrvars.json` (alphabetical)
/// 3. Environment variables matching prefix `PKR_VAR_<variable_name>`
/// 4. Command-line `-var-file` files in order
/// 5. Command-line `-var` key-value pairs in order (highest precedence)
#[must_use]
pub fn load_variables_with_precedence(
    dir: Option<&Path>,
    var_files: Option<&[String]>,
    cli_vars: Option<&[String]>,
) -> HashMap<String, String> {
    let mut vars = HashMap::new();

    // 1 & 2. Auto-loaded files in working directory
    let target_dir = dir.unwrap_or_else(|| Path::new("."));
    if let Ok(entries) = std::fs::read_dir(target_dir) {
        let mut auto_files = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if let Some(name) = path.file_name().and_then(|n| n.to_str())
                && (name.ends_with(".pkrvars.hcl")
                    || name.ends_with(".pkrvars.json")
                    || name.ends_with(".auto.pkrvars.hcl")
                    || name.ends_with(".auto.pkrvars.json"))
            {
                auto_files.push(path);
            }
        }
        auto_files.sort();
        for path in auto_files {
            if let Ok(content) = std::fs::read_to_string(&path)
                && let Ok(parsed) =
                    serde_json::from_str::<HashMap<String, serde_json::Value>>(&content)
            {
                for (k, v) in parsed {
                    vars.insert(
                        k,
                        match v {
                            serde_json::Value::String(s) => s,
                            other => other.to_string(),
                        },
                    );
                }
            }
        }
    }

    // 3. Environment variables PKR_VAR_<name>
    for (k, v) in std::env::vars() {
        if let Some(var_name) = k.strip_prefix("PKR_VAR_") {
            vars.insert(var_name.to_string(), v);
        }
    }

    // 4. CLI -var-file files in the order passed
    if let Some(files) = var_files {
        for f in files {
            if let Ok(content) = std::fs::read_to_string(f)
                && let Ok(parsed) =
                    serde_json::from_str::<HashMap<String, serde_json::Value>>(&content)
            {
                for (k, v) in parsed {
                    vars.insert(
                        k,
                        match v {
                            serde_json::Value::String(s) => s,
                            other => other.to_string(),
                        },
                    );
                }
            }
        }
    }

    // 5. CLI -var '<key>=<val>' in the order passed (highest precedence)
    if let Some(cli) = cli_vars {
        for v in cli {
            if let Some((k, val)) = v.split_once('=') {
                vars.insert(k.to_string(), val.to_string());
            }
        }
    }

    vars
}

/// Resolves the Packer configuration path, checking `PACKER_CONFIG` environment variable,
/// falling back to `~/.packerconfig` and `~/.config/packer/packer.json`.
#[must_use]
pub fn packer_config_path() -> std::path::PathBuf {
    if let Ok(path) = std::env::var("PACKER_CONFIG") {
        return std::path::PathBuf::from(path);
    }
    if let Ok(home) = std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE")) {
        let home_path = std::path::Path::new(&home);
        let legacy = home_path.join(".packerconfig");
        if legacy.exists() {
            return legacy;
        }
        let xdg = home_path.join(".config").join("packer").join("packer.json");
        if xdg.exists() {
            return xdg;
        }
        return legacy;
    }
    std::path::PathBuf::from(".packerconfig")
}

/// Resolves the Packer cache directory, checking `PACKER_CACHE_DIR` environment variable,
/// falling back to `./packer_cache`.
#[must_use]
pub fn packer_cache_dir() -> std::path::PathBuf {
    if let Ok(dir) = std::env::var("PACKER_CACHE_DIR") {
        return std::path::PathBuf::from(dir);
    }
    std::path::PathBuf::from("packer_cache")
}

/// Caches or retrieves a file from the central `PACKER_CACHE_DIR` using `fs2` concurrent file locking,
/// validating expected SHA256 checksums before reuse.
///
/// # Errors
/// Returns `StampError` if download, locking, or verification fails.
pub async fn get_or_download_cached(
    source_url: &str,
    expected_checksum: Option<&str>,
) -> Result<std::path::PathBuf, crate::error::StampError> {
    use fs2::FileExt;
    use sha2::Digest as _;
    use std::io::{Read, Write};

    let cache_dir = packer_cache_dir();
    std::fs::create_dir_all(&cache_dir)?;

    let filename = format!("{:x}", sha2::Sha256::digest(source_url.as_bytes()));
    let cached_file = cache_dir.join(&filename);
    let lock_file_path = cache_dir.join(format!("{filename}.lock"));

    let lock_file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_file_path)?;

    lock_file.lock_exclusive()?;

    let check_cached = || -> Result<Option<std::path::PathBuf>, crate::error::StampError> {
        if cached_file.exists() {
            if let Some(expected) = expected_checksum {
                let mut file = std::fs::File::open(&cached_file)?;
                let mut hasher = sha2::Sha256::new();
                let mut buf = [0u8; 8192];
                loop {
                    let n = file.read(&mut buf)?;
                    if n == 0 {
                        break;
                    }
                    hasher.update(&buf[..n]);
                }
                let actual = hex::encode(hasher.finalize());
                if actual.eq_ignore_ascii_case(expected) {
                    return Ok(Some(cached_file.clone()));
                }
                let _ = std::fs::remove_file(&cached_file);
            } else {
                return Ok(Some(cached_file.clone()));
            }
        }
        Ok(None)
    };

    if let Some(path) = check_cached()? {
        let _ = lock_file.unlock();
        let _ = std::fs::remove_file(&lock_file_path);
        return Ok(path);
    }

    if let Ok(local_path) = std::fs::canonicalize(source_url)
        && local_path.is_file()
    {
        if let Some(expected) = expected_checksum {
            let bytes = std::fs::read(&local_path)?;
            let actual = hex::encode(sha2::Sha256::digest(&bytes));
            if !actual.eq_ignore_ascii_case(expected) {
                let _ = lock_file.unlock();
                let _ = std::fs::remove_file(&lock_file_path);
                return Err(crate::error::StampError::Validation(format!(
                    "Checksum mismatch for '{source_url}': expected {expected}, got {actual}"
                )));
            }
        }
        std::fs::copy(&local_path, &cached_file)?;
        let _ = lock_file.unlock();
        let _ = std::fs::remove_file(&lock_file_path);
        return Ok(cached_file);
    }

    let response = reqwest::get(source_url)
        .await
        .map_err(|e| crate::error::StampError::Execution(e.to_string()))?;
    let bytes = response
        .bytes()
        .await
        .map_err(|e| crate::error::StampError::Execution(e.to_string()))?;

    if let Some(expected) = expected_checksum {
        let actual = hex::encode(sha2::Sha256::digest(&bytes));
        if !actual.eq_ignore_ascii_case(expected) {
            let _ = lock_file.unlock();
            let _ = std::fs::remove_file(&lock_file_path);
            return Err(crate::error::StampError::Validation(format!(
                "Checksum mismatch for '{source_url}': expected {expected}, got {actual}"
            )));
        }
    }

    let temp_path = cache_dir.join(format!("{filename}.tmp"));
    let mut temp_file = std::fs::File::create(&temp_path)?;
    temp_file.write_all(&bytes)?;
    std::fs::rename(temp_path, &cached_file)?;

    let _ = lock_file.unlock();
    let _ = std::fs::remove_file(&lock_file_path);

    Ok(cached_file)
}

/// Cleans cache files in `cache_dir` that are older than `max_age_days`.
///
/// # Errors
/// Returns `StampError` if reading or removing cache files fails.
pub fn clean_cache(
    cache_dir: &std::path::Path,
    max_age_days: u64,
) -> Result<usize, crate::error::StampError> {
    let mut removed = 0;
    if !cache_dir.exists() {
        return Ok(0);
    }
    let now = std::time::SystemTime::now();
    let max_age_duration = std::time::Duration::from_secs(max_age_days * 86400);

    for entry in std::fs::read_dir(cache_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_file()
            && path.extension().is_none_or(|ext| ext != "lock")
            && let Ok(metadata) = entry.metadata()
            && let Ok(modified) = metadata.modified()
            && let Ok(elapsed) = now.duration_since(modified)
            && elapsed >= max_age_duration
        {
            std::fs::remove_file(&path)?;
            removed += 1;
        }
    }
    Ok(removed)
}

/// Resolves the Packer configuration directory, checking `PACKER_CONFIG_DIR` environment variable,
/// falling back to `~/.packer.d`.
#[must_use]
pub fn packer_config_dir() -> std::path::PathBuf {
    if let Ok(dir) = std::env::var("PACKER_CONFIG_DIR")
        && !dir.trim().is_empty()
    {
        return std::path::PathBuf::from(dir);
    }
    if let Ok(home) = std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE")) {
        return std::path::Path::new(&home).join(".packer.d");
    }
    std::path::PathBuf::from(".packer.d")
}

/// Resolves all plugin search directories according to Packer's hierarchy:
/// 1. Directories specified in `PACKER_PLUGIN_PATH` (colon or semicolon separated)
/// 2. Current working directory `./plugins`
/// 3. Sibling directory to the current executable
/// 4. User directory `~/.packer.d/plugins` (or `PACKER_CONFIG_DIR/plugins`)
/// 5. System XDG directory (`~/.local/share/packer/plugins`)
#[must_use]
pub fn resolve_plugin_search_paths() -> Vec<std::path::PathBuf> {
    let mut paths = Vec::new();

    if let Ok(env_path) = std::env::var("PACKER_PLUGIN_PATH") {
        for part in std::env::split_paths(&env_path) {
            if !part.as_os_str().is_empty() {
                paths.push(part);
            }
        }
    }

    paths.push(std::path::PathBuf::from("./plugins"));

    if let Ok(exe_path) = std::env::current_exe()
        && let Some(exe_dir) = exe_path.parent()
    {
        paths.push(exe_dir.join("plugins"));
    }

    paths.push(packer_config_dir().join("plugins"));

    if let Ok(home) = std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE")) {
        let home_path = std::path::Path::new(&home);
        paths.push(
            home_path
                .join(".local")
                .join("share")
                .join("packer")
                .join("plugins"),
        );
    }

    paths
}

/// Initializes structured Packer logging based on `PACKER_LOG` and `PACKER_LOG_PATH` environment variables.
///
/// Supports log levels `TRACE`, `DEBUG`, `INFO`, `WARN`, `ERROR` or numeric (`1` for debug).
///
/// # Errors
/// Returns `StampError` if log file opening or initialization fails.
pub fn init_packer_logging() -> Result<Option<String>, crate::error::StampError> {
    let log_level = match std::env::var("PACKER_LOG") {
        Ok(lvl) => match lvl.to_ascii_uppercase().as_str() {
            "1" | "DEBUG" => "DEBUG",
            "TRACE" => "TRACE",
            "INFO" => "INFO",
            "WARN" => "WARN",
            "ERROR" => "ERROR",
            _ => return Ok(None),
        },
        Err(_) => return Ok(None),
    };

    if let Ok(log_path) = std::env::var("PACKER_LOG_PATH") {
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        writeln!(file, "[{now}] [{log_level}] Packer logging initialized")?;
    }

    Ok(Some(log_level.to_string()))
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn test_docker_executable_default() {
        let _guard = ENV_MUTEX
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        unsafe {
            std::env::remove_var("DOCKER_EXECUTABLE");
        }
        assert_eq!(docker_executable(), "docker");
    }

    #[test]
    fn test_docker_executable_override() {
        let _guard = ENV_MUTEX
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        unsafe {
            std::env::set_var("DOCKER_EXECUTABLE", "podman");
        }
        assert_eq!(docker_executable(), "podman");
        unsafe {
            std::env::remove_var("DOCKER_EXECUTABLE");
        }
    }

    #[test]
    fn test_load_variables_with_precedence() -> Result<(), Box<dyn std::error::Error>> {
        let _guard = ENV_MUTEX
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        let temp_dir =
            std::env::temp_dir().join(format!("stamp_test_vars_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&temp_dir)?;

        // 1. Auto-loaded file
        let auto_file = temp_dir.join("test.auto.pkrvars.json");
        std::fs::write(
            &auto_file,
            r#"{"foo": "from_auto", "bar": "from_auto", "auto_num": 99}"#,
        )?;
        let ignore_file = temp_dir.join("ignore.txt");
        std::fs::write(&ignore_file, b"ignore")?;

        // 2. Env var
        unsafe {
            std::env::set_var("PKR_VAR_bar", "from_env");
            std::env::set_var("PKR_VAR_baz", "from_env");
        }

        // 3. Var file
        let var_file = temp_dir.join("custom.json");
        std::fs::write(
            &var_file,
            r#"{"baz": "from_var_file", "qux": "from_var_file"}"#,
        )?;
        let var_files = vec![var_file.to_string_lossy().to_string()];

        // 4. CLI var
        let cli_vars = vec!["qux=from_cli".to_string()];

        let result =
            load_variables_with_precedence(Some(&temp_dir), Some(&var_files), Some(&cli_vars));

        // Assert precedence:
        // 'foo' only in auto file
        assert_eq!(result.get("foo").map(String::as_str), Some("from_auto"));
        assert_eq!(result.get("auto_num").map(String::as_str), Some("99"));
        // 'bar' in auto file and env -> env wins
        assert_eq!(result.get("bar").map(String::as_str), Some("from_env"));
        // 'baz' in env and var_file -> var_file wins
        assert_eq!(result.get("baz").map(String::as_str), Some("from_var_file"));
        // 'qux' in var_file and cli -> cli wins
        assert_eq!(result.get("qux").map(String::as_str), Some("from_cli"));

        unsafe {
            std::env::remove_var("PKR_VAR_bar");
            std::env::remove_var("PKR_VAR_baz");
        }
        let _ = std::fs::remove_dir_all(temp_dir);
        Ok(())
    }

    #[test]
    fn test_load_variables_edge_cases() -> Result<(), Box<dyn std::error::Error>> {
        let _guard = ENV_MUTEX
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        // 1. None for all params and non-existent dir
        let empty_res = load_variables_with_precedence(None, None, None);
        assert!(!empty_res.contains_key("nonexistent_test_key_xyz"));

        let non_existent_res = load_variables_with_precedence(
            Some(Path::new("/nonexistent/random/path/test_dir")),
            None,
            None,
        );
        assert!(non_existent_res.is_empty());

        // 2. Non-string JSON values in var file
        let temp_dir =
            std::env::temp_dir().join(format!("stamp_test_vars_edge_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&temp_dir)?;

        let num_file = temp_dir.join("num.pkrvars.json");
        std::fs::write(&num_file, r#"{"count": 42, "enabled": true}"#)?;

        let broken_auto = temp_dir.join("broken.auto.pkrvars.json");
        std::fs::write(&broken_auto, b"not json")?;

        let cli_malformed = vec!["bad_format".to_string(), "valid=yes".to_string()];
        let var_files = vec![
            num_file.to_string_lossy().to_string(),
            "/nonexistent/file/path.json".to_string(),
        ];

        let result =
            load_variables_with_precedence(Some(&temp_dir), Some(&var_files), Some(&cli_malformed));
        assert_eq!(result.get("count").map(String::as_str), Some("42"));
        assert_eq!(result.get("enabled").map(String::as_str), Some("true"));
        assert_eq!(result.get("valid").map(String::as_str), Some("yes"));
        assert!(!result.contains_key("bad_format"));

        let _ = std::fs::remove_dir_all(temp_dir);
        Ok(())
    }

    #[test]
    fn test_packer_config_path() -> Result<(), Box<dyn std::error::Error>> {
        let _guard = ENV_MUTEX
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        // 1. PACKER_CONFIG set
        unsafe {
            std::env::set_var("PACKER_CONFIG", "/custom/path/config.json");
        }
        assert_eq!(
            packer_config_path(),
            std::path::PathBuf::from("/custom/path/config.json")
        );
        unsafe {
            std::env::remove_var("PACKER_CONFIG");
        }

        // 2. HOME with legacy file exists
        let temp_dir = tempfile::tempdir()?;
        let legacy_file = temp_dir.path().join(".packerconfig");
        std::fs::write(&legacy_file, b"legacy")?;
        unsafe {
            std::env::set_var("HOME", temp_dir.path());
        }
        assert_eq!(packer_config_path(), legacy_file);

        // 3. HOME with XDG file exists
        let _ = std::fs::remove_file(&legacy_file);
        let xdg_dir = temp_dir.path().join(".config").join("packer");
        std::fs::create_dir_all(&xdg_dir)?;
        let xdg_file = xdg_dir.join("packer.json");
        std::fs::write(&xdg_file, b"xdg")?;
        assert_eq!(packer_config_path(), xdg_file);

        // 4. HOME with neither file existing (returns legacy path)
        let _ = std::fs::remove_file(&xdg_file);
        assert_eq!(packer_config_path(), legacy_file);

        // 5. Neither HOME nor USERPROFILE set
        unsafe {
            std::env::remove_var("HOME");
            std::env::remove_var("USERPROFILE");
        }
        assert_eq!(
            packer_config_path(),
            std::path::PathBuf::from(".packerconfig")
        );

        Ok(())
    }

    #[test]
    fn test_packer_cache_dir() {
        let _guard = ENV_MUTEX
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        unsafe {
            std::env::set_var("PACKER_CACHE_DIR", "/custom/cache/dir");
        }
        assert_eq!(
            packer_cache_dir(),
            std::path::PathBuf::from("/custom/cache/dir")
        );
        unsafe {
            std::env::remove_var("PACKER_CACHE_DIR");
        }
        assert_eq!(packer_cache_dir(), std::path::PathBuf::from("packer_cache"));
    }

    #[test]
    fn test_packer_config_dir() -> Result<(), Box<dyn std::error::Error>> {
        let _guard = ENV_MUTEX
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        unsafe {
            std::env::set_var("PACKER_CONFIG_DIR", "/custom/config/dir");
        }
        assert_eq!(
            packer_config_dir(),
            std::path::PathBuf::from("/custom/config/dir")
        );

        // Empty PACKER_CONFIG_DIR
        unsafe {
            std::env::set_var("PACKER_CONFIG_DIR", "   ");
        }
        let temp_dir = tempfile::tempdir()?;
        unsafe {
            std::env::set_var("HOME", temp_dir.path());
        }
        assert_eq!(packer_config_dir(), temp_dir.path().join(".packer.d"));

        // Fallback when neither HOME nor USERPROFILE is set
        unsafe {
            std::env::remove_var("PACKER_CONFIG_DIR");
            std::env::remove_var("HOME");
            std::env::remove_var("USERPROFILE");
        }
        assert_eq!(packer_config_dir(), std::path::PathBuf::from(".packer.d"));
        Ok(())
    }

    #[tokio::test]
    async fn test_get_or_download_cached_and_clean() -> Result<(), Box<dyn std::error::Error>> {
        use sha2::Digest as _;
        let _guard = ENV_MUTEX
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        let temp_dir = tempfile::tempdir()?;
        unsafe {
            std::env::set_var("PACKER_CACHE_DIR", temp_dir.path());
        }

        // 1. Local file with checksum
        let source_file = temp_dir.path().join("source.iso");
        std::fs::write(&source_file, b"ISO_CONTENT_12345")?;
        let expected_sha256 = hex::encode(sha2::Sha256::digest(b"ISO_CONTENT_12345"));

        let source_path_str = match source_file.to_str() {
            Some(s) => s,
            None => return Err("Invalid path string".into()),
        };

        let cached = get_or_download_cached(source_path_str, Some(&expected_sha256)).await?;
        assert!(cached.exists());

        // 2. Re-read from cache with checksum
        let cached2 = get_or_download_cached(source_path_str, Some(&expected_sha256)).await?;
        assert_eq!(cached, cached2);

        // 3. Re-read from cache without checksum
        let cached3 = get_or_download_cached(source_path_str, None).await?;
        assert_eq!(cached, cached3);

        // 4. Local file without checksum
        let other_file = temp_dir.path().join("other.bin");
        std::fs::write(&other_file, b"OTHER_DATA")?;
        let other_path_str = match other_file.to_str() {
            Some(s) => s,
            None => return Err("Invalid path string".into()),
        };
        let other_cached = get_or_download_cached(other_path_str, None).await?;
        assert!(other_cached.exists());

        // 5. Checksum mismatch on local file
        let bad_checksum = get_or_download_cached(source_path_str, Some("bad_hash")).await;
        assert!(bad_checksum.is_err());

        // 6. Cache file corrupted (mismatch causes removal and re-fetching)
        std::fs::write(&cached, b"CORRUPTED_CACHE")?;
        let re_cached = get_or_download_cached(source_path_str, Some(&expected_sha256)).await?;
        assert_eq!(re_cached, cached);

        // 7. Clean cache
        let cleaned = clean_cache(temp_dir.path(), 0)?;
        assert!(cleaned >= 1);

        // 7b. Clean cache with recent files (max_age_days = 100, none removed)
        let fresh_file = temp_dir.path().join("fresh.bin");
        std::fs::write(&fresh_file, b"fresh")?;
        let cleaned_fresh = clean_cache(temp_dir.path(), 100)?;
        assert_eq!(cleaned_fresh, 0);

        // 8. Clean cache on nonexistent directory
        let nonexistent = temp_dir.path().join("nonexistent_sub");
        assert_eq!(clean_cache(&nonexistent, 0)?, 0);

        unsafe {
            std::env::remove_var("PACKER_CACHE_DIR");
        }
        Ok(())
    }

    #[tokio::test]
    async fn test_get_or_download_cached_http() -> Result<(), Box<dyn std::error::Error>> {
        use sha2::Digest as _;
        let _guard = ENV_MUTEX
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        let temp_dir = tempfile::tempdir()?;
        unsafe {
            std::env::set_var("PACKER_CACHE_DIR", temp_dir.path());
        }

        let mut server = mockito::Server::new_async().await;
        let body = b"HTTP_DOWNLOADED_PAYLOAD";
        let expected_sha256 = hex::encode(sha2::Sha256::digest(body));

        let _mock = server
            .mock("GET", "/file.tar.gz")
            .with_status(200)
            .with_body(body)
            .expect(3)
            .create_async()
            .await;

        let url = format!("{}/file.tar.gz", server.url());

        // 1. Download with valid checksum
        let cached = get_or_download_cached(&url, Some(&expected_sha256)).await?;
        assert!(cached.exists());
        assert_eq!(std::fs::read(&cached)?, body);

        // Clean cache to test download without checksum
        let _ = clean_cache(temp_dir.path(), 0);

        // 2. Download without checksum (None)
        let cached_no_cs = get_or_download_cached(&url, None).await?;
        assert!(cached_no_cs.exists());
        assert_eq!(std::fs::read(&cached_no_cs)?, body);

        // Clean cache to test checksum mismatch
        let _ = clean_cache(temp_dir.path(), 0);

        // 3. Download with checksum mismatch
        let mismatch_res = get_or_download_cached(&url, Some("bad_checksum_hash")).await;
        assert!(mismatch_res.is_err());

        unsafe {
            std::env::remove_var("PACKER_CACHE_DIR");
        }
        Ok(())
    }

    #[test]
    fn test_resolve_plugin_search_paths() {
        let _guard = ENV_MUTEX
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        unsafe {
            std::env::set_var("PACKER_PLUGIN_PATH", "/custom/plugins::/other/plugins:");
        }
        let paths = resolve_plugin_search_paths();
        assert!(paths.contains(&std::path::PathBuf::from("/custom/plugins")));
        assert!(paths.contains(&std::path::PathBuf::from("/other/plugins")));
        assert!(paths.contains(&std::path::PathBuf::from("./plugins")));

        // Test with PACKER_PLUGIN_PATH unset and HOME/USERPROFILE unset
        unsafe {
            std::env::remove_var("PACKER_PLUGIN_PATH");
            std::env::remove_var("HOME");
            std::env::remove_var("USERPROFILE");
        }
        let paths_no_env = resolve_plugin_search_paths();
        assert!(paths_no_env.contains(&std::path::PathBuf::from("./plugins")));
    }

    #[test]
    fn test_init_packer_logging() -> Result<(), Box<dyn std::error::Error>> {
        let _guard = ENV_MUTEX
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        let temp_dir = tempfile::tempdir()?;
        let temp_log = temp_dir.path().join("test_packer_log.log");
        let temp_log_str = match temp_log.to_str() {
            Some(s) => s,
            None => return Err("Invalid path string".into()),
        };

        // 1. "1" level
        unsafe {
            std::env::set_var("PACKER_LOG", "1");
            std::env::set_var("PACKER_LOG_PATH", temp_log_str);
        }
        let lvl = init_packer_logging()?;
        assert_eq!(lvl.as_deref(), Some("DEBUG"));
        assert!(temp_log.exists());

        // 2. TRACE level
        unsafe {
            std::env::set_var("PACKER_LOG", "TRACE");
        }
        assert_eq!(init_packer_logging()?.as_deref(), Some("TRACE"));

        // 3. INFO level without PACKER_LOG_PATH
        unsafe {
            std::env::set_var("PACKER_LOG", "INFO");
            std::env::remove_var("PACKER_LOG_PATH");
        }
        assert_eq!(init_packer_logging()?.as_deref(), Some("INFO"));

        // 4. WARN level
        unsafe {
            std::env::set_var("PACKER_LOG", "warn");
        }
        assert_eq!(init_packer_logging()?.as_deref(), Some("WARN"));

        // 5. ERROR level
        unsafe {
            std::env::set_var("PACKER_LOG", "error");
        }
        assert_eq!(init_packer_logging()?.as_deref(), Some("ERROR"));

        // 6. Unknown level
        unsafe {
            std::env::set_var("PACKER_LOG", "unknown_level");
        }
        assert_eq!(init_packer_logging()?, None);

        // 7. Unset PACKER_LOG
        unsafe {
            std::env::remove_var("PACKER_LOG");
        }
        assert_eq!(init_packer_logging()?, None);

        Ok(())
    }
}
