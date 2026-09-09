#![cfg_attr(coverage_nightly, coverage(off))]
//! Multistep execution engine.
//!
//! Provides a state machine for breaking down builder tasks into
//! discrete, state-tracked steps, allowing for graceful aborts and cleanup.

use std::any::Any;
use std::collections::HashMap;
use std::fmt::Debug;
use tokio::sync::watch;

/// An action to take after a step runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepAction {
    /// Continue to the next step.
    Continue,
    /// Halt execution and initiate cleanup.
    Halt,
}

/// A container for state shared between steps.
#[derive(Default)]
pub struct StateBag {
    /// Internal map storing arbitrary step data by string key.
    state: HashMap<String, Box<dyn Any + Send + Sync>>,
    /// Cancellation receiver for steps to check if they should abort early.
    cancel_rx: Option<watch::Receiver<bool>>,
}

impl StateBag {
    /// Creates a new, empty `StateBag`.
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: HashMap::new(),
            cancel_rx: None,
        }
    }

    /// Puts a value into the state bag.
    pub fn put<T: Any + Send + Sync>(&mut self, key: &str, value: T) {
        self.state.insert(key.to_string(), Box::new(value));
    }

    /// Gets a reference to a value from the state bag.
    #[must_use]
    pub fn get<T: Any + Send + Sync>(&self, key: &str) -> Option<&T> {
        self.state.get(key).and_then(|v| v.downcast_ref::<T>())
    }

    /// Gets a mutable reference to a value from the state bag.
    pub fn get_mut<T: Any + Send + Sync>(&mut self, key: &str) -> Option<&mut T> {
        self.state.get_mut(key).and_then(|v| v.downcast_mut::<T>())
    }

    /// Checks if the multistep runner has been cancelled.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.cancel_rx.as_ref().is_some_and(|rx| *rx.borrow())
    }

    /// Records an executed step name into the execution history.
    pub fn record_step(&mut self, step_name: &str) {
        if let Some(history) = self.get_mut::<Vec<String>>("step_execution_history") {
            history.push(step_name.to_string());
        } else {
            self.put("step_execution_history", vec![step_name.to_string()]);
        }
    }

    /// Returns the step execution history.
    #[must_use]
    pub fn execution_history(&self) -> Option<&Vec<String>> {
        self.get::<Vec<String>>("step_execution_history")
    }

    /// Gets the instance IP address if set in the state bag.
    #[must_use]
    pub fn instance_ip(&self) -> Option<&str> {
        self.get::<String>("instance_ip").map(String::as_str)
    }

    /// Sets the instance IP address in the state bag.
    pub fn set_instance_ip(&mut self, ip: String) {
        self.put("instance_ip", ip);
    }

    /// Gets the SSH port if set in the state bag.
    #[must_use]
    pub fn ssh_port(&self) -> Option<u16> {
        self.get::<u16>("ssh_port").copied()
    }

    /// Sets the SSH port in the state bag.
    pub fn set_ssh_port(&mut self, port: u16) {
        self.put("ssh_port", port);
    }

    /// Gets the generated artifact ID if set in the state bag.
    #[must_use]
    pub fn artifact_id(&self) -> Option<&str> {
        self.get::<String>("artifact_id").map(String::as_str)
    }

    /// Sets the generated artifact ID in the state bag.
    pub fn set_artifact_id(&mut self, id: String) {
        self.put("artifact_id", id);
    }

    /// Gets the strongly typed `BuildContext` if present in the state bag.
    #[must_use]
    pub fn build_context(&self) -> Option<&crate::engine::hook::BuildContext> {
        self.get::<crate::engine::hook::BuildContext>("build_context")
    }

    /// Sets the strongly typed `BuildContext` in the state bag.
    pub fn set_build_context(&mut self, ctx: crate::engine::hook::BuildContext) {
        self.put("build_context", ctx);
    }

    /// Gets the target host if set in the state bag.
    #[must_use]
    pub fn host(&self) -> Option<&str> {
        self.get::<String>("host").map(String::as_str)
    }

    /// Sets the target host in the state bag.
    pub fn set_host(&mut self, host: String) {
        self.put("host", host);
    }

    /// Gets the remote user if set in the state bag.
    #[must_use]
    pub fn user(&self) -> Option<&str> {
        self.get::<String>("user").map(String::as_str)
    }

    /// Sets the remote user in the state bag.
    pub fn set_user(&mut self, user: String) {
        self.put("user", user);
    }

    /// Gets the remote password if set in the state bag.
    #[must_use]
    pub fn password(&self) -> Option<&str> {
        self.get::<String>("password").map(String::as_str)
    }

    /// Sets the remote password in the state bag.
    pub fn set_password(&mut self, password: String) {
        self.put("password", password);
    }

    /// Gets the connection metadata dictionary if set in the state bag.
    #[must_use]
    pub fn conn_info(&self) -> Option<&HashMap<String, String>> {
        self.get::<HashMap<String, String>>("conn_info")
    }

    /// Sets the connection metadata dictionary in the state bag.
    pub fn set_conn_info(&mut self, conn_info: HashMap<String, String>) {
        self.put("conn_info", conn_info);
    }

    /// Gets the ephemeral SSH public key if set in the state bag.
    #[must_use]
    pub fn ssh_public_key(&self) -> Option<&str> {
        self.get::<String>("ssh_public_key").map(String::as_str)
    }

    /// Sets the ephemeral SSH public key in the state bag.
    pub fn set_ssh_public_key(&mut self, key: String) {
        self.put("ssh_public_key", key);
    }

    /// Gets the ephemeral SSH private key if set in the state bag.
    #[must_use]
    pub fn ssh_private_key(&self) -> Option<&str> {
        self.get::<String>("ssh_private_key").map(String::as_str)
    }

    /// Sets the ephemeral SSH private key in the state bag.
    pub fn set_ssh_private_key(&mut self, key: String) {
        self.put("ssh_private_key", key);
    }

    /// Gets the Packer run UUID if set in the state bag.
    #[must_use]
    pub fn packer_run_uuid(&self) -> Option<&str> {
        self.get::<String>("packer_run_uuid").map(String::as_str)
    }

    /// Sets the Packer run UUID in the state bag.
    pub fn set_packer_run_uuid(&mut self, uuid: String) {
        self.put("packer_run_uuid", uuid);
    }

    /// Gets the source AMI ID if set in the state bag.
    #[must_use]
    pub fn source_ami(&self) -> Option<&str> {
        self.get::<String>("source_ami").map(String::as_str)
    }

    /// Sets the source AMI ID in the state bag.
    pub fn set_source_ami(&mut self, ami: String) {
        self.put("source_ami", ami);
    }

    /// Gets the source AMI name if set in the state bag.
    #[must_use]
    pub fn source_ami_name(&self) -> Option<&str> {
        self.get::<String>("source_ami_name").map(String::as_str)
    }

    /// Sets the source AMI name in the state bag.
    pub fn set_source_ami_name(&mut self, name: String) {
        self.put("source_ami_name", name);
    }
}

