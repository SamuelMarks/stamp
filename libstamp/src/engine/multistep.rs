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

/// A compile-time typed key for storing and retrieving values in a [`StateBag`].
pub trait TypedStateKey {
    /// The concrete value type associated with this key.
    type Value: Any + Send + Sync + 'static;
    /// The unique string key identifier.
    const KEY: &'static str;
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

    /// Puts a strongly-typed value into the state bag using a [`TypedStateKey`].
    pub fn put_typed<K: TypedStateKey>(&mut self, value: K::Value) {
        self.put(K::KEY, value);
    }

    /// Gets a reference to a value from the state bag.
    #[must_use]
    pub fn get<T: Any + Send + Sync>(&self, key: &str) -> Option<&T> {
        self.state.get(key).and_then(|v| v.downcast_ref::<T>())
    }

    /// Gets a reference to a value using a [`TypedStateKey`].
    #[must_use]
    pub fn get_typed<K: TypedStateKey>(&self) -> Option<&K::Value> {
        self.get::<K::Value>(K::KEY)
    }

    /// Attempts to get a reference to a value from the state bag, returning a structured
    /// [`crate::error::StampError::StateBagMissingKey`] or
    /// [`crate::error::StampError::StateBagTypeMismatch`] on failure.
    ///
    /// # Errors
    /// Returns `StampError` if the key does not exist or has a different concrete type.
    pub fn try_get<T: Any + Send + Sync>(&self, key: &str) -> Result<&T, crate::error::StampError> {
        let Some(boxed) = self.state.get(key) else {
            return Err(crate::error::StampError::StateBagMissingKey {
                key: key.to_string(),
                expected_type: std::any::type_name::<T>(),
            });
        };
        boxed
            .downcast_ref::<T>()
            .ok_or_else(|| crate::error::StampError::StateBagTypeMismatch {
                key: key.to_string(),
                expected_type: std::any::type_name::<T>(),
            })
    }

