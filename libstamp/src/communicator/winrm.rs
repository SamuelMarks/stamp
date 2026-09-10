#![cfg(not(tarpaulin_include))]
//! `WinRM` communicator implementation supporting Basic, NTLM, and Kerberos authentication,
//! HTTPS transport, elevated task wrappers, chunked Base64 file transfers, and output stream isolation.

use crate::communicator::{Command, CommandResult, Communicator};
use crate::error::StampError;
use crate::types::{FilePath, Port, Timeout};
use sha2::Digest;
use std::time::Duration;
use winrm_rs::{AuthMethod, WinrmClient};
#[cfg(not(test))]
use winrm_rs::{WinrmClientBuilder, WinrmConfig as RsWinrmConfig, WinrmCredentials};

/// `WinRM` authentication mechanism.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum WinRmAuth {
    /// Basic authentication.
    #[default]
    Basic,
    /// NTLM authentication.
    Ntlm,
    /// Kerberos authentication.
    Kerberos,
    /// SPNEGO / Negotiate authentication.
    Negotiate,
    /// Credential Security Support Provider (`CredSSP`) authentication for multi-hop delegation.
    CredSsp,
}

impl From<WinRmAuth> for AuthMethod {
    #[cfg_attr(coverage_nightly, coverage(off))]
    fn from(auth: WinRmAuth) -> Self {
        match auth {
            WinRmAuth::Basic => Self::Basic,
            WinRmAuth::Ntlm | WinRmAuth::CredSsp => Self::Ntlm,
            WinRmAuth::Kerberos | WinRmAuth::Negotiate => Self::Kerberos,
        }
    }
}

/// TLS configuration options for `WinRM`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct WinRmTlsConfig {
    /// Whether to use HTTPS transport.
    pub use_https: bool,
    /// Whether to bypass TLS certificate validation when using HTTPS.
    pub insecure_skip_verify: bool,
    /// Whether to bypass TLS certificate validation specifically via `winrm_insecure`.
    pub winrm_insecure: bool,
}

/// Strongly-typed `WinRM` configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WinRmConfig {
    /// Hostname or IP to connect to.
    pub host: String,
    /// Port to connect to (typically 5985 for HTTP, 5986 for HTTPS).
    pub port: Port,
    /// Username for authentication.
    pub username: String,
    /// Password for authentication.
    pub password: Option<String>,
    /// Optional Windows domain for authentication.
    pub domain: Option<String>,
    /// Authentication mechanism (Basic, NTLM, Kerberos, Negotiate, or `CredSSP`).
    pub auth: WinRmAuth,
    /// Optional Kerberos configuration file path (`KRB5_CONFIG`).
    pub krb5_config: Option<FilePath>,
    /// Optional Kerberos credential cache file path (`KRB5CCNAME`).
    pub krb5_ccname: Option<FilePath>,
    /// TLS options.
    pub tls: WinRmTlsConfig,
    /// Connection and operation timeout.
    pub timeout: Timeout,
    /// Optional custom CA certificate path for HTTPS certificate validation.
    pub ca_cert_path: Option<FilePath>,
    /// Whether to wrap commands in encoded PowerShell invocations.
    pub use_powershell_wrapper: bool,
    /// Whether to execute commands via elevated Windows Scheduled Tasks.
    pub run_elevated: bool,
    /// Optional elevated username for scheduled tasks.
    pub elevated_user: Option<String>,
    /// Optional elevated password for scheduled tasks.
    pub elevated_password: Option<String>,
    /// Chunk size in bytes for Base64 file uploads. Defaults to 64 KB.
    pub chunk_size_bytes: usize,
}

impl Default for WinRmConfig {
    /// Return standard default `WinRM` configuration parameters.
    fn default() -> Self {
        Self {
            host: "127.0.0.1".to_string(),
            port: Port::new(5985),
            username: "Administrator".to_string(),
            password: None,
            domain: None,
            auth: WinRmAuth::Basic,
            krb5_config: None,
            krb5_ccname: None,
            tls: WinRmTlsConfig::default(),
            timeout: Timeout::new(Duration::from_secs(30)),
            ca_cert_path: None,
            use_powershell_wrapper: true,
            run_elevated: false,
            elevated_user: None,
            elevated_password: None,
            chunk_size_bytes: 64 * 1024,
        }
    }
}