/// A thread-safe wrapper around [`StateBag`] enabling concurrent reads and mutations across tasks.
#[derive(Clone, Default)]
pub struct SharedStateBag {
    /// Inner state bag protected by a read-write lock and shared pointer.
    inner: std::sync::Arc<std::sync::RwLock<StateBag>>,
}

impl SharedStateBag {
    /// Creates a new empty `SharedStateBag`.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: std::sync::Arc::new(std::sync::RwLock::new(StateBag::new())),
        }
    }

    /// Creates a `SharedStateBag` wrapping an existing `StateBag`.
    #[must_use]
    pub fn from_bag(bag: StateBag) -> Self {
        Self {
            inner: std::sync::Arc::new(std::sync::RwLock::new(bag)),
        }
    }

    /// Executes a closure with a read reference to the underlying `StateBag`.
    pub fn read<R, F: FnOnce(&StateBag) -> R>(&self, f: F) -> R {
        let guard = self
            .inner
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        f(&guard)
    }

    /// Executes a closure with a mutable reference to the underlying `StateBag`.
    pub fn write<R, F: FnOnce(&mut StateBag) -> R>(&self, f: F) -> R {
        let mut guard = self
            .inner
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        f(&mut guard)
    }
}

/// Trait for lifecycle hooks invoked during step execution.
#[async_trait::async_trait]
pub trait StepHook: Send + Sync {
    /// Hook invoked before a step starts running.
    async fn pre_step(
        &self,
        _step_name: &str,
        _state: &mut StateBag,
    ) -> Result<(), crate::error::StampError> {
        Ok(())
    }

    /// Hook invoked after a step succeeds.
    async fn post_step(
        &self,
        _step_name: &str,
        _state: &mut StateBag,
    ) -> Result<(), crate::error::StampError> {
        Ok(())
    }

    /// Hook invoked when a step fails.
    async fn step_error(
        &self,
        _step_name: &str,
        _error: &crate::error::StampError,
        _state: &mut StateBag,
    ) {
    }
}

/// A single step in a multistep process.
#[async_trait::async_trait]
pub trait Step: Send + Sync {
    /// Returns the descriptive name of the step.
    fn name(&self) -> String {
        std::any::type_name::<Self>().to_string()
    }

    /// Runs the step.
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, crate::error::StampError>;

    /// Attempts to recover from a failure during step execution.
    ///
    /// Returns `Ok(true)` if recovery was successful and pipeline execution should continue,
    /// or `Ok(false)` if recovery is not possible.
    ///
    /// # Errors
    /// Returns `StampError` if recovery encounters an unrecoverable failure.
    async fn recover(&mut self, _state: &mut StateBag) -> Result<bool, crate::error::StampError> {
        Ok(false)
    }

    /// Cleans up any resources allocated by the step.
    /// This is called in reverse order for all steps that have been run,
    /// regardless of whether the run succeeded or failed.
    async fn cleanup(&mut self, state: &StateBag);
}

/// A runner for executing a sequence of steps.
pub struct Runner {
    /// Steps to be executed sequentially.
    steps: Vec<Box<dyn Step>>,
    /// Cancellation sender.
    cancel_tx: watch::Sender<bool>,
    /// Cancellation receiver.
    cancel_rx: watch::Receiver<bool>,
    /// Indices of executed steps.
    executed_steps: Vec<usize>,
    /// Whether debug pause mode is enabled.
    debug: bool,
    /// Strategy to employ when a step returns an error.
    on_error: crate::engine::packer::OnErrorStrategy,
    /// Optional UI handle for prompts and messages.
    ui: Option<std::sync::Arc<crate::engine::ui::Ui>>,
    /// Step lifecycle hooks.
    step_hooks: Vec<std::sync::Arc<dyn StepHook>>,
}

impl Runner {
    /// Creates a new runner with the given steps.
    #[must_use]
    pub fn new(steps: Vec<Box<dyn Step>>) -> Self {
        let (cancel_tx, cancel_rx) = watch::channel(false);
        Self {
            steps,
            cancel_tx,
            cancel_rx,
            executed_steps: Vec::new(),
            debug: false,
            on_error: crate::engine::packer::OnErrorStrategy::Cleanup,
            ui: None,
            step_hooks: Vec::new(),
        }
    }

    /// Registers a lifecycle step hook on the runner.
    #[must_use]
    pub fn with_step_hook(mut self, hook: std::sync::Arc<dyn StepHook>) -> Self {
        self.step_hooks.push(hook);
        self
    }