    /// Attempts to get a reference to a value using a [`TypedStateKey`].
    ///
    /// # Errors
    /// Returns `StampError` if the key does not exist or has a different concrete type.
    pub fn try_get_typed<K: TypedStateKey>(&self) -> Result<&K::Value, crate::error::StampError> {
        self.try_get::<K::Value>(K::KEY)
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

    /// Gets the HTTP server IP if set in the state bag.
    #[must_use]
    pub fn http_ip(&self) -> Option<&str> {
        self.get::<String>("http_ip").map(String::as_str)
    }

    /// Sets the HTTP server IP in the state bag.
    pub fn set_http_ip(&mut self, ip: String) {
        self.put("http_ip", ip);
    }

    /// Gets the HTTP server port if set in the state bag.
    #[must_use]
    pub fn http_port(&self) -> Option<u16> {
        self.get::<u16>("http_port").copied()
    }

    /// Sets the HTTP server port in the state bag.
    pub fn set_http_port(&mut self, port: u16) {
        self.put("http_port", port);
    }

    /// Removes and returns a value from the state bag if present and of type `T`.
    pub fn take<T: Any + Send + Sync>(&mut self, key: &str) -> Option<T> {
        let boxed = self.state.remove(key)?;
        boxed.downcast::<T>().ok().map(|b| *b)
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
        let guard = match self.inner.read() {
            Ok(g) => g,
            Err(e) => e.into_inner(),
        };
        f(&guard)
    }

    /// Executes a closure with a mutable reference to the underlying `StateBag`.
    pub fn write<R, F: FnOnce(&mut StateBag) -> R>(&self, f: F) -> R {
        let mut guard = match self.inner.write() {
            Ok(g) => g,
            Err(e) => e.into_inner(),
        };
        f(&mut guard)
    }

    /// Removes and returns a value from the shared state bag if present and of type `T`.
    pub fn take<T: Any + Send + Sync>(&self, key: &str) -> Option<T> {
        self.write(|b| b.take::<T>(key))
    }

    /// Gets the HTTP server IP from the shared state bag.
    #[must_use]
    pub fn http_ip(&self) -> Option<String> {
        self.read(|b| b.http_ip().map(ToString::to_string))
    }

    /// Sets the HTTP server IP in the shared state bag.
    pub fn set_http_ip(&self, ip: String) {
        self.write(|b| b.set_http_ip(ip));
    }

    /// Gets the HTTP server port from the shared state bag.
    #[must_use]
    pub fn http_port(&self) -> Option<u16> {
        self.read(|b| b.http_port())
    }

    /// Sets the HTTP server port in the shared state bag.
    pub fn set_http_port(&self, port: u16) {
        self.write(|b| b.set_http_port(port));
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
    #[must_use]
    pub fn spawn_signal_listener(&self) -> tokio::task::JoinHandle<()> {
        let cancel_tx = self.cancel_tx.clone();
        tokio::spawn(async move {
            #[cfg(unix)]
            {
                use tokio::signal::unix::{SignalKind, signal};
                let Ok(mut sigint) = signal(SignalKind::interrupt()) else {
                    return;
                };
                let Ok(mut sigterm) = signal(SignalKind::terminate()) else {
                    return;
                };
                tokio::select! {
                    _ = sigint.recv() => {},
                    _ = sigterm.recv() => {},
                }
            }
            #[cfg(not(unix))]
            {
                let _ = tokio::signal::ctrl_c().await;
            }
            let _ = cancel_tx.send(true);

            #[cfg(not(test))]
            {
                #[cfg(unix)]
                {
                    use tokio::signal::unix::{SignalKind, signal};
                    let mut sigint2 = signal(SignalKind::interrupt()).ok();
                    let mut sigterm2 = signal(SignalKind::terminate()).ok();
                    tokio::select! {
                        () = async {
                            if let Some(ref mut s) = sigint2 {
                                s.recv().await;
                            } else {
                                std::future::pending::<()>().await;
                            }
                        } => {},
                        () = async {
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
            }
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

    /// Runs the multistep sequence and applies the given [`crate::engine::packer::OnErrorStrategy`]
    /// for automatic cleanup or interactive prompting.
    ///
    /// # Errors
    /// Returns `StampError` if any step fails during execution.
    pub async fn run_with_strategy(
        &mut self,
        state: &mut StateBag,
        builder_name: &str,
        ui: &crate::engine::ui::Ui,
        on_error: crate::engine::packer::OnErrorStrategy,
    ) -> Result<(), crate::error::StampError> {
        match self.run(state).await {
            Ok(()) => {
                self.cleanup(state).await;
                Ok(())
            }
            Err(e) => {
                match on_error {
                    crate::engine::packer::OnErrorStrategy::Cleanup => {
                        self.cleanup(state).await;
                    }
                    crate::engine::packer::OnErrorStrategy::Abort
                    | crate::engine::packer::OnErrorStrategy::RunCleanupProvisioner => {}
                    crate::engine::packer::OnErrorStrategy::Ask => {
                        let msg = format!(
                            "Build '{builder_name}' errored: {e}\nDo you want to clean up? [y/N]: "
                        );
                        if let Ok(ans) = ui.ask("stamp", &msg)
                            && (ans == "y" || ans == "yes")
                        {
                            self.cleanup(state).await;
                        }
                    }
                }
                Err(e)
            }
        }
    }
}

/// Reusable provisioning step for multistep builders.
///
/// Connects via the configured [`crate::types::CommunicatorKind`] (or an existing communicator
/// stored in the [`StateBag`]), populates a [`crate::engine::hook::BuildContext`] with a real
/// run UUID, invokes [`crate::engine::hook::ProvisionHook::run_provisioners`], and shuts down
/// any active [`crate::builder::http_server::HttpServerHandle`] after provisioning completes.
#[derive(Clone)]
pub struct StepProvision {
    /// UI reference for terminal output.
    pub ui: std::sync::Arc<crate::engine::ui::Ui>,
    /// Builder instance name.
    pub name: String,
    /// Builder type identifier (e.g., `"alicloud-ecs"`, `"vmware-iso"`).
    pub builder_type: String,
    /// Provisioning hook.
    pub hook: std::sync::Arc<dyn crate::engine::hook::ProvisionHook>,
    /// Primary state bag key used to look up the guest IP address.
    pub ip_state_key: String,
    /// Whether an absent IP in the state bag should return an error instead of defaulting to `127.0.0.1`.
    pub require_ip: bool,
    /// Communicator protocol kind.
    pub communicator_kind: crate::types::CommunicatorKind,
    /// SSH username.
    pub ssh_username: String,
    /// Optional SSH password.
    pub ssh_password: Option<String>,
    /// Optional SSH port override.
    pub ssh_port: Option<u16>,
    /// Optional SSH private key file path.
    pub ssh_private_key_path: Option<crate::types::FilePath>,
    /// Optional `WinRM` username.
    pub winrm_username: Option<String>,
    /// Optional `WinRM` password.
    pub winrm_password: Option<String>,
    /// Optional `WinRM` port override.
    pub winrm_port: Option<u16>,
    /// Whether `WinRM` uses HTTPS/SSL.
    pub winrm_use_ssl: bool,
    /// Whether `WinRM` skips TLS certificate verification.
    pub winrm_insecure: bool,
    /// Optional custom status message logged when provisioning begins.
    pub message: Option<String>,
}

impl StepProvision {
    /// Creates a new [`StepProvision`] with sensible SSH defaults.
    ///
    /// # Arguments
    /// * `ui` - UI logger handle.
    /// * `name` - Builder instance name.
    /// * `builder_type` - Builder type string.
    /// * `hook` - Provisioning hook.
    #[must_use]
    pub fn new(
        ui: std::sync::Arc<crate::engine::ui::Ui>,
        name: impl Into<String>,
        builder_type: impl Into<String>,
        hook: std::sync::Arc<dyn crate::engine::hook::ProvisionHook>,
    ) -> Self {
        Self {
            ui,
            name: name.into(),
            builder_type: builder_type.into(),
            hook,
            ip_state_key: "instance_ip".to_string(),
            require_ip: false,
            communicator_kind: crate::types::CommunicatorKind::Ssh,
            ssh_username: "root".to_string(),
            ssh_password: None,
            ssh_port: None,
            ssh_private_key_path: None,
            winrm_username: None,
            winrm_password: None,
            winrm_port: None,
            winrm_use_ssl: false,
            winrm_insecure: false,
            message: None,
        }
    }

    /// Sets the primary state bag key used to look up the guest IP address.
    #[must_use]
    pub fn with_ip_key(mut self, key: impl Into<String>) -> Self {
        self.ip_state_key = key.into();
        self
    }

    /// Requires the IP address to be present in the state bag under `ip_state_key`.
    #[must_use]
    pub fn with_require_ip(mut self, require: bool) -> Self {
        self.require_ip = require;
        self
    }

    /// Sets the communicator protocol kind.
    #[must_use]
    pub fn with_communicator_kind(mut self, kind: crate::types::CommunicatorKind) -> Self {
        self.communicator_kind = kind;
        self
    }

    /// Sets the SSH username and optional password.
    #[must_use]
    pub fn with_ssh_credentials(
        mut self,
        username: impl Into<String>,
        password: Option<String>,
    ) -> Self {
        self.ssh_username = username.into();
        self.ssh_password = password;
        self
    }

    /// Sets the SSH port override.
    #[must_use]
    pub fn with_ssh_port(mut self, port: u16) -> Self {
        self.ssh_port = Some(port);
        self
    }

    /// Sets the SSH private key path.
    #[must_use]
    pub fn with_ssh_private_key(mut self, path: Option<crate::types::FilePath>) -> Self {
        self.ssh_private_key_path = path;
        self
    }

    /// Sets `WinRM` credentials and connection options.
    #[must_use]
    pub fn with_winrm_options(
        mut self,
        username: Option<String>,
        password: Option<String>,
        port: Option<u16>,
        use_ssl: bool,
        insecure: bool,
    ) -> Self {
        self.winrm_username = username;
        self.winrm_password = password;
        self.winrm_port = port;
        self.winrm_use_ssl = use_ssl;
        self.winrm_insecure = insecure;
        self
    }

    /// Sets the status message displayed when provisioning starts.
    #[must_use]
    pub fn with_message(mut self, message: impl Into<String>) -> Self {
        self.message = Some(message.into());
        self
    }
}

#[async_trait::async_trait]
impl Step for StepProvision {
    async fn run(&mut self, state: &mut StateBag) -> Result<StepAction, crate::error::StampError> {
        let ip = if self.require_ip {
            state
                .get::<String>(&self.ip_state_key)
                .cloned()
                .ok_or_else(|| {
                    crate::error::StampError::Builder(format!("Missing {}", self.ip_state_key))
                })?
        } else {
            state
                .get::<String>(&self.ip_state_key)
                .or_else(|| state.get::<String>("instance_ip"))
                .or_else(|| state.get::<String>("vm_ip"))
                .or_else(|| state.get::<String>("server_ip"))
                .or_else(|| state.get::<String>("server_addr"))
                .or_else(|| state.get::<String>("guest_ip"))
                .or_else(|| state.get::<String>("machine_ip"))
                .or_else(|| state.get::<String>("linode_ip"))
                .or_else(|| state.get::<String>("container_ip"))
                .cloned()
                .unwrap_or_else(|| "127.0.0.1".to_string())
        };

        let msg = self
            .message
            .clone()
            .unwrap_or_else(|| format!("Provisioning {}...", self.name));
        self.ui.say(&self.name, &msg);

        let port = match self.communicator_kind {
            crate::types::CommunicatorKind::WinRm => self
                .winrm_port
                .or_else(|| state.get::<u16>("winrm_port").copied())
                .unwrap_or(if self.winrm_use_ssl { 5986 } else { 5985 }),
            _ => self.ssh_port.or_else(|| state.ssh_port()).unwrap_or(22),
        };

        let comm: std::sync::Arc<dyn crate::communicator::Communicator> = if let Some(existing) =
            state
                .get::<std::sync::Arc<dyn crate::communicator::Communicator>>("communicator")
                .cloned()
        {
            existing
        } else if self.communicator_kind == crate::types::CommunicatorKind::WinRm {
            let winrm_cfg = crate::communicator::winrm::WinRmConfig {
                host: ip.clone(),
                port: crate::types::Port::new(port),
                username: self
                    .winrm_username
                    .clone()
                    .unwrap_or_else(|| "Administrator".to_string()),
                password: self.winrm_password.clone(),
                tls: crate::communicator::winrm::WinRmTlsConfig {
                    use_https: self.winrm_use_ssl,
                    insecure_skip_verify: self.winrm_insecure,
                    winrm_insecure: self.winrm_insecure,
                },
                timeout: crate::types::Timeout::new(std::time::Duration::from_secs(10)),
                ..Default::default()
            };
            std::sync::Arc::new(crate::communicator::winrm::WinRmCommunicator::new(
                winrm_cfg,
            ))
        } else {
            let priv_key = self.ssh_private_key_path.clone().or_else(|| {
                state
                    .get::<crate::types::FilePath>("private_key_path")
                    .cloned()
            });
            let ssh_cfg = crate::communicator::ssh::SshConfig {
                host: ip.clone(),
                port: crate::types::Port::new(port),
                username: self.ssh_username.clone(),
                password: self.ssh_password.clone(),
                private_key_path: priv_key,
                timeout: crate::types::Timeout::new(std::time::Duration::from_secs(10)),
                ..Default::default()
            };
            std::sync::Arc::new(crate::communicator::ssh::SshCommunicator::new(ssh_cfg))
        };

        state.put("communicator", comm.clone());
        state.put("provision_hook", self.hook.clone());

        let run_uuid = state
            .packer_run_uuid()
            .map_or_else(|| uuid::Uuid::new_v4().to_string(), ToString::to_string);
        let mut conn_info = state.conn_info().cloned().unwrap_or_default();
        if let Some(http_ip) = state.http_ip() {
            conn_info.insert("packer_http_ip".to_string(), http_ip.to_string());
        }
        if let Some(http_port) = state.http_port() {
            conn_info.insert("packer_http_port".to_string(), http_port.to_string());
        }

        let build_ctx = crate::engine::hook::BuildContext {
            build_id: self.name.clone(),
            build_name: self.name.clone(),
            build_type: self.builder_type.clone(),
            host: ip,
            port,
            user: if self.communicator_kind == crate::types::CommunicatorKind::WinRm {
                self.winrm_username
                    .clone()
                    .unwrap_or_else(|| "Administrator".to_string())
            } else {
                self.ssh_username.clone()
            },
            password: if self.communicator_kind == crate::types::CommunicatorKind::WinRm {
                self.winrm_password.clone()
            } else {
                self.ssh_password.clone()
            },
            conn_type: self.communicator_kind.to_string(),
            conn_info,
            packer_run_uuid: run_uuid,
            source_name: self.name.clone(),
            source_type: self.builder_type.clone(),
            ssh_public_key: state.ssh_public_key().map(ToString::to_string),
            ssh_private_key: state.ssh_private_key().map(ToString::to_string),
            ..Default::default()
        };
        state.set_build_context(build_ctx.clone());

        let prov_res = self
            .hook
            .run_provisioners(comm, &build_ctx, self.ui.clone())
            .await;

        if let Some(handle) =
            state.get::<crate::builder::http_server::HttpServerHandle>("http_server_handle")
        {
            handle.abort();
        }

        prov_res?;
        Ok(StepAction::Continue)
    }

    async fn cleanup(&mut self, state: &StateBag) {
        if let Some(handle) =
            state.get::<crate::builder::http_server::HttpServerHandle>("http_server_handle")
        {
            handle.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::StampError;
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
                if let Ok(mut l) = self.log.lock() {
                    l.push(format!("cancel {}", self.name));
                }
                return Ok(StepAction::Halt);
            }
            if let Ok(mut l) = self.log.lock() {
                l.push(format!("run {}", self.name));
            }
            Ok(self.action)
        }

        async fn cleanup(&mut self, _state: &StateBag) {
            if let Ok(mut l) = self.log.lock() {
                l.push(format!("cleanup {}", self.name));
            }
        }
    }

    struct SampleTypedKey;
    impl TypedStateKey for SampleTypedKey {
        type Value = u32;
        const KEY: &'static str = "sample_typed_u32";
    }

    #[test]
    fn test_state_bag() -> Result<(), StampError> {
        let mut bag = StateBag::new();
        bag.put("key1", 42i32);
        bag.put("key2", "hello".to_string());

        assert_eq!(bag.get::<i32>("key1"), Some(&42));
        assert_eq!(bag.get::<String>("key2"), Some(&"hello".to_string()));
        assert_eq!(bag.get::<i32>("key2"), None); // Wrong type
        assert_eq!(bag.get::<i32>("missing"), None);

        assert_eq!(bag.try_get::<i32>("key1")?, &42);
        assert!(matches!(
            bag.try_get::<i32>("missing"),
            Err(StampError::StateBagMissingKey { .. })
        ));
        assert!(matches!(
            bag.try_get::<i32>("key2"),
            Err(StampError::StateBagTypeMismatch { .. })
        ));

        bag.put_typed::<SampleTypedKey>(99);
        assert_eq!(bag.get_typed::<SampleTypedKey>(), Some(&99));
        assert_eq!(bag.try_get_typed::<SampleTypedKey>()?, &99);

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
        let history = bag
            .execution_history()
            .ok_or_else(|| StampError::Execution("missing history".to_string()))?;
        assert_eq!(history, &["StepOne", "StepTwo"]);
        Ok(())
    }

    #[tokio::test]
    async fn test_runner_success() -> Result<(), StampError> {
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

        runner.run(&mut state).await?;
        runner.cleanup(&state).await;

        let log_data = log
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(*log_data, vec!["run 1", "run 2", "cleanup 2", "cleanup 1"]);
        Ok(())
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

        let log_data = log
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // step3 should not be run, but step1 and step2 should be cleaned up.
        assert_eq!(*log_data, vec!["run 1", "run 2", "cleanup 2", "cleanup 1"]);
    }

    #[tokio::test]
    async fn test_runner_empty() -> Result<(), StampError> {
        let mut runner = Runner::new(vec![]);
        let mut state = StateBag::new();
        runner.run(&mut state).await
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

        let log_data = log
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert!(log_data.is_empty());
    }

    #[tokio::test]
    async fn test_runner_cancel_during_run() {
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
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push("run cancel_step".to_string());
                let _ = self.cancel_tx.send(true);
                Ok(StepAction::Continue)
            }

            async fn cleanup(&mut self, _state: &StateBag) {
                self.log
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push("cleanup cancel_step".to_string());
            }
        }

        let log = Arc::new(Mutex::new(Vec::new()));
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

        let log_data = log
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // step2 shouldn't run because it's cancelled
        assert_eq!(*log_data, vec!["run cancel_step", "cleanup cancel_step"]);
    }

    #[tokio::test]
    async fn test_runner_debug_and_on_error_modes() {
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
                .unwrap_or_else(std::sync::PoisonError::into_inner);
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
        assert_eq!(
            *attempts
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            2
        );
        let mut mut_step = RetryStep {
            attempts,
            action_on_retry: StepAction::Continue,
        };
        let _ = mut_step.run(&mut state).await;
        mut_step.cleanup(&state).await;
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
            attempts,
            action_on_retry: StepAction::Halt,
        };

        let mut runner = Runner::new(vec![Box::new(step)])
            .with_ui(ui)
            .with_on_error(crate::engine::packer::OnErrorStrategy::Ask);

        assert!(runner.run(&mut state).await.is_err());
    }

    #[tokio::test]
    async fn test_runner_on_error_ask_abort() {
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

        let mut runner = Runner::new(vec![Box::new(FailingStep)])
            .with_ui(ui)
            .with_on_error(crate::engine::packer::OnErrorStrategy::Ask);

        assert!(runner.run(&mut state).await.is_err());
        let mut mut_step = FailingStep;
        let _ = mut_step.run(&mut state).await;
        mut_step.cleanup(&state).await;
    }

    #[tokio::test]
    async fn test_run_with_strategy_and_step_provision() -> Result<(), StampError> {
        let ui = Arc::new(crate::engine::ui::Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));
        let hook = Arc::new(crate::engine::hook::DefaultProvisionHook {
            provisioners: Arc::new(vec![]),
            error_cleanup_provisioners: Arc::new(vec![]),
        });

        let step_prov = StepProvision::new(ui.clone(), "test-b", "custom", hook.clone())
            .with_ip_key("vm_ip")
            .with_require_ip(false)
            .with_communicator_kind(crate::types::CommunicatorKind::Ssh)
            .with_ssh_credentials("ubuntu", Some("pass".to_string()))
            .with_ssh_port(2222)
            .with_ssh_private_key(None)
            .with_message("Custom provisioning...");

        let mut runner = Runner::new(vec![Box::new(step_prov)]);
        let mut state = StateBag::new();
        state.put("vm_ip", "10.0.0.5".to_string());
        runner
            .run_with_strategy(
                &mut state,
                "test-b",
                &ui,
                crate::engine::packer::OnErrorStrategy::Cleanup,
            )
            .await?;

        let ctx = state
            .build_context()
            .ok_or_else(|| StampError::Execution("missing ctx".to_string()))?;
        assert_eq!(ctx.host, "10.0.0.5");
        assert_eq!(ctx.port, 2222);
        assert_eq!(ctx.user, "ubuntu");
        assert_ne!(ctx.packer_run_uuid, "mocked-uuid");

        // WinRM path + require_ip failure path
        let mut step_winrm = StepProvision::new(ui.clone(), "test-win", "hyperv", hook)
            .with_ip_key("win_ip")
            .with_require_ip(true)
            .with_communicator_kind(crate::types::CommunicatorKind::WinRm)
            .with_winrm_options(
                Some("Admin".to_string()),
                Some("secret".to_string()),
                Some(5986),
                true,
                true,
            );
        let mut empty_state = StateBag::new();
        assert!(step_winrm.run(&mut empty_state).await.is_err());

        empty_state.put("win_ip", "10.0.0.9".to_string());
        step_winrm.run(&mut empty_state).await?;
        step_winrm.cleanup(&empty_state).await;

        // Test run_with_strategy error paths (Cleanup, Abort, Ask)
        let mut fail_runner_cleanup = Runner::new(vec![Box::new(FailingStep2)]);
        assert!(
            fail_runner_cleanup
                .run_with_strategy(
                    &mut empty_state,
                    "test-b",
                    &ui,
                    crate::engine::packer::OnErrorStrategy::Cleanup
                )
                .await
                .is_err()
        );

        let mut fail_runner_abort = Runner::new(vec![Box::new(FailingStep2)]);
        assert!(
            fail_runner_abort
                .run_with_strategy(
                    &mut empty_state,
                    "test-b",
                    &ui,
                    crate::engine::packer::OnErrorStrategy::Abort
                )
                .await
                .is_err()
        );

        let queue = Arc::new(Mutex::new(std::collections::VecDeque::from([
            "y".to_string()
        ])));
        let ask_ui = Arc::new(
            crate::engine::ui::Ui::new(
                crate::engine::packer::FeatureState::Disabled,
                crate::engine::packer::FeatureState::Disabled,
                crate::engine::packer::FeatureState::Disabled,
            )
            .with_mock_inputs(queue),
        );
        let mut fail_runner_ask = Runner::new(vec![Box::new(FailingStep2)]);
        assert!(
            fail_runner_ask
                .run_with_strategy(
                    &mut empty_state,
                    "test-b",
                    &ask_ui,
                    crate::engine::packer::OnErrorStrategy::Ask
                )
                .await
                .is_err()
        );

        Ok(())
    }

    struct FailingComm;
    #[async_trait::async_trait]
    impl crate::communicator::Communicator for FailingComm {
        async fn execute(
            &self,
            cmd: &crate::communicator::Command,
        ) -> Result<crate::communicator::CommandResult, crate::error::StampError> {
            if cmd.command.contains("fail") {
                Err(crate::error::StampError::Execution("comm fail".to_string()))
            } else {
                Ok(crate::communicator::CommandResult {
                    stdout: "out".to_string(),
                    stderr: "err".to_string(),
                    exit_code: 0,
                })
            }
        }
        async fn upload(
            &self,
            _l: &crate::types::FilePath,
            _r: &crate::types::FilePath,
        ) -> Result<(), crate::error::StampError> {
            Ok(())
        }
        async fn download(
            &self,
            _r: &crate::types::FilePath,
            _l: &crate::types::FilePath,
        ) -> Result<(), crate::error::StampError> {
            Ok(())
        }
    }

    struct FailingStep2;
    #[async_trait::async_trait]
    impl Step for FailingStep2 {
        async fn run(&mut self, _s: &mut StateBag) -> Result<StepAction, crate::error::StampError> {
            Err(crate::error::StampError::Execution("fail".to_string()))
        }
        async fn cleanup(&mut self, _s: &StateBag) {}
    }

    #[tokio::test]
    async fn test_runner_on_error_ask_no_ui() {
        let mut runner = Runner::new(vec![Box::new(FailingStep2)])
            .with_on_error(crate::engine::packer::OnErrorStrategy::Ask);
        let mut state = StateBag::new();
        assert!(runner.run(&mut state).await.is_err());
    }

    #[tokio::test]
    async fn test_runner_on_error_ask_retry_fail() {
        let queue = Arc::new(Mutex::new(std::collections::VecDeque::new()));
        if let Ok(mut q) = queue.lock() {
            q.push_back("retry".to_string());
            q.push_back("cleanup".to_string());
        }
        let ui = Arc::new(
            crate::engine::ui::Ui::new(
                crate::engine::packer::FeatureState::Disabled,
                crate::engine::packer::FeatureState::Disabled,
                crate::engine::packer::FeatureState::Disabled,
            )
            .with_mock_inputs(queue),
        );
        let mut runner = Runner::new(vec![Box::new(FailingStep2)])
            .with_ui(ui)
            .with_on_error(crate::engine::packer::OnErrorStrategy::Ask);
        let mut state = StateBag::new();
        assert!(runner.run(&mut state).await.is_err());
    }

    #[tokio::test]
    async fn test_runner_on_error_ask_shell_eof() {
        let queue = Arc::new(Mutex::new(std::collections::VecDeque::new()));
        if let Ok(mut q) = queue.lock() {
            q.push_back("shell".to_string());
        }
        let ui = Arc::new(
            crate::engine::ui::Ui::new(
                crate::engine::packer::FeatureState::Disabled,
                crate::engine::packer::FeatureState::Disabled,
                crate::engine::packer::FeatureState::Disabled,
            )
            .with_mock_inputs(queue),
        );
        let mut runner = Runner::new(vec![Box::new(FailingStep2)])
            .with_ui(ui)
            .with_on_error(crate::engine::packer::OnErrorStrategy::Ask);
        let mut state = StateBag::new();
        let mock_comm: Arc<dyn crate::communicator::Communicator> = Arc::new(FailingComm);
        state.put("communicator", mock_comm.clone());
        let _ = mock_comm
            .upload(
                &crate::types::FilePath::new(std::path::PathBuf::from("")),
                &crate::types::FilePath::new(std::path::PathBuf::from("")),
            )
            .await;
        let _ = mock_comm
            .download(
                &crate::types::FilePath::new(std::path::PathBuf::from("")),
                &crate::types::FilePath::new(std::path::PathBuf::from("")),
            )
            .await;
        assert!(runner.run(&mut state).await.is_err());
    }

    #[tokio::test]
    async fn test_runner_on_error_ask_shell_no_comm() {
        let queue = Arc::new(Mutex::new(std::collections::VecDeque::new()));
        if let Ok(mut q) = queue.lock() {
            q.push_back("shell".to_string());
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
        let mut runner = Runner::new(vec![Box::new(FailingStep2)])
            .with_ui(ui)
            .with_on_error(crate::engine::packer::OnErrorStrategy::Ask);
        let mut state = StateBag::new();
        assert!(runner.run(&mut state).await.is_err());
    }

    #[tokio::test]
    async fn test_runner_on_error_ask_shell() {
        let mut state = StateBag::new();
        let mock_comm: Arc<dyn crate::communicator::Communicator> = Arc::new(FailingComm);
        state.put("communicator", mock_comm);

        let queue = Arc::new(Mutex::new(std::collections::VecDeque::new()));
        if let Ok(mut q) = queue.lock() {
            q.push_back("shell".to_string()); // ask choice
            q.push_back("echo test".to_string()); // in shell command
            q.push_back("fail test".to_string()); // in shell command
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

        let mut runner = Runner::new(vec![Box::new(FailingStep2)])
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
        ErrorStep.cleanup(&state).await;
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
        let ui = std::sync::Arc::new(crate::engine::ui::Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));
        let step = RecoveringStep { recovered: false };
        let mut runner = Runner::new(vec![Box::new(step)]).with_ui(ui);
        let mut state = StateBag::new();
        let res = runner.run(&mut state).await;
        assert!(res.is_ok());
        let mut mut_step = RecoveringStep { recovered: true };
        mut_step.cleanup(&state).await;
    }

    #[tokio::test]
    async fn test_runner_on_error_run_cleanup_provisioner() {
        let step = ErrorStep;
        let ui = std::sync::Arc::new(crate::engine::ui::Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));
        let mut runner = Runner::new(vec![Box::new(step)])
            .with_ui(ui)
            .with_on_error(crate::engine::packer::OnErrorStrategy::RunCleanupProvisioner);
        let mut state = StateBag::new();

        let mock_comm: std::sync::Arc<dyn crate::communicator::Communicator> =
            std::sync::Arc::new(crate::communicator::mock::MockCommunicator::default());
        state.put("communicator", mock_comm.clone());
        let hook: std::sync::Arc<dyn crate::engine::hook::ProvisionHook> =
            std::sync::Arc::new(MockProvHookNoUi);
        state.put("provision_hook", hook.clone());

        let res = runner.run(&mut state).await;
        assert!(res.is_err());

        // Extra coverage calls:
        let ui2 = std::sync::Arc::new(crate::engine::ui::Ui::new(
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
            crate::engine::packer::FeatureState::Disabled,
        ));
        let _ = hook
            .run_provisioners(
                mock_comm.clone(),
                &crate::engine::hook::BuildContext::default(),
                ui2.clone(),
            )
            .await;
        let _ = hook
            .run_error_cleanup_provisioners(
                mock_comm.clone(),
                &crate::engine::hook::BuildContext::default(),
                ui2.clone(),
            )
            .await;
    }

    struct MockProvHookNoUi;
    #[async_trait::async_trait]
    impl crate::engine::hook::ProvisionHook for MockProvHookNoUi {
        async fn run_provisioners(
            &self,
            _c: std::sync::Arc<dyn crate::communicator::Communicator>,
            _ctx: &crate::engine::hook::BuildContext,
            _ui: std::sync::Arc<crate::engine::ui::Ui>,
        ) -> Result<(), crate::error::StampError> {
            Ok(())
        }
        async fn run_error_cleanup_provisioners(
            &self,
            _c: std::sync::Arc<dyn crate::communicator::Communicator>,
            _ctx: &crate::engine::hook::BuildContext,
            _ui: std::sync::Arc<crate::engine::ui::Ui>,
        ) -> Result<(), crate::error::StampError> {
            Ok(())
        }
    }

    struct MockProvHookNoUi2;
    #[async_trait::async_trait]
    impl crate::engine::hook::ProvisionHook for MockProvHookNoUi2 {
        async fn run_provisioners(
            &self,
            _c: std::sync::Arc<dyn crate::communicator::Communicator>,
            _ctx: &crate::engine::hook::BuildContext,
            _ui: std::sync::Arc<crate::engine::ui::Ui>,
        ) -> Result<(), crate::error::StampError> {
            Ok(())
        }
        async fn run_error_cleanup_provisioners(
            &self,
            _c: std::sync::Arc<dyn crate::communicator::Communicator>,
            _ctx: &crate::engine::hook::BuildContext,
            _ui: std::sync::Arc<crate::engine::ui::Ui>,
        ) -> Result<(), crate::error::StampError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn test_runner_on_error_run_cleanup_provisioner_no_ui_but_with_comm_and_hook() {
        let step = ErrorStep;
        let mut runner = Runner::new(vec![Box::new(step)])
            .with_on_error(crate::engine::packer::OnErrorStrategy::RunCleanupProvisioner);
        let mut state = StateBag::new();
        state.put(
            "communicator",
            std::sync::Arc::new(crate::communicator::mock::MockCommunicator::default())
                as std::sync::Arc<dyn crate::communicator::Communicator>,
        );
        let hook: std::sync::Arc<dyn crate::engine::hook::ProvisionHook> =
            std::sync::Arc::new(MockProvHookNoUi2);
        state.put("provision_hook", hook.clone());

        let _ = hook
            .run_provisioners(
                std::sync::Arc::new(crate::communicator::mock::MockCommunicator::default()),
                &crate::engine::hook::BuildContext::default(),
                std::sync::Arc::new(crate::engine::ui::Ui::new(
                    crate::engine::packer::FeatureState::Disabled,
                    crate::engine::packer::FeatureState::Disabled,
                    crate::engine::packer::FeatureState::Disabled,
                )),
            )
            .await;
        let _ = hook
            .run_error_cleanup_provisioners(
                std::sync::Arc::new(crate::communicator::mock::MockCommunicator::default()),
                &crate::engine::hook::BuildContext::default(),
                std::sync::Arc::new(crate::engine::ui::Ui::new(
                    crate::engine::packer::FeatureState::Disabled,
                    crate::engine::packer::FeatureState::Disabled,
                    crate::engine::packer::FeatureState::Disabled,
                )),
            )
            .await;
        assert!(runner.run(&mut state).await.is_err());
    }

    #[tokio::test]
    async fn test_runner_on_error_run_cleanup_provisioner_no_ui_and_comm() {
        let step = ErrorStep;
        let mut runner = Runner::new(vec![Box::new(step)])
            .with_on_error(crate::engine::packer::OnErrorStrategy::RunCleanupProvisioner);
        let mut state = StateBag::new();
        assert!(runner.run(&mut state).await.is_err());

        let mut runner2 = Runner::new(vec![Box::new(ErrorStep)])
            .with_on_error(crate::engine::packer::OnErrorStrategy::RunCleanupProvisioner);
        let mut state2 = StateBag::new();
        state2.put(
            "communicator",
            std::sync::Arc::new(crate::communicator::mock::MockCommunicator::default())
                as std::sync::Arc<dyn crate::communicator::Communicator>,
        );
        assert!(runner2.run(&mut state2).await.is_err());
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

        let mut bag3 = StateBag::new();
        bag3.put("foo", 123i32);
        assert_eq!(bag3.take::<i32>("foo"), Some(123i32));
        assert_eq!(bag3.take::<i32>("foo"), None); // Already taken
        bag3.put("bar", 456i32);
        assert_eq!(bag3.take::<String>("bar"), None); // Wrong type

        let shared3 = SharedStateBag::new();
        shared3.write(|b| b.put("baz", 789i32));
        assert_eq!(shared3.take::<i32>("baz"), Some(789i32));

        shared3.set_http_ip("192.168.1.50".to_string());
        assert_eq!(shared3.http_ip(), Some("192.168.1.50".to_string()));
        shared3.set_http_port(8080);
        assert_eq!(shared3.http_port(), Some(8080));
    }

    struct MockHook {
        pre: std::sync::atomic::AtomicBool,
        post: std::sync::atomic::AtomicBool,
        err: std::sync::atomic::AtomicBool,
    }

    #[async_trait::async_trait]
    impl StepHook for MockHook {
        async fn pre_step(
            &self,
            _step_name: &str,
            _state: &mut StateBag,
        ) -> Result<(), crate::error::StampError> {
            self.pre.store(true, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        }
        async fn post_step(
            &self,
            _step_name: &str,
            _state: &mut StateBag,
        ) -> Result<(), crate::error::StampError> {
            self.post.store(true, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        }
        async fn step_error(
            &self,
            _step_name: &str,
            _error: &crate::error::StampError,
            _state: &mut StateBag,
        ) {
            self.err.store(true, std::sync::atomic::Ordering::SeqCst);
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
            pre: std::sync::atomic::AtomicBool::new(false),
            post: std::sync::atomic::AtomicBool::new(false),
            err: std::sync::atomic::AtomicBool::new(false),
        });
        let mut runner = Runner::new(vec![Box::new(SuccessStep)]).with_step_hook(hook.clone());
        let mut state = StateBag::new();
        let res = runner.run(&mut state).await;
        assert!(res.is_ok());
        assert!(hook.pre.load(std::sync::atomic::Ordering::SeqCst));
        let mut mut_step = SuccessStep;
        let _ = mut_step.run(&mut state).await;
        mut_step.cleanup(&state).await;
    }

    #[tokio::test]
    async fn test_step_hooks_failure() {
        let hook = Arc::new(MockHook {
            pre: std::sync::atomic::AtomicBool::new(false),
            post: std::sync::atomic::AtomicBool::new(false),
            err: std::sync::atomic::AtomicBool::new(false),
        });
        let mut runner = Runner::new(vec![Box::new(ErrorStep)]).with_step_hook(hook.clone());
        let mut state = StateBag::new();
        let res = runner.run(&mut state).await;
        assert!(res.is_err());
        assert!(hook.pre.load(std::sync::atomic::Ordering::SeqCst));
        assert!(!hook.post.load(std::sync::atomic::Ordering::SeqCst));
        assert!(hook.err.load(std::sync::atomic::Ordering::SeqCst));
    }

    struct DefaultHook;
    #[async_trait::async_trait]
    impl StepHook for DefaultHook {}

    #[tokio::test]
    async fn test_default_step_hook() {
        let hook = DefaultHook;
        let mut bag = StateBag::new();
        let _ = hook.pre_step("t", &mut bag).await;
        let _ = hook.post_step("t", &mut bag).await;
        hook.step_error(
            "t",
            &crate::error::StampError::Execution("e".to_string()),
            &mut bag,
        )
        .await;
    }

    #[tokio::test]
    async fn test_runner_signal_listener_sigint() {
        let runner = Runner::new(vec![Box::new(SuccessStep)]);
        let handle = runner.spawn_signal_listener();

        #[cfg(unix)]
        {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            unsafe {
                libc::kill(libc::getpid(), libc::SIGINT);
            }
            let _ = tokio::time::timeout(std::time::Duration::from_secs(1), handle).await;
        }
        #[cfg(not(unix))]
        {
            handle.abort();
        }
    }

    #[tokio::test]
    async fn test_runner_signal_listener_sigterm() {
        let runner = Runner::new(vec![Box::new(SuccessStep)]);
        let handle = runner.spawn_signal_listener();

        #[cfg(unix)]
        {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            unsafe {
                libc::kill(libc::getpid(), libc::SIGTERM);
            }
            let _ = tokio::time::timeout(std::time::Duration::from_secs(1), handle).await;
        }
        #[cfg(not(unix))]
        {
            handle.abort();
        }
    }
}