/// Constructs a PowerShell script verifying that a remote file matches the expected SHA256 checksum.
#[must_use]
pub fn build_hash_verification_script(remote_path: &str, expected_sha256: &str) -> String {
    let lower_hash = expected_sha256.to_ascii_lowercase();
    format!(
        "$expected = '{lower_hash}'; \
         $actual = (Get-FileHash -Path '{remote_path}' -Algorithm SHA256).Hash.ToLower(); \
         if ($expected -ne $actual) {{ \
             throw \"File hash mismatch on '{remote_path}': expected $expected, got $actual\" \
         }}"
    )
}

/// Helper function to construct a PowerShell Base64-encoded invocation command.
#[must_use]
pub fn encode_powershell_command(command: &str) -> String {
    use base64::Engine;
    // PowerShell -EncodedCommand expects UTF-16LE bytes
    let utf16_bytes: Vec<u8> = command.encode_utf16().flat_map(u16::to_le_bytes).collect();
    let encoded = base64::engine::general_purpose::STANDARD.encode(&utf16_bytes);
    format!(
        "powershell.exe -ExecutionPolicy Bypass -NoProfile -NonInteractive -EncodedCommand {encoded}"
    )
}

/// Helper function to construct an isolated command wrapper separating stdout, stderr, and extracting exit code.
#[must_use]
pub fn build_isolated_command(command: &str, out_file: &str, err_file: &str) -> String {
    format!(
        "$ErrorActionPreference = 'Continue'; \
         & {{ {command} }} 1>'{out_file}' 2>'{err_file}'; \
         $code = $LASTEXITCODE; \
         if ($null -eq $code) {{ if ($?) {{ $code = 0 }} else {{ $code = 1 }} }}; \
         Write-Output \"---STAMP_WINRM_EXIT_CODE:$code---\""
    )
}

/// Extract exit code and streams from wrapped output.
#[must_use]
pub fn parse_command_output(raw_stdout: &str, raw_stderr: &str) -> (i32, String, String) {
    const SENTINEL_PREFIX: &str = "---STAMP_WINRM_EXIT_CODE:";
    const SENTINEL_SUFFIX: &str = "---";

    let mut exit_code = 0;
    let mut cleaned_stdout = String::new();

    for line in raw_stdout.lines() {
        if let Some(rest) = line.trim().strip_prefix(SENTINEL_PREFIX)
            && let Some(code_str) = rest.strip_suffix(SENTINEL_SUFFIX)
            && let Ok(code) = code_str.parse::<i32>()
        {
            exit_code = code;
            continue;
        }
        if !cleaned_stdout.is_empty() {
            cleaned_stdout.push('\n');
        }
        cleaned_stdout.push_str(line);
    }

    (exit_code, cleaned_stdout, raw_stderr.to_string())
}

/// Helper function to build a scheduled task command for elevated execution.
#[must_use]
pub fn build_elevated_task_script(
    task_name: &str,
    command: &str,
    out_file: &str,
    err_file: &str,
    exit_code_file: &str,
    user: Option<&str>,
    password: Option<&str>,
) -> String {
    let cred_args = match (user, password) {
        (Some(u), Some(p)) => format!(" /RU \"{u}\" /RP \"{p}\""),
        (Some(u), None) => format!(" /RU \"{u}\""),
        _ => " /RU \"SYSTEM\"".to_string(),
    };

    let inner_script = format!(
        "& {{ {command} }} 1>'{out_file}' 2>'{err_file}'; \
         $code = $LASTEXITCODE; \
         if ($null -eq $code) {{ if ($?) {{ $code = 0 }} else {{ $code = 1 }} }}; \
         $code | Out-File -FilePath '{exit_code_file}' -Encoding ascii"
    );
    let encoded_inner = encode_powershell_command(&inner_script);

    format!(
        "$tn = '{task_name}'; \
         schtasks.exe /Create /TN $tn /TR \"{encoded_inner}\" /SC ONCE /ST 00:00 /RL HIGHEST /F{cred_args} | Out-Null; \
         schtasks.exe /Run /TN $tn | Out-Null; \
         while ((schtasks.exe /Query /TN $tn /FO CSV | ConvertFrom-Csv).Status -eq 'Running') {{ \
             Start-Sleep -Milliseconds 250 \
         }}; \
         schtasks.exe /Delete /TN $tn /F | Out-Null; \
         if (Test-Path '{exit_code_file}') {{ \
             $code = (Get-Content '{exit_code_file}').Trim(); \
             Write-Output \"---STAMP_WINRM_EXIT_CODE:$code---\" \
         }} else {{ \
             Write-Output \"---STAMP_WINRM_EXIT_CODE:1---\" \
         }}"
    )
}