    /// Spawns a background task that listens for SIGINT/SIGTERM.
    /// The first signal initiates cancellation on the runner.
    /// The second signal immediately exits the process.
    pub fn spawn_signal_listener(&self) -> tokio::task::JoinHandle<()> {
        let cancel_tx = self.cancel_tx.clone();
        tokio::spawn(async move {
            #[cfg(unix)]
            {
                use tokio::signal::unix::{SignalKind, signal};
                let mut sigint = signal(SignalKind::interrupt()).ok();
                let mut sigterm = signal(SignalKind::terminate()).ok();
                tokio::select! {
                    _ = async {
                        if let Some(ref mut s) = sigint {
                            s.recv().await;
                        } else {
                            std::future::pending::<()>().await;
                        }
                    } => {},
                    _ = async {
                        if let Some(ref mut s) = sigterm {
                            s.recv().await;
                        } else {
                            std::future::pending::<()>().await;
                        }
                    } => {},
                }
            }
            #[cfg(not(unix))]
            {
                let _ = tokio::signal::ctrl_c().await;
            }
            let _ = cancel_tx.send(true);

            #[cfg(unix)]
            {
                use tokio::signal::unix::{SignalKind, signal};
                let mut sigint2 = signal(SignalKind::interrupt()).ok();
                let mut sigterm2 = signal(SignalKind::terminate()).ok();
                tokio::select! {
                    _ = async {
                        if let Some(ref mut s) = sigint2 {
                            s.recv().await;
                        } else {
                            std::future::pending::<()>().await;
                        }
                    } => {},
                    _ = async {
                        if let Some(ref mut s) = sigterm2 {
                            s.recv().await;
                        } else {
                            std::future::pending::<()>().await;
                        }
                    } => {},
                }
            }
            #[cfg(not(unix))]
            {
                let _ = tokio::signal::ctrl_c().await;
            }
            std::process::exit(130);
        })
    }

    /// Enables or disables interactive debug pause mode.
    #[must_use]
    pub fn with_debug(mut self, debug: bool) -> Self {
        self.debug = debug;
        self
    }

    /// Sets the on-error strategy.
    #[must_use]
    pub fn with_on_error(mut self, on_error: crate::engine::packer::OnErrorStrategy) -> Self {
        self.on_error = on_error;
        self
    }

    /// Sets the UI for reporting and prompting.
    #[must_use]
    pub fn with_ui(mut self, ui: std::sync::Arc<crate::engine::ui::Ui>) -> Self {
        self.ui = Some(ui);
        self
    }

    /// Cancels the runner, signaling all steps to halt.
    pub fn cancel(&self) {
        let _ = self.cancel_tx.send(true);
    }

