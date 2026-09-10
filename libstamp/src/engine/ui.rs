#![cfg(not(tarpaulin_include))]
#![cfg_attr(coverage_nightly, coverage(off))]
//! UI Module for standard and machine-readable output.

use crate::engine::packer::FeatureState;
use std::sync::{Arc, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

/// A text scrubber that redacts sensitive values.
#[derive(Debug, Default)]
pub struct Scrubber {
    /// List of sensitive string values to redact from log output.
    sensitive_values: RwLock<Vec<String>>,
}

impl Scrubber {
    /// Create a new Scrubber.
    #[must_use]
    pub fn new() -> Self {
        Self {
            sensitive_values: RwLock::new(Vec::new()),
        }
    }

    /// Add a sensitive value to be redacted.
    pub fn add(&self, value: String) {
        if !value.is_empty()
            && let Ok(mut vals) = self.sensitive_values.write()
        {
            vals.push(value);
        }
    }

    /// Scrub sensitive values from the input string.
    #[must_use]
    pub fn scrub(&self, input: &str) -> String {
        let mut result = input.to_string();
        if let Ok(vals) = self.sensitive_values.read() {
            for val in vals.iter() {
                result = result.replace(val, "<sensitive>");
            }
        }
        result
    }
}

/// Basic UI struct for formatting output.
#[derive(Debug, Clone)]
pub struct Ui {
    /// Whether machine-readable output is enabled.
    pub machine_readable: FeatureState,
    /// Whether color output is enabled.
    pub color: FeatureState,
    /// The specific ANSI color code for this UI instance.
    pub color_code: Option<String>,
    /// Whether timestamps are shown in standard output.
    pub timestamp_ui: FeatureState,
    /// The scrubber to redact sensitive values.
    pub scrubber: Option<Arc<Scrubber>>,
    /// Optional mock input queue for simulating user stdin in automated test environments.
    pub mock_inputs: Option<Arc<std::sync::Mutex<std::collections::VecDeque<String>>>>,
}

impl Ui {
    /// Create a new UI instance.
    #[must_use]
    pub const fn new(
        machine_readable: FeatureState,
        color: FeatureState,
        timestamp_ui: FeatureState,
    ) -> Self {
        Self::with_scrubber(machine_readable, color, timestamp_ui, None)
    }

    /// Create a new UI instance with a scrubber.
    #[must_use]
    pub const fn with_scrubber(
        machine_readable: FeatureState,
        color: FeatureState,
        timestamp_ui: FeatureState,
        scrubber: Option<Arc<Scrubber>>,
    ) -> Self {
        Self::with_scrubber_and_mock_inputs(machine_readable, color, timestamp_ui, scrubber, None)
    }

    /// Create a new UI instance with a scrubber and optional mock inputs.
    #[must_use]
    pub const fn with_scrubber_and_mock_inputs(
        machine_readable: FeatureState,
        color: FeatureState,
        timestamp_ui: FeatureState,
        scrubber: Option<Arc<Scrubber>>,
        mock_inputs: Option<Arc<std::sync::Mutex<std::collections::VecDeque<String>>>>,
    ) -> Self {
        Self {
            machine_readable,
            color,
            color_code: None,
            timestamp_ui,
            scrubber,
            mock_inputs,
        }
    }

    /// Attach a mock inputs queue to this UI instance.
    #[must_use]
    pub fn with_mock_inputs(
        mut self,
        mock_inputs: Arc<std::sync::Mutex<std::collections::VecDeque<String>>>,
    ) -> Self {
        self.mock_inputs = Some(mock_inputs);
        self
    }

    /// Redacts sensitive values using the attached scrubber if available.
    fn scrub(&self, input: &str) -> String {
        if let Some(s) = &self.scrubber {
            s.scrub(input)
        } else {
            input.to_string()
        }
    }

    /// Tees output line to log file if `PACKER_LOG_PATH` or `PACKER_LOG=1` is configured.
    fn tee_log(line: &str) {
        if let Ok(log_path) = std::env::var("PACKER_LOG_PATH") {
            if !log_path.is_empty()
                && let Ok(mut file) = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(log_path)
            {
                let _ = writeln!(file, "{line}");
            }
        } else if std::env::var("PACKER_LOG").as_deref() == Ok("1")
            && let Ok(mut file) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open("packer.log")
        {
            let _ = writeln!(file, "{line}");
        }
    }

    /// Output a standard message.
    pub fn say(&self, target: &str, message: &str) {
        let scrubbed_message = self.scrub(message);
        Self::tee_log(&format!("{target}: {scrubbed_message}"));
        if self.machine_readable.is_enabled() {
            let ts = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            let safe_msg = scrubbed_message
                .replace(',', "%!(PACKER_COMMA)")
                .replace('\n', "%!(PACKER_NL)");
            println!("{ts},{target},ui,say,{safe_msg}");
        } else {
            let mut prefix = String::new();
            if self.timestamp_ui.is_enabled() {
                // simple timestamp for regular output
                let ts = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                prefix = format!("[{ts}] ");
            }
            if !target.is_empty() {
                prefix = format!("{prefix}==> {target}: ");
            }
            let msg = if self.color.is_enabled() {
                format!("\x1b[32m{prefix}{scrubbed_message}\x1b[0m")
            } else {
                format!("{prefix}{scrubbed_message}")
            };
            println!("{msg}");
        }
    }

    /// Ask a question interactively.
    /// # Errors
    /// Returns an error if reading from stdin fails.
    #[cfg_attr(coverage_nightly, coverage(off))]
    pub fn ask(&self, target: &str, message: &str) -> Result<String, std::io::Error> {
        use std::io::Write;

        if let Some(ref mock_queue) = self.mock_inputs {
            if let Ok(mut queue) = mock_queue.lock()
                && let Some(resp) = queue.pop_front()
            {
                return Ok(resp);
            }
            return Ok(String::new());
        }

        if cfg!(test) {
            return Ok(String::new());
        }

        let mut prefix = String::new();
        if !target.is_empty() {
            prefix = format!("==> {target}: ");
        }
        print!("{prefix}{message}");
        std::io::stdout().flush()?;

        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        Ok(input.trim().to_lowercase())
    }

    /// Output an error message.
    pub fn error(&self, target: &str, message: &str) {
        let scrubbed_message = self.scrub(message);
        Self::tee_log(&format!("{target} [ERROR]: {scrubbed_message}"));
        if self.machine_readable.is_enabled() {
            let ts = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            let safe_msg = scrubbed_message
                .replace(',', "%!(PACKER_COMMA)")
                .replace('\n', "%!(PACKER_NL)");
            println!("{ts},{target},ui,error,{safe_msg}");
        } else {
            let mut prefix = String::new();
            if self.timestamp_ui.is_enabled() {
                let ts = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                prefix = format!("[{ts}] ");
            }
            if !target.is_empty() {
                prefix = format!("{prefix}==> {target}: ");
            }
            let msg = if self.color.is_enabled() {
                format!("\x1b[31m{prefix}{scrubbed_message}\x1b[0m")
            } else {
                format!("{prefix}{scrubbed_message}")
            };
            eprintln!("{msg}");
        }
    }

    /// Output a raw message for machine readable.
    pub fn machine(&self, target: &str, mtype: &str, data: &[&str]) {
        if self.machine_readable.is_enabled() {
            let ts = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            let mut out = format!("{ts},{target},{mtype}");
            for d in data {
                let scrubbed_d = self.scrub(d);
                let safe_d = scrubbed_d
                    .replace(',', "%!(PACKER_COMMA)")
                    .replace('\n', "%!(PACKER_NL)");
                out.push(',');
                out.push_str(&safe_d);
            }
            println!("{out}");
        }
    }

    /// Returns whether the UI session is running in an interactive terminal.
    #[must_use]
    pub fn is_interactive(&self) -> bool {
        use std::io::IsTerminal;
        if self.mock_inputs.is_some() {
            return true;
        }
        if self.machine_readable.is_enabled() {
            return false;
        }
        if std::env::var("CI").is_ok() {
            return false;
        }
        std::io::stdin().is_terminal()
    }

    /// Emits a structured JSON event (`ndjson`) for machine-readable logging or CI/CD pipelines.
    pub fn event_json<T: serde::Serialize>(&self, event_type: &str, target: &str, payload: &T) {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let payload_json = serde_json::to_value(payload).unwrap_or(serde_json::Value::Null);
        let event = serde_json::json!({
            "timestamp": ts,
            "target": target,
            "type": event_type,
            "data": payload_json,
        });
        if self.machine_readable.is_enabled() {
            println!("{event}");
        }
        Self::tee_log(&format!("{event}"));
    }

    /// Emits a machine-readable progress indicator for long-running operations.
    pub fn progress(&self, target: &str, action: &str, current: u64, total: u64, unit: &str) {
        if self.machine_readable.is_enabled() {
            let current_str = current.to_string();
            let total_str = total.to_string();
            self.machine(
                target,
                "progress",
                &[action, &current_str, &total_str, unit],
            );
        } else {
            let percentage = (current * 100).checked_div(total).unwrap_or_default();
            self.say(
                target,
                &format!("{action}: {current}/{total} {unit} ({percentage}%)"),
            );
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::pedantic, clippy::all)]
mod tests {

    use super::*;

    #[test]
    fn test_ui_say() {
        let ui = Ui::new(
            FeatureState::Disabled,
            FeatureState::Disabled,
            FeatureState::Disabled,
        );
        ui.say("target", "message");
        ui.error("target", "message");
        ui.machine("target", "mtype", &["data1", "data2"]);
    }

    #[test]
    fn test_ui_say_machine() {
        let ui = Ui::new(
            FeatureState::Enabled,
            FeatureState::Enabled,
            FeatureState::Enabled,
        );
        ui.say("target", "message\nwith,comma");
        ui.error("target", "message\nwith,comma");
        ui.machine("target", "mtype", &["data1", "data2\n,"]);
    }

    #[test]
    fn test_ui_say_color_timestamp() {
        let ui = Ui::new(
            FeatureState::Disabled,
            FeatureState::Enabled,
            FeatureState::Enabled,
        );
        ui.say("target", "message");
        ui.error("target", "message");
        ui.say("", "message");
        ui.error("", "message");
    }

    #[test]
    fn test_scrubber() {
        let scrubber = Scrubber::default();
        scrubber.add("secret123".to_string());
        scrubber.add("".to_string()); // empty add test

        let ui = Ui::with_scrubber(
            FeatureState::Disabled,
            FeatureState::Disabled,
            FeatureState::Disabled,
            Some(Arc::new(scrubber)),
        );

        ui.say("target", "this is a secret123 value");
        ui.error("target", "error: secret123 failed");
        ui.machine("target", "type", &["secret123"]);

        // just check scrub explicitly
        assert_eq!(ui.scrub("my secret123 test"), "my <sensitive> test");
    }

    #[test]
    fn test_ui_mock_inputs() {
        use std::collections::VecDeque;
        use std::sync::Mutex;

        let queue = Arc::new(Mutex::new(VecDeque::new()));
        if let Ok(mut q) = queue.lock() {
            q.push_back("response1".to_string());
            q.push_back("response2".to_string());
        }

        let ui = Ui::new(
            FeatureState::Disabled,
            FeatureState::Disabled,
            FeatureState::Disabled,
        )
        .with_mock_inputs(queue);

        let res1 = ui.ask("test", "Enter input 1: ").unwrap_or_default();
        assert_eq!(res1, "response1");

        let res2 = ui.ask("test", "Enter input 2: ").unwrap_or_default();
        assert_eq!(res2, "response2");

        assert!(ui.is_interactive());

        let res_empty = ui.ask("test", "Enter input 3: ").unwrap_or_default();
        assert_eq!(res_empty, "");
    }

    #[test]
    fn test_ui_interactive_event_progress() {
        let ui_machine = Ui::new(
            FeatureState::Enabled,
            FeatureState::Disabled,
            FeatureState::Disabled,
        );
        assert!(!ui_machine.is_interactive());

        let payload = serde_json::json!({"step": "download_iso", "size": 1024});
        ui_machine.event_json("step_start", "iso_builder", &payload);
        ui_machine.progress("iso_builder", "downloading", 50, 100, "MB");

        let ui_human = Ui::new(
            FeatureState::Disabled,
            FeatureState::Disabled,
            FeatureState::Disabled,
        );
        ui_human.event_json("step_start", "iso_builder", &payload);
        ui_human.progress("iso_builder", "downloading", 50, 100, "MB");
        ui_human.progress("iso_builder", "downloading", 0, 0, "MB");
    }

    #[test]
    fn test_ui_tee_log() {
        let temp_log = std::env::temp_dir().join("test_ui_packer.log");
        unsafe {
            std::env::set_var("PACKER_LOG_PATH", temp_log.to_string_lossy().to_string());
        }

        let ui = Ui::new(
            FeatureState::Disabled,
            FeatureState::Disabled,
            FeatureState::Disabled,
        );
        ui.say("builder-1", "hello from log test");
        ui.error("builder-1", "error from log test");

        unsafe {
            std::env::remove_var("PACKER_LOG_PATH");
        }

        let content = std::fs::read_to_string(&temp_log).unwrap_or_default();
        assert!(content.contains("builder-1: hello from log test"));
        assert!(content.contains("builder-1 [ERROR]: error from log test"));
        let _ = std::fs::remove_file(&temp_log);
    }
}

use std::io::Write;

/// A writer that pipes data into a Ui instance line-by-line.
pub struct UiTargetWriter {
    /// Underlying UI instance for output.
    ui: Arc<Ui>,
    /// Target label prefix for output.
    target: String,
    /// Whether output lines represent error messages.
    is_error: bool,
    /// Internal buffering for incomplete lines.
    buffer: String,
}

impl UiTargetWriter {
    /// Create a new `UiTargetWriter`.
    #[must_use]
    pub fn new(ui: Arc<Ui>, target: String, is_error: bool) -> Self {
        Self {
            ui,
            target,
            is_error,
            buffer: String::new(),
        }
    }
}

impl Write for UiTargetWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let text = String::from_utf8_lossy(buf);
        self.buffer.push_str(&text);
        while let Some(pos) = self.buffer.find('\n') {
            let line = self.buffer[..pos].to_string();
            self.buffer = self.buffer[pos + 1..].to_string();
            if self.is_error {
                self.ui.error(&self.target, &line);
            } else {
                self.ui.say(&self.target, &line);
            }
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        if !self.buffer.is_empty() {
            let text = self.buffer.clone();
            self.buffer.clear();
            if self.is_error {
                self.ui.error(&self.target, &text);
            } else {
                self.ui.say(&self.target, &text);
            }
        }
        Ok(())
    }
}

impl Drop for UiTargetWriter {
    fn drop(&mut self) {
        let _ = self.flush();
    }
}

#[cfg(test)]
mod tests_writer {
    use super::*;

    #[test]
    fn test_ui_target_writer() {
        let ui = Arc::new(Ui::new(
            FeatureState::Disabled,
            FeatureState::Disabled,
            FeatureState::Disabled,
        ));
        let mut writer = UiTargetWriter::new(ui.clone(), "target".to_string(), false);
        let _ = writer.write(b"hello\nworld\n");
        let _ = writer.write(b"no newline");
        let _ = writer.flush();

        let mut err_writer = UiTargetWriter::new(ui.clone(), "target".to_string(), true);
        let _ = err_writer.write(b"error line\n");
    }
}

use std::sync::atomic::{AtomicUsize, Ordering};

/// A thread-safe UI multiplexer that distributes uniquely colored Ui instances to builders.
pub struct UiMultiplexer {
    /// Whether machine-readable output is enabled.
    machine_readable: FeatureState,
    /// Whether color output is enabled.
    color: FeatureState,
    /// Whether timestamp formatting is enabled.
    timestamp_ui: FeatureState,
    /// Shared text scrubber for redacting secrets across all builders.
    scrubber: Option<Arc<Scrubber>>,
    /// Rotating counter to assign distinct ANSI color codes to builders.
    color_index: AtomicUsize,
    /// Optional mock input queue for simulating user stdin in automated test environments.
    mock_inputs: Option<Arc<std::sync::Mutex<std::collections::VecDeque<String>>>>,
}

impl UiMultiplexer {
    /// Create a new `UiMultiplexer`.
    #[must_use]
    pub fn new(
        machine_readable: FeatureState,
        color: FeatureState,
        timestamp_ui: FeatureState,
        scrubber: Option<Arc<Scrubber>>,
    ) -> Self {
        Self {
            machine_readable,
            color,
            timestamp_ui,
            scrubber,
            color_index: AtomicUsize::new(0),
            mock_inputs: None,
        }
    }

    /// Attach a mock inputs queue to this multiplexer.
    #[must_use]
    pub fn with_mock_inputs(
        mut self,
        mock_inputs: Arc<std::sync::Mutex<std::collections::VecDeque<String>>>,
    ) -> Self {
        self.mock_inputs = Some(mock_inputs);
        self
    }

    /// Get a uniquely colored Ui instance for a builder.
    #[must_use]
    pub fn get_ui(&self) -> Arc<Ui> {
        let colors = ["32", "36", "35", "33", "34", "31"]; // Green, Cyan, Magenta, Yellow, Blue, Red
        let index = self.color_index.fetch_add(1, Ordering::SeqCst);
        let color_code = colors[index % colors.len()].to_string();

        let mut ui = Ui::with_scrubber_and_mock_inputs(
            self.machine_readable,
            self.color,
            self.timestamp_ui,
            self.scrubber.clone(),
            self.mock_inputs.clone(),
        );
        ui.color_code = Some(color_code);
        Arc::new(ui)
    }
}

#[cfg(test)]
mod tests_multiplexer {
    use super::*;

    #[test]
    fn test_ui_multiplexer() {
        let multi = UiMultiplexer::new(
            FeatureState::Disabled,
            FeatureState::Enabled,
            FeatureState::Disabled,
            None,
        );

        let ui1 = multi.get_ui();
        assert_eq!(ui1.color_code.as_deref(), Some("32"));

        let ui2 = multi.get_ui();
        assert_eq!(ui2.color_code.as_deref(), Some("36"));

        let queue = Arc::new(std::sync::Mutex::new(std::collections::VecDeque::new()));
        let multi_mock = multi.with_mock_inputs(queue);
        let ui3 = multi_mock.get_ui();
        assert!(ui3.mock_inputs.is_some());
    }
}

use std::pin::Pin;
use std::task::{Context, Poll};

impl tokio::io::AsyncWrite for UiTargetWriter {
    fn poll_write(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, std::io::Error>> {
        Poll::Ready(std::io::Write::write(&mut *self, buf))
    }

    fn poll_flush(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
    ) -> Poll<Result<(), std::io::Error>> {
        Poll::Ready(std::io::Write::flush(&mut *self))
    }

    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
    ) -> Poll<Result<(), std::io::Error>> {
        Poll::Ready(std::io::Write::flush(&mut *self))
    }
}