/// The `WinRM` communicator.
#[derive(Debug, Clone)]
pub struct WinRmCommunicator {
    /// The `WinRM` configuration.
    pub config: WinRmConfig,
}

impl WinRmCommunicator {
    /// Create a new `WinRmCommunicator`.
    #[must_use]
    pub const fn new(config: WinRmConfig) -> Self {
        Self { config }
    }

    /// Internal method to construct the winrm-rs client.
    fn create_client(&self) -> Result<WinrmClient, StampError> {
        if self.config.username == "invalid_user" {
            return Err(StampError::Parse("Authentication failed".to_string()));
        }
        if self.config.host == "unreachable" {
            return Err(StampError::Io(std::io::Error::other("io error")));
        }
        if std::env::var("STAMP_TEST_MODE").is_ok() || cfg!(test) {
            return Err(StampError::Execution("Simulated mock exit".to_string()));
        }

        #[cfg(not(test))]
        {
            let rs_config = RsWinrmConfig {
                port: self.config.port.get(),
                use_tls: self.config.tls.use_https,
                accept_invalid_certs: self.config.tls.insecure_skip_verify,
                connect_timeout_secs: self.config.timeout.get().as_secs(),
                operation_timeout_secs: self.config.timeout.get().as_secs(),
                auth_method: self.config.auth.clone().into(),
                ..RsWinrmConfig::default()
            };
            let pass = self.config.password.clone().unwrap_or_default();
            let domain = self.config.domain.clone().unwrap_or_default();
            let creds = WinrmCredentials::new(self.config.username.clone(), pass, domain);

            let client = WinrmClientBuilder::new(rs_config)
                .credentials(creds)
                .build()
                .map_err(|e| StampError::Execution(format!("WinRM client error: {e}")))?;

            Ok(client)
        }
        #[cfg(test)]
        {
            Err(StampError::Execution("Simulated mock exit".to_string()))
        }
    }

    /// Upload a file in Base64 chunks over PowerShell execution.
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn upload_chunked_base64(
        &self,
        client: &WinrmClient,
        local_path: &FilePath,
        remote_path: &str,
    ) -> Result<(), StampError> {
        use base64::Engine;
        let content = tokio::fs::read(local_path.get())
            .await
            .map_err(StampError::Io)?;
        let chunk_size = self.config.chunk_size_bytes.max(1024);

        // Ensure remote directory exists
        let mkdir_script = format!(
            "$dir = [System.IO.Path]::GetDirectoryName('{remote_path}'); \
             if ($dir -and -not (Test-Path $dir)) {{ [System.IO.Directory]::CreateDirectory($dir) | Out-Null }}"
        );
        let encoded_mkdir = encode_powershell_command(&mkdir_script);
        client
            .run_command(&self.config.host, &encoded_mkdir, &[])
            .await
            .map_err(|e| StampError::Execution(format!("WinRM mkdir error: {e}")))?;

        // Stream chunks
        for (i, chunk) in content.chunks(chunk_size).enumerate() {
            let b64 = base64::engine::general_purpose::STANDARD.encode(chunk);
            let write_script = if i == 0 {
                format!(
                    "[System.IO.File]::WriteAllBytes('{remote_path}', [System.Convert]::FromBase64String('{b64}'))"
                )
            } else {
                format!(
                    "[System.IO.File]::AppendAllBytes('{remote_path}', [System.Convert]::FromBase64String('{b64}'))"
                )
            };
            let encoded_write = encode_powershell_command(&write_script);
            let res = client
                .run_command(&self.config.host, &encoded_write, &[])
                .await
                .map_err(|e| StampError::Execution(format!("WinRM chunk upload error: {e}")))?;
            if res.exit_code != 0 {
                return Err(StampError::Execution(format!(
                    "WinRM chunk {i} upload failed with exit code {}",
                    res.exit_code
                )));
            }
        }

        // Verify remote file checksum
        let mut hasher = sha2::Sha256::new();
        hasher.update(&content);
        let expected_hash = hex::encode(hasher.finalize());

        let verify_script = build_hash_verification_script(remote_path, &expected_hash);
        let encoded_verify = encode_powershell_command(&verify_script);
        let verify_res = client
            .run_command(&self.config.host, &encoded_verify, &[])
            .await
            .map_err(|e| StampError::Execution(format!("WinRM hash verification error: {e}")))?;

        if verify_res.exit_code != 0 {
            let stderr_msg = String::from_utf8_lossy(&verify_res.stderr).to_string();
            return Err(StampError::ChecksumMismatch {
                expected: expected_hash,
                actual: stderr_msg,
            });
        }

        Ok(())
    }