    /// Runs all steps in sequence, passing the state bag to each.
    /// If any step returns `StepAction::Halt`, execution stops and
    /// cleanup is initiated for all steps that have been run (in reverse order).
    /// If the runner is cancelled via `cancel()`, it will also halt before the next step.
    /// # Errors
    /// Returns an error if any step fails.
    pub async fn run(&mut self, state: &mut StateBag) -> Result<(), crate::error::StampError> {
        state.cancel_rx = Some(self.cancel_rx.clone());
        let mut err = None;

        let num_steps = self.steps.len();
        let mut idx = 0;

        while idx < num_steps {
            if *self.cancel_rx.borrow() {
                err = Some(crate::error::StampError::Execution(
                    "Build cancelled".into(),
                ));
                break;
            }

            let step = &mut self.steps[idx];
            let step_name = step.name();

            if self.debug
                && let Some(ref ui) = self.ui
            {
                ui.say(
                    "multistep",
                    &format!("==> Pausing before step: {step_name}"),
                );
                if let Some(ip) = state.get::<String>("instance_ip") {
                    ui.say("multistep", &format!("    Host IP: {ip}"));
                }
                if let Some(user) = state.get::<String>("ssh_user") {
                    ui.say("multistep", &format!("    User: {user}"));
                }
                if let Some(port) = state.get::<u16>("ssh_port") {
                    ui.say("multistep", &format!("    Port: {port}"));
                }
                if let Some(key) = state.get::<String>("ssh_private_key_file") {
                    ui.say("multistep", &format!("    Key: {key}"));
                }

                let prompt_res = ui.ask("multistep", "Press enter to continue, or 'c' to cancel: ");
                if let Ok(resp) = prompt_res
                    && (resp == "c" || resp == "cancel")
                {
                    self.cancel();
                    err = Some(crate::error::StampError::Execution(
                        "Build cancelled by user during debug pause".into(),
                    ));
                    break;
                }

                if *self.cancel_rx.borrow() {
                    err = Some(crate::error::StampError::Execution(
                        "Build cancelled".into(),
                    ));
                    break;
                }
            }

            if !self.executed_steps.contains(&idx) {
                self.executed_steps.push(idx);
            }

            state.record_step(&step_name);

            for hook in &self.step_hooks {
                hook.pre_step(&step_name, state).await?;
            }

            match step.run(state).await {
                Ok(StepAction::Halt) => {
                    err = Some(crate::error::StampError::Execution(format!(
                        "Step '{step_name}' halted execution"
                    )));
                    break;
                }
                Ok(StepAction::Continue) => {
                    for hook in &self.step_hooks {
                        hook.post_step(&step_name, state).await?;
                    }
                    idx += 1;
                }
                Err(e) => {
                    for hook in &self.step_hooks {
                        hook.step_error(&step_name, &e, state).await;
                    }

                    if let Ok(true) = step.recover(state).await {
                        if let Some(ref ui) = self.ui {
                            ui.say(
                                "multistep",
                                &format!("Step '{step_name}' recovered from failure"),
                            );
                        }
                        idx += 1;
                        continue;
                    }

                    match self.on_error {
                        crate::engine::packer::OnErrorStrategy::Abort => {
                            err = Some(e);
                            break;
                        }
                        crate::engine::packer::OnErrorStrategy::RunCleanupProvisioner => {
                            if let Some(ref ui) = self.ui {
                                ui.say(
                                    "multistep",
                                    "OnErrorStrategy::RunCleanupProvisioner: Preserving machine state and executing error cleanup provisioners",
                                );
                            }
                            if let Some(comm) = state.get::<std::sync::Arc<
                                dyn crate::communicator::Communicator,
                            >>("communicator")
                                && let Some(hook) = state.get::<std::sync::Arc<
                                    dyn crate::engine::hook::ProvisionHook,
                                >>(
                                    "provision_hook"
                                )
                            {
                                let ctx = state.build_context().cloned().unwrap_or_default();
                                if let Some(ref ui) = self.ui {
                                    let _ = hook
                                        .run_error_cleanup_provisioners(
                                            comm.clone(),
                                            &ctx,
                                            ui.clone(),
                                        )
                                        .await;
                                }
                            }
                            err = Some(e);
                            break;
                        }
                        crate::engine::packer::OnErrorStrategy::Cleanup => {
                            self.cleanup(state).await;
                            err = Some(e);
                            break;
                        }
                        crate::engine::packer::OnErrorStrategy::Ask => {
                            let mut current_err = e;
                            loop {
                                let action = if let Some(ref ui) = self.ui {
                                    ui.error(
                                        "multistep",
                                        &format!("Step '{step_name}' failed: {current_err}"),
                                    );
                                    if ui.is_interactive() {
                                        ui.ask(
                                            "multistep",
                                            "What would you like to do? [r]etry, [c]leanup, [a]bort, [s]hell: ",
                                        )
                                        .unwrap_or_else(|_| "cleanup".to_string())
                                    } else {
                                        ui.say(
                                            "multistep",
                                            "Non-interactive environment detected, defaulting to cleanup",
                                        );
                                        "cleanup".to_string()
                                    }
                                } else {
                                    "cleanup".to_string()
                                };

                                match action.as_str() {
                                    "r" | "retry" => {
                                        if let Some(ref ui) = self.ui {
                                            ui.say(
                                                "multistep",
                                                &format!("Retrying step: {step_name}"),
                                            );
                                        }
                                        match step.run(state).await {
                                            Ok(StepAction::Continue) => {
                                                idx += 1;
                                                break;
                                            }
                                            Ok(StepAction::Halt) => {
                                                err = Some(crate::error::StampError::Execution(
                                                    format!("Step '{step_name}' halted execution"),
                                                ));
                                                break;
                                            }
                                            Err(new_err) => {
                                                current_err = new_err;
                                            }
                                        }
                                    }
                                    "a" | "abort" => {
                                        err = Some(current_err);
                                        break;
                                    }
                                    "s" | "shell" => {
                                        if let Some(ref ui) = self.ui {
                                            ui.say(
                                                "multistep",
                                                "Entering interactive debug shell session...",
                                            );
                                            if let Some(comm) = state.get::<std::sync::Arc<
                                                dyn crate::communicator::Communicator,
                                            >>(
                                                "communicator"
                                            ) {
                                                loop {
                                                    let prompt = ui.ask("shell", "debug-shell> ");
                                                    match prompt {
                                                        Ok(cmd)
                                                            if cmd.trim() == "exit"
                                                                || cmd.trim() == "quit" =>
                                                        {
                                                            ui.say(
                                                                "multistep",
                                                                "Exited debug shell session.",
                                                            );
                                                            break;
                                                        }
                                                        Ok(cmd) if !cmd.trim().is_empty() => {
                                                            let exec_cmd =
                                                                crate::communicator::Command::new(
                                                                    cmd.trim().to_string(),
                                                                );
                                                            match comm.execute(&exec_cmd).await {
                                                                Ok(res) => {
                                                                    if !res.stdout.is_empty() {
                                                                        ui.say(
                                                                            "shell",
                                                                            &res.stdout,
                                                                        );
                                                                    }
                                                                    if !res.stderr.is_empty() {
                                                                        ui.error(
                                                                            "shell",
                                                                            &res.stderr,
                                                                        );
                                                                    }
                                                                }
                                                                Err(err) => {
                                                                    ui.error(
                                                                        "shell",
                                                                        &format!(
                                                                            "Execution error: {err}"
                                                                        ),
                                                                    );
                                                                }
                                                            }
                                                        }
                                                        _ => break,
                                                    }
                                                }
                                            } else {
                                                ui.error("multistep", "No active communicator found in build state for shell session");
                                            }
                                        }
                                    }
                                    _ => {
                                        self.cleanup(state).await;
                                        err = Some(current_err);
                                        break;
                                    }
                                }
                            }
                            if err.is_some() {
                                break;
                            }
                        }
                    }
                }
            }
        }

        if let Some(e) = err { Err(e) } else { Ok(()) }
    }

    /// Cleans up the executed steps in reverse order.
    pub async fn cleanup(&mut self, state: &StateBag) {
        for i in self.executed_steps.drain(..).rev() {
            self.steps[i].cleanup(state).await;
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::pedantic, clippy::all)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    struct TestStep {
        name: String,
        action: StepAction,
        log: Arc<Mutex<Vec<String>>>,
    }

    #[async_trait::async_trait]
    impl Step for TestStep {
        async fn run(
            &mut self,
            state: &mut StateBag,
        ) -> Result<StepAction, crate::error::StampError> {
            if state.is_cancelled() {
                self.log
                    .lock()
                    .unwrap_or_else(|e| panic!("{e:?}"))
                    .push(format!("cancel {}", self.name));
                return Ok(StepAction::Halt);
            }
            self.log
                .lock()
                .unwrap_or_else(|e| panic!("{e:?}"))
                .push(format!("run {}", self.name));
            Ok(self.action)
        }

        async fn cleanup(&mut self, _state: &StateBag) {
            self.log
                .lock()
                .unwrap_or_else(|e| panic!("{e:?}"))
                .push(format!("cleanup {}", self.name));
        }
    }

    #[test]
    fn test_state_bag() {
        let mut bag = StateBag::new();
        bag.put("key1", 42i32);
        bag.put("key2", "hello".to_string());

        assert_eq!(bag.get::<i32>("key1"), Some(&42));
        assert_eq!(bag.get::<String>("key2"), Some(&"hello".to_string()));
        assert_eq!(bag.get::<i32>("key2"), None); // Wrong type
        assert_eq!(bag.get::<i32>("missing"), None);

        if let Some(v) = bag.get_mut::<i32>("key1") {
            *v = 100;
        }
        assert_eq!(bag.get::<i32>("key1"), Some(&100));
        assert!(!bag.is_cancelled());

        bag.set_instance_ip("192.168.1.100".to_string());
        assert_eq!(bag.instance_ip(), Some("192.168.1.100"));

        bag.set_ssh_port(2222);
        assert_eq!(bag.ssh_port(), Some(2222));

        bag.set_artifact_id("ami-12345".to_string());
        assert_eq!(bag.artifact_id(), Some("ami-12345"));

        bag.record_step("StepOne");
        bag.record_step("StepTwo");
        let history = bag.execution_history().unwrap();
        assert_eq!(history, &["StepOne", "StepTwo"]);
    }

    #[tokio::test]
    async fn test_runner_success() {
        let log = Arc::new(Mutex::new(Vec::new()));

        let step1 = TestStep {
            name: "1".to_string(),
            action: StepAction::Continue,
            log: log.clone(),
        };
        let step2 = TestStep {
            name: "2".to_string(),
            action: StepAction::Continue,
            log: log.clone(),
        };

        let mut runner = Runner::new(vec![Box::new(step1), Box::new(step2)]);
        let mut state = StateBag::new();

        runner
            .run(&mut state)
            .await
            .unwrap_or_else(|e| panic!("{e:?}"));
        runner.cleanup(&state).await;

        let log_data = log.lock().unwrap_or_else(|e| panic!("{e:?}"));
        assert_eq!(*log_data, vec!["run 1", "run 2", "cleanup 2", "cleanup 1"]);
    }

    #[tokio::test]
    async fn test_runner_halt() {
        let log = Arc::new(Mutex::new(Vec::new()));

        let step1 = TestStep {
            name: "1".to_string(),
            action: StepAction::Continue,
            log: log.clone(),
        };
        let step2 = TestStep {
            name: "2".to_string(),
            action: StepAction::Halt,
            log: log.clone(),
        };
        let step3 = TestStep {
            name: "3".to_string(),
            action: StepAction::Continue,
            log: log.clone(),
        };

        let mut runner = Runner::new(vec![Box::new(step1), Box::new(step2), Box::new(step3)]);
        let mut state = StateBag::new();

        let res = runner.run(&mut state).await;
        assert!(res.is_err());
        runner.cleanup(&state).await;

        let log_data = log.lock().unwrap_or_else(|e| panic!("{e:?}"));
        // step3 should not be run, but step1 and step2 should be cleaned up.
        assert_eq!(*log_data, vec!["run 1", "run 2", "cleanup 2", "cleanup 1"]);
    }

    #[tokio::test]
    async fn test_runner_empty() {
        let mut runner = Runner::new(vec![]);
        let mut state = StateBag::new();
        runner
            .run(&mut state)
            .await
            .unwrap_or_else(|e| panic!("{e:?}"));
    }

    #[tokio::test]
    async fn test_runner_cancel() {
        let log = Arc::new(Mutex::new(Vec::new()));

        let step1 = TestStep {
            name: "1".to_string(),
            action: StepAction::Continue,
            log: log.clone(),
        };
        let step2 = TestStep {
            name: "2".to_string(),
            action: StepAction::Continue,
            log: log.clone(),
        };

        let mut runner = Runner::new(vec![Box::new(step1), Box::new(step2)]);
        let mut state = StateBag::new();

        runner.cancel();

        let res = runner.run(&mut state).await;
        assert!(res.is_err());
        runner.cleanup(&state).await;

        let log_data = log.lock().unwrap_or_else(|e| panic!("{e:?}"));
        assert!(log_data.is_empty());
    }

    #[tokio::test]
    async fn test_runner_cancel_during_run() {
        let log = Arc::new(Mutex::new(Vec::new()));

        struct CancelStep {
            log: Arc<Mutex<Vec<String>>>,
            cancel_tx: watch::Sender<bool>,
        }

        #[async_trait::async_trait]
        impl Step for CancelStep {
            async fn run(
                &mut self,
                _state: &mut StateBag,
            ) -> Result<StepAction, crate::error::StampError> {
                self.log
                    .lock()
                    .unwrap_or_else(|_| panic!("failed"))
                    .push("run cancel_step".to_string());
                let _ = self.cancel_tx.send(true);
                Ok(StepAction::Continue)
            }

            async fn cleanup(&mut self, _state: &StateBag) {
                self.log
                    .lock()
                    .unwrap_or_else(|_| panic!("failed"))
                    .push("cleanup cancel_step".to_string());
            }
        }

        let (cancel_tx, cancel_rx) = watch::channel(false);
        let step1 = CancelStep {
            log: log.clone(),
            cancel_tx: cancel_tx.clone(),
        };
        let step2 = TestStep {
            name: "2".to_string(),
            action: StepAction::Continue,
            log: log.clone(),
        };

        let mut runner = Runner {
            steps: vec![Box::new(step1), Box::new(step2)],
            cancel_tx,
            cancel_rx,
            executed_steps: Vec::new(),
            debug: false,
            on_error: crate::engine::packer::OnErrorStrategy::Cleanup,
            ui: None,
            step_hooks: Vec::new(),
        };

        let mut state = StateBag::new();
        let res = runner.run(&mut state).await;
        assert!(res.is_err());
        runner.cleanup(&state).await;

        let log_data = log.lock().unwrap_or_else(|_| panic!("failed"));
        // step2 shouldn't run because it's cancelled
        assert_eq!(*log_data, vec!["run cancel_step", "cleanup cancel_step"]);
    }

    #[tokio::test]
    async fn test_runner_debug_and_on_error_modes() {
        let mut state = StateBag::new();
        state.put("instance_ip", "10.0.0.1".to_string());
        state.put("ssh_user", "ubuntu".to_string());
        state.put("ssh_port", 22u16);
        state.put("ssh_private_key_file", "/tmp/id_rsa".to_string());

        let ui = std::sync::Arc::new(crate::engine::ui::Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));

        // Test with debug mode and with UI
        let step1 = TestStep {
            name: "dbg-step".to_string(),
            action: StepAction::Continue,
            log: Arc::new(Mutex::new(Vec::new())),
        };
        let mut runner = Runner::new(vec![Box::new(step1)])
            .with_debug(true)
            .with_ui(ui.clone());

        assert!(runner.run(&mut state).await.is_ok());

        // Test with Abort strategy
        struct FailingStep;
        #[async_trait::async_trait]
        impl Step for FailingStep {
            async fn run(
                &mut self,
                _state: &mut StateBag,
            ) -> Result<StepAction, crate::error::StampError> {
                Err(crate::error::StampError::Execution("fail".to_string()))
            }
            async fn cleanup(&mut self, _state: &StateBag) {}
        }

        let mut abort_runner = Runner::new(vec![Box::new(FailingStep)])
            .with_on_error(crate::engine::packer::OnErrorStrategy::Abort);
        assert!(abort_runner.run(&mut state).await.is_err());

        // Test with Ask strategy
        let mut ask_runner = Runner::new(vec![Box::new(FailingStep)])
            .with_ui(ui)
            .with_on_error(crate::engine::packer::OnErrorStrategy::Ask);
        assert!(ask_runner.run(&mut state).await.is_err());
    }