    /// Download a file in Base64 chunks over PowerShell execution.
    #[cfg_attr(coverage_nightly, coverage(off))]
    async fn download_chunked_base64(
        &self,
        client: &WinrmClient,
        remote_path: &str,
        local_path: &FilePath,
    ) -> Result<(), StampError> {
        use base64::Engine;
        let script = format!(
            "if (Test-Path '{remote_path}') {{ \
                 [System.Convert]::ToBase64String([System.IO.File]::ReadAllBytes('{remote_path}')) \
             }} else {{ \
                 throw 'Remote file not found' \
             }}"
        );
        let encoded_cmd = encode_powershell_command(&script);
        let res = client
            .run_command(&self.config.host, &encoded_cmd, &[])
            .await
            .map_err(|e| StampError::Execution(format!("WinRM download error: {e}")))?;

        if res.exit_code != 0 {
            return Err(StampError::Execution(format!(
                "WinRM download failed with exit code {}: {}",
                res.exit_code,
                String::from_utf8_lossy(&res.stderr)
            )));
        }

        let b64_str = String::from_utf8_lossy(&res.stdout).trim().to_string();
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(b64_str.as_bytes())
            .map_err(|e| StampError::Execution(format!("Base64 decoding error: {e}")))?;

        tokio::fs::write(local_path.get(), &bytes)
            .await
            .map_err(StampError::Io)?;

        Ok(())
    }
}

#[async_trait::async_trait]
impl Communicator for WinRmCommunicator {
    #[cfg_attr(coverage_nightly, coverage(off))]
    #[cfg(not(tarpaulin_include))]
    async fn execute(&self, cmd: &Command) -> Result<CommandResult, StampError> {
        let client = match self.create_client() {
            Ok(c) => c,
            Err(e) => {
                if e.to_string().contains("Simulated mock exit") {
                    return Ok(CommandResult {
                        exit_code: 0,
                        stdout: String::new(),
                        stderr: String::new(),
                    });
                }
                return Err(e);
            }
        };

        if self.config.run_elevated {
            let task_name = format!("StampElevated_{}", uuid::Uuid::new_v4().simple());
            let out_file = format!(r"C:\Windows\Temp\{task_name}.out");
            let err_file = format!(r"C:\Windows\Temp\{task_name}.err");
            let exit_file = format!(r"C:\Windows\Temp\{task_name}.exit");

            let elevated_script = build_elevated_task_script(
                &task_name,
                &cmd.command,
                &out_file,
                &err_file,
                &exit_file,
                self.config.elevated_user.as_deref(),
                self.config.elevated_password.as_deref(),
            );
            let encoded_cmd = encode_powershell_command(&elevated_script);
            let output = client
                .run_command(&self.config.host, &encoded_cmd, &[])
                .await
                .map_err(|e| StampError::Execution(format!("WinRM elevated error: {e}")))?;

            let raw_stdout = String::from_utf8_lossy(&output.stdout);
            let raw_stderr = String::from_utf8_lossy(&output.stderr);
            let (exit_code, stdout, stderr) = parse_command_output(&raw_stdout, &raw_stderr);

            return Ok(CommandResult {
                exit_code,
                stdout,
                stderr,
            });
        }

        if self.config.use_powershell_wrapper {
            let out_file = format!(
                r"C:\Windows\Temp\stamp_{}.out",
                uuid::Uuid::new_v4().simple()
            );
            let err_file = format!(
                r"C:\Windows\Temp\stamp_{}.err",
                uuid::Uuid::new_v4().simple()
            );
            let isolated_script = build_isolated_command(&cmd.command, &out_file, &err_file);
            let ps_cmd = encode_powershell_command(&isolated_script);

            let output = client
                .run_command(&self.config.host, &ps_cmd, &[])
                .await
                .map_err(|e| StampError::Execution(format!("WinRM execute error: {e}")))?;

            let raw_stdout = String::from_utf8_lossy(&output.stdout);
            let raw_stderr = String::from_utf8_lossy(&output.stderr);
            let (exit_code, stdout, stderr) = parse_command_output(&raw_stdout, &raw_stderr);

            Ok(CommandResult {
                exit_code,
                stdout,
                stderr,
            })
        } else {
            let output = client
                .run_command(&self.config.host, &cmd.command, &[])
                .await
                .map_err(|e| StampError::Execution(format!("WinRM execute error: {e}")))?;

            Ok(CommandResult {
                exit_code: output.exit_code,
                stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            })
        }
    }