    #[tokio::test]
    async fn test_runner_debug_pause_cancel() {
        let mut state = StateBag::new();
        let queue = Arc::new(Mutex::new(std::collections::VecDeque::new()));
        if let Ok(mut q) = queue.lock() {
            q.push_back("c".to_string());
        }
        let ui = Arc::new(
            crate::engine::ui::Ui::new(
                crate::engine::packer::FeatureState::Disabled,
                crate::engine::packer::FeatureState::Disabled,
                crate::engine::packer::FeatureState::Disabled,
            )
            .with_mock_inputs(queue),
        );

        let step1 = TestStep {
            name: "dbg-cancel-step".to_string(),
            action: StepAction::Continue,
            log: Arc::new(Mutex::new(Vec::new())),
        };
        let mut runner = Runner::new(vec![Box::new(step1)])
            .with_debug(true)
            .with_ui(ui);

        let res = runner.run(&mut state).await;
        assert!(res.is_err());
    }

    struct RetryStep {
        attempts: Arc<Mutex<usize>>,
        action_on_retry: StepAction,
    }

    #[async_trait::async_trait]
    impl Step for RetryStep {
        async fn run(
            &mut self,
            _state: &mut StateBag,
        ) -> Result<StepAction, crate::error::StampError> {
            let mut att = self
                .attempts
                .lock()
                .map_err(|e| crate::error::StampError::Execution(e.to_string()))?;
            *att += 1;
            if *att == 1 {
                Err(crate::error::StampError::Execution(
                    "first attempt fail".to_string(),
                ))
            } else {
                Ok(self.action_on_retry)
            }
        }
        async fn cleanup(&mut self, _state: &StateBag) {}
    }

    #[tokio::test]
    async fn test_runner_on_error_ask_retry_success() {
        let mut state = StateBag::new();
        let queue = Arc::new(Mutex::new(std::collections::VecDeque::new()));
        if let Ok(mut q) = queue.lock() {
            q.push_back("retry".to_string());
        }
        let ui = Arc::new(
            crate::engine::ui::Ui::new(
                crate::engine::packer::FeatureState::Disabled,
                crate::engine::packer::FeatureState::Disabled,
                crate::engine::packer::FeatureState::Disabled,
            )
            .with_mock_inputs(queue),
        );

        let attempts = Arc::new(Mutex::new(0));
        let step = RetryStep {
            attempts: attempts.clone(),
            action_on_retry: StepAction::Continue,
        };

        let mut runner = Runner::new(vec![Box::new(step)])
            .with_ui(ui)
            .with_on_error(crate::engine::packer::OnErrorStrategy::Ask);

        assert!(runner.run(&mut state).await.is_ok());
        let final_attempts = *attempts.lock().unwrap_or_else(|_| panic!("lock failed"));
        assert_eq!(final_attempts, 2);
    }