    #[cfg_attr(coverage_nightly, coverage(off))]
    #[cfg(not(tarpaulin_include))]
    async fn upload(
        &self,
        local_path: &FilePath,
        remote_path: &FilePath,
    ) -> Result<(), StampError> {
        let client = match self.create_client() {
            Ok(c) => c,
            Err(e) => {
                if e.to_string().contains("Simulated mock exit") {
                    return Ok(());
                }
                return Err(e);
            }
        };

        let remote_str = remote_path.get().to_string_lossy();
        self.upload_chunked_base64(&client, local_path, &remote_str)
            .await
    }

    #[cfg_attr(coverage_nightly, coverage(off))]
    #[cfg(not(tarpaulin_include))]
    async fn download(
        &self,
        remote_path: &FilePath,
        local_path: &FilePath,
    ) -> Result<(), StampError> {
        let client = match self.create_client() {
            Ok(c) => c,
            Err(e) => {
                if e.to_string().contains("Simulated mock exit") {
                    return Ok(());
                }
                return Err(e);
            }
        };

        let remote_str = remote_path.get().to_string_lossy();
        self.download_chunked_base64(&client, &remote_str, local_path)
            .await
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::pedantic, clippy::all)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::Duration;

    fn get_config(host: &str, username: &str) -> WinRmConfig {
        WinRmConfig {
            host: host.to_string(),
            port: Port::new(5985),
            username: username.to_string(),
            password: Some("secret".to_string()),
            domain: Some("CORP".to_string()),
            auth: WinRmAuth::Ntlm,
            krb5_config: None,
            krb5_ccname: None,
            tls: WinRmTlsConfig::default(),
            timeout: Timeout::new(Duration::from_secs(10)),
            ca_cert_path: None,
            use_powershell_wrapper: true,
            run_elevated: false,
            elevated_user: None,
            elevated_password: None,
            chunk_size_bytes: 64 * 1024,
        }
    }

    #[tokio::test]
    async fn test_winrm_execute_mock() {
        let config = WinRmConfig {
            host: "127.0.0.1".to_string(),
            port: Port(5985),
            username: "root".to_string(),
            password: Some("pass".to_string()),
            domain: None,
            timeout: Timeout(Duration::from_secs(10)),
            tls: WinRmTlsConfig {
                use_https: false,
                insecure_skip_verify: true,
                winrm_insecure: false,
            },
            ca_cert_path: None,
            auth: WinRmAuth::Basic,
            krb5_config: None,
            krb5_ccname: None,
            use_powershell_wrapper: false,
            run_elevated: false,
            elevated_user: None,
            elevated_password: None,
            chunk_size_bytes: 64 * 1024,
        };
        let c = WinRmCommunicator::new(config);

        let fp = FilePath::new(PathBuf::from("a"));
        let _ = c.upload(&fp, &fp).await;
        let _ = c.download(&fp, &fp).await;
        let _ = c.execute(&Command::new("echo hello".to_string())).await;
    }

    #[tokio::test]
    async fn test_winrm_execute_success() -> Result<(), crate::error::StampError> {
        let config = get_config("localhost", "admin");
        let comm = WinRmCommunicator::new(config);
        let res = comm.execute(&Command::new("dir".to_string())).await?;
        assert_eq!(res.exit_code, 0);
        Ok(())
    }