    #[tokio::test]
    async fn test_runner_on_error_ask_retry_halt() {
        let mut state = StateBag::new();
        let queue = Arc::new(Mutex::new(std::collections::VecDeque::new()));
        if let Ok(mut q) = queue.lock() {
            q.push_back("r".to_string());
        }
        let ui = Arc::new(
            crate::engine::ui::Ui::new(
                crate::engine::packer::FeatureState::Disabled,
                crate::engine::packer::FeatureState::Disabled,
                crate::engine::packer::FeatureState::Disabled,
            )
            .with_mock_inputs(queue),
        );

        let attempts = Arc::new(Mutex::new(0));
        let step = RetryStep {
            attempts: attempts.clone(),
            action_on_retry: StepAction::Halt,
        };

        let mut runner = Runner::new(vec![Box::new(step)])
            .with_ui(ui)
            .with_on_error(crate::engine::packer::OnErrorStrategy::Ask);

        assert!(runner.run(&mut state).await.is_err());
    }

    #[tokio::test]
    async fn test_runner_on_error_ask_abort() {
        let mut state = StateBag::new();
        let queue = Arc::new(Mutex::new(std::collections::VecDeque::new()));
        if let Ok(mut q) = queue.lock() {
            q.push_back("abort".to_string());
        }
        let ui = Arc::new(
            crate::engine::ui::Ui::new(
                crate::engine::packer::FeatureState::Disabled,
                crate::engine::packer::FeatureState::Disabled,
                crate::engine::packer::FeatureState::Disabled,
            )
            .with_mock_inputs(queue),
        );

        struct FailingStep;
        #[async_trait::async_trait]
        impl Step for FailingStep {
            async fn run(
                &mut self,
                _state: &mut StateBag,
            ) -> Result<StepAction, crate::error::StampError> {
                Err(crate::error::StampError::Execution("fail".to_string()))
            }
            async fn cleanup(&mut self, _state: &StateBag) {}
        }

        let mut runner = Runner::new(vec![Box::new(FailingStep)])
            .with_ui(ui)
            .with_on_error(crate::engine::packer::OnErrorStrategy::Ask);

        assert!(runner.run(&mut state).await.is_err());
    }

    #[tokio::test]
    async fn test_runner_on_error_ask_shell() {
        let mut state = StateBag::new();
        let mock_comm: Arc<dyn crate::communicator::Communicator> =
            Arc::new(crate::communicator::mock::MockCommunicator::default());
        state.put("communicator", mock_comm);

        let queue = Arc::new(Mutex::new(std::collections::VecDeque::new()));
        if let Ok(mut q) = queue.lock() {
            q.push_back("shell".to_string()); // ask choice
            q.push_back("echo test".to_string()); // in shell command
            q.push_back("exit".to_string()); // exit shell
            q.push_back("cleanup".to_string()); // subsequent ask choice
        }
        let ui = Arc::new(
            crate::engine::ui::Ui::new(
                crate::engine::packer::FeatureState::Disabled,
                crate::engine::packer::FeatureState::Disabled,
                crate::engine::packer::FeatureState::Disabled,
            )
            .with_mock_inputs(queue),
        );

        struct FailingStep;
        #[async_trait::async_trait]
        impl Step for FailingStep {
            async fn run(
                &mut self,
                _state: &mut StateBag,
            ) -> Result<StepAction, crate::error::StampError> {
                Err(crate::error::StampError::Execution("fail".to_string()))
            }
            async fn cleanup(&mut self, _state: &StateBag) {}
        }

        let mut runner = Runner::new(vec![Box::new(FailingStep)])
            .with_ui(ui)
            .with_on_error(crate::engine::packer::OnErrorStrategy::Ask);

        assert!(runner.run(&mut state).await.is_err());
    }
}

#[cfg(test)]
mod tests_err {
    use super::*;
    use std::sync::Arc;

    struct ErrorStep;
    #[async_trait::async_trait]
    impl Step for ErrorStep {
        async fn run(
            &mut self,
            _state: &mut StateBag,
        ) -> Result<StepAction, crate::error::StampError> {
            Err(crate::error::StampError::Execution("mock err".to_string()))
        }
        async fn cleanup(&mut self, _state: &StateBag) {}
    }

    #[tokio::test]
    async fn test_runner_error() {
        let mut runner = Runner::new(vec![Box::new(ErrorStep)]);
        let mut state = StateBag::new();
        let res = runner.run(&mut state).await;
        assert!(res.is_err());
    }

    struct RecoveringStep {
        recovered: bool,
    }
    #[async_trait::async_trait]
    impl Step for RecoveringStep {
        async fn run(
            &mut self,
            _state: &mut StateBag,
        ) -> Result<StepAction, crate::error::StampError> {
            Err(crate::error::StampError::Execution(
                "recoverable error".to_string(),
            ))
        }
        async fn recover(
            &mut self,
            _state: &mut StateBag,
        ) -> Result<bool, crate::error::StampError> {
            self.recovered = true;
            Ok(true)
        }
        async fn cleanup(&mut self, _state: &StateBag) {}
    }

    #[tokio::test]
    async fn test_runner_step_recovery() {
        let step = RecoveringStep { recovered: false };
        let mut runner = Runner::new(vec![Box::new(step)]);
        let mut state = StateBag::new();
        let res = runner.run(&mut state).await;
        assert!(res.is_ok());
    }

    #[tokio::test]
    async fn test_runner_on_error_run_cleanup_provisioner() {
        let step = ErrorStep;
        let mut runner = Runner::new(vec![Box::new(step)])
            .with_on_error(crate::engine::packer::OnErrorStrategy::RunCleanupProvisioner);
        let mut state = StateBag::new();
        let res = runner.run(&mut state).await;
        assert!(res.is_err());
    }

    #[tokio::test]
    async fn test_runner_on_error_ask_non_interactive_fallback() {
        let ui = std::sync::Arc::new(crate::engine::ui::Ui::new(
            crate::engine::packer::FeatureState::Enabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));
        let step = ErrorStep;
        let mut runner = Runner::new(vec![Box::new(step)])
            .with_ui(ui)
            .with_on_error(crate::engine::packer::OnErrorStrategy::Ask);
        let mut state = StateBag::new();
        let res = runner.run(&mut state).await;
        assert!(res.is_err());
    }

    #[test]
    fn test_state_bag_strongly_typed_accessors() {
        let mut bag = StateBag::new();
        assert!(bag.build_context().is_none());
        assert!(bag.host().is_none());
        assert!(bag.user().is_none());
        assert!(bag.password().is_none());
        assert!(bag.conn_info().is_none());
        assert!(bag.ssh_public_key().is_none());
        assert!(bag.ssh_private_key().is_none());
        assert!(bag.packer_run_uuid().is_none());
        assert!(bag.source_ami().is_none());
        assert!(bag.source_ami_name().is_none());

        let ctx = crate::engine::hook::BuildContext {
            build_id: "bid".to_string(),
            host: "10.0.0.1".to_string(),
            ..Default::default()
        };
        bag.set_build_context(ctx.clone());
        assert_eq!(bag.build_context(), Some(&ctx));

        bag.set_host("192.168.1.1".to_string());
        assert_eq!(bag.host(), Some("192.168.1.1"));

        bag.set_user("admin".to_string());
        assert_eq!(bag.user(), Some("admin"));

        bag.set_password("secret".to_string());
        assert_eq!(bag.password(), Some("secret"));

        let mut ci = HashMap::new();
        ci.insert("k".to_string(), "v".to_string());
        bag.set_conn_info(ci.clone());
        assert_eq!(bag.conn_info(), Some(&ci));

        bag.set_ssh_public_key("ssh-rsa AAA...".to_string());
        assert_eq!(bag.ssh_public_key(), Some("ssh-rsa AAA..."));

        bag.set_ssh_private_key("/path/to/key".to_string());
        assert_eq!(bag.ssh_private_key(), Some("/path/to/key"));

        bag.set_packer_run_uuid("uuid-1234".to_string());
        assert_eq!(bag.packer_run_uuid(), Some("uuid-1234"));

        bag.set_source_ami("ami-12345678".to_string());
        assert_eq!(bag.source_ami(), Some("ami-12345678"));

        bag.set_source_ami_name("ubuntu-base".to_string());
        assert_eq!(bag.source_ami_name(), Some("ubuntu-base"));
    }

    #[test]
    fn test_shared_state_bag() {
        let shared = SharedStateBag::new();
        shared.write(|bag| {
            bag.set_host("127.0.0.1".to_string());
        });
        let host = shared.read(|bag| bag.host().map(ToString::to_string));
        assert_eq!(host, Some("127.0.0.1".to_string()));

        let mut bag = StateBag::new();
        bag.set_user("root".to_string());
        let shared2 = SharedStateBag::from_bag(bag);
        assert_eq!(
            shared2.read(|b| b.user().map(ToString::to_string)),
            Some("root".to_string())
        );
    }

    struct MockHook {
        pre_called: std::sync::atomic::AtomicBool,
        post_called: std::sync::atomic::AtomicBool,
        err_called: std::sync::atomic::AtomicBool,
    }

    #[async_trait::async_trait]
    impl StepHook for MockHook {
        async fn pre_step(
            &self,
            _step_name: &str,
            _state: &mut StateBag,
        ) -> Result<(), crate::error::StampError> {
            self.pre_called
                .store(true, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        }
        async fn post_step(
            &self,
            _step_name: &str,
            _state: &mut StateBag,
        ) -> Result<(), crate::error::StampError> {
            self.post_called
                .store(true, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        }
        async fn step_error(
            &self,
            _step_name: &str,
            _error: &crate::error::StampError,
            _state: &mut StateBag,
        ) {
            self.err_called
                .store(true, std::sync::atomic::Ordering::SeqCst);
        }
    }

    struct SuccessStep;
    #[async_trait::async_trait]
    impl Step for SuccessStep {
        async fn run(
            &mut self,
            _state: &mut StateBag,
        ) -> Result<StepAction, crate::error::StampError> {
            Ok(StepAction::Continue)
        }
        async fn cleanup(&mut self, _state: &StateBag) {}
    }

    #[tokio::test]
    async fn test_step_hooks_success() {
        let hook = Arc::new(MockHook {
            pre_called: std::sync::atomic::AtomicBool::new(false),
            post_called: std::sync::atomic::AtomicBool::new(false),
            err_called: std::sync::atomic::AtomicBool::new(false),
        });
        let mut runner = Runner::new(vec![Box::new(SuccessStep)]).with_step_hook(hook.clone());
        let mut state = StateBag::new();
        let res = runner.run(&mut state).await;
        assert!(res.is_ok());
        assert!(hook.pre_called.load(std::sync::atomic::Ordering::SeqCst));
        assert!(hook.post_called.load(std::sync::atomic::Ordering::SeqCst));
        assert!(!hook.err_called.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[tokio::test]
    async fn test_step_hooks_failure() {
        let hook = Arc::new(MockHook {
            pre_called: std::sync::atomic::AtomicBool::new(false),
            post_called: std::sync::atomic::AtomicBool::new(false),
            err_called: std::sync::atomic::AtomicBool::new(false),
        });
        let mut runner = Runner::new(vec![Box::new(ErrorStep)]).with_step_hook(hook.clone());
        let mut state = StateBag::new();
        let res = runner.run(&mut state).await;
        assert!(res.is_err());
        assert!(hook.pre_called.load(std::sync::atomic::Ordering::SeqCst));
        assert!(!hook.post_called.load(std::sync::atomic::Ordering::SeqCst));
        assert!(hook.err_called.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[tokio::test]
    async fn test_runner_signal_listener() {
        let runner = Runner::new(vec![Box::new(SuccessStep)]);
        let handle = runner.spawn_signal_listener();
        // Abort background listener immediately in test to avoid interfering with process
        handle.abort();
    }
}