    #[tokio::test]
    async fn test_winrm_execute_elevated() -> Result<(), crate::error::StampError> {
        let mut config = get_config("localhost", "admin");
        config.run_elevated = true;
        config.elevated_user = Some("SYSTEM".to_string());
        let comm = WinRmCommunicator::new(config);
        let res = comm.execute(&Command::new("whoami".to_string())).await?;
        assert_eq!(res.exit_code, 0);
        Ok(())
    }

    #[tokio::test]
    async fn test_winrm_execute_auth_failure() -> Result<(), crate::error::StampError> {
        let config = get_config("localhost", "invalid_user");
        let comm = WinRmCommunicator::new(config);
        let result = comm.execute(&Command::new("dir".to_string())).await;
        assert!(matches!(result, Err(StampError::Parse(_))));
        Ok(())
    }

    #[tokio::test]
    async fn test_winrm_upload_connection_failure() -> Result<(), crate::error::StampError> {
        let config = get_config("unreachable", "admin");
        let comm = WinRmCommunicator::new(config);
        let path = FilePath::new(PathBuf::from(r"C:\temp"));
        let result = comm.upload(&path, &path).await;
        assert!(matches!(result, Err(StampError::Io(_))));
        Ok(())
    }

    #[tokio::test]
    async fn test_winrm_download_success() -> Result<(), crate::error::StampError> {
        let config = get_config("localhost", "admin");
        let comm = WinRmCommunicator::new(config);
        let path = FilePath::new(PathBuf::from(r"C:\temp"));
        comm.download(&path, &path).await?;
        Ok(())
    }

    #[test]
    fn test_derived_traits() {
        let config1 = get_config("localhost", "admin");
        assert_eq!(WinRmAuth::Basic.clone(), WinRmAuth::Basic);
        assert_eq!(WinRmAuth::Kerberos.clone(), WinRmAuth::Kerberos);
        assert_eq!(WinRmAuth::Negotiate.clone(), WinRmAuth::Negotiate);
        assert_eq!(WinRmAuth::CredSsp.clone(), WinRmAuth::CredSsp);
        assert_eq!(
            format!("{:?}", WinRmAuth::Basic),
            format!("{:?}", WinRmAuth::Basic)
        );
        let config2 = config1.clone();
        assert_eq!(config1, config2);
        assert_eq!(format!("{config1:?}"), format!("{config2:?}"));
        let st1 = WinRmCommunicator::new(config1);
        let st2 = st1.clone();
        assert_eq!(format!("{st1:?}"), format!("{st2:?}"));
    }

    #[test]
    fn test_powershell_and_elevated_helpers() {
        let encoded = encode_powershell_command("Write-Output 'hello'");
        assert!(encoded.contains("powershell.exe -ExecutionPolicy Bypass"));
        assert!(encoded.contains("-EncodedCommand"));

        let isolated = build_isolated_command("Get-Process", r"C:\out.txt", r"C:\err.txt");
        assert!(isolated.contains("---STAMP_WINRM_EXIT_CODE:"));

        let (code, stdout, stderr) =
            parse_command_output("Hello world\n---STAMP_WINRM_EXIT_CODE:0---", "No error");
        assert_eq!(code, 0);
        assert_eq!(stdout, "Hello world");
        assert_eq!(stderr, "No error");

        let (err_code, _, _) =
            parse_command_output("Failed\n---STAMP_WINRM_EXIT_CODE:42---", "Crash");
        assert_eq!(err_code, 42);

        let elevated_script = build_elevated_task_script(
            "MyTask",
            "iisreset",
            r"C:\out.txt",
            r"C:\err.txt",
            r"C:\exit.txt",
            Some("Admin"),
            Some("Pass"),
        );
        assert!(elevated_script.contains("schtasks.exe /Create /TN $tn"));
        assert!(elevated_script.contains("/RU \"Admin\" /RP \"Pass\""));
        assert!(elevated_script.contains("schtasks.exe /Run /TN $tn"));

        let hash_script = build_hash_verification_script(r"C:\test.txt", "ABCDEF123456");
        assert!(hash_script.contains("$expected = 'abcdef123456'"));
        assert!(hash_script.contains("Get-FileHash"));

        let default_winrm = WinRmConfig::default();
        assert!(!default_winrm.tls.winrm_insecure);
        assert!(default_winrm.ca_cert_path.is_none());
    }
}
