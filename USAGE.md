# Stamp Usage & Reference Guide

Stamp is a high-performance, Rust-native machine image building framework and CLI providing 100% functional parity with **HashiCorp Packer**. Under the hood, Stamp acts as an orchestrator for the `libstamp` core library and provides complete wire compatibility with upstream `packer-plugin-*` external binaries.

---

## Table of Contents

1. [Installation](#installation)
2. [CLI Commands Overview](#cli-commands-overview)
   - [stamp build](#stamp-build)
   - [stamp console](#stamp-console)
   - [stamp validate](#stamp-validate)
   - [stamp fmt](#stamp-fmt)
   - [stamp fix](#stamp-fix)
   - [stamp hcl2-upgrade](#stamp-hcl2-upgrade)
   - [stamp init](#stamp-init)
   - [stamp plugins](#stamp-plugins)
   - [stamp inspect](#stamp-inspect)
   - [stamp test](#stamp-test)
   - [stamp autocomplete](#stamp-autocomplete)
   - [stamp version](#stamp-version)
3. [Environment Variables](#environment-variables)
4. [Interactive Debugging & Error Handling](#interactive-debugging--error-handling)
5. [Machine-Readable Output](#machine-readable-output)
6. [HCP Packer & Policy Enforcement](#hcp-packer--policy-enforcement)
7. [Authoring Standalone Plugins (SDK)](#authoring-standalone-plugins-sdk)
8. [Embedding `libstamp` in Rust Projects](#embedding-libstamp-in-rust-projects)
9. [Auditing and Verification](#auditing-and-verification)

---

## Installation

```sh
# Clone repository
git clone https://github.com/your-org/stamp.git
cd stamp

# Install the binary locally
cargo install --path ./stamp
```

Ensure `~/.cargo/bin` is in your system `$PATH` to use `stamp` natively.

---

## CLI Commands Overview

### `stamp build`

Executes build definitions described in HCL2 (`.pkr.hcl`) or legacy JSON (`.json`) templates:

```sh
# Basic build
stamp build template.pkr.hcl

# Target specific builders
stamp build -only=amazon-ebs.ubuntu template.pkr.hcl

# Exclude specific builders
stamp build -except=qemu.debian template.pkr.hcl

# Supply variables via flags or variable files
stamp build -var "region=us-west-2" -var-file=vars.pkrvars.hcl template.pkr.hcl

# Interactive step-by-step debugging (pauses after each step)
stamp build -debug template.pkr.hcl

# Configurable error response strategies
stamp build -on-error=ask template.pkr.hcl
stamp build -on-error=abort template.pkr.hcl
stamp build -on-error=run-cleanup-provisioner template.pkr.hcl

# Force build even if output artifacts already exist
stamp build -force template.pkr.hcl

# Limit parallel builder concurrency
stamp build -parallel-builds=2 template.pkr.hcl

# Machine-readable CSV output
stamp build -machine-readable template.pkr.hcl
```

### `stamp console`

Interactive Read-Eval-Print Loop (REPL) for evaluating HCL2 expressions, variables, locals, functions, and data source lookups in real time:

```sh
stamp console template.pkr.hcl
# In the REPL:
> var.region
"us-east-1"
> upper("hello")
"HELLO"
> [for s in ["a", "b"]: upper(s)]
["A", "B"]
```

### `stamp validate`

Statically verifies syntax, block schemas, and variables without executing builds:

```sh
# Full validation including data source evaluations
stamp validate -evaluate-datasources template.pkr.hcl

# Structural syntax-only check (skips plugin lookups)
stamp validate -syntax-only template.pkr.hcl

# Control warnings on undeclared variables
stamp validate -warn-on-undeclared-var template.pkr.hcl
stamp validate -no-warn-undeclared-var template.pkr.hcl
```

### `stamp fmt`

Rewrites HCL2 configuration files to canonical format and style:

```sh
# Format single file in place
stamp fmt template.pkr.hcl

# Recursively format directory
stamp fmt -recursive ./packer/

# Check formatting in CI without modifying files (exit code 1 if diffs found)
stamp fmt -check template.pkr.hcl

# Output unified diff of proposed changes
stamp fmt -diff template.pkr.hcl
```

### `stamp fix`

Rewrites backwards-compatible historical JSON templates, automatically resolving deprecated keys and builder/provisioner names:

```sh
stamp fix legacy-template.json > updated-template.json
```

### `stamp hcl2-upgrade`

Converts legacy JSON templates into modern, idiomatically structured HCL2 configuration files:

```sh
stamp hcl2-upgrade legacy-template.json
# Generates legacy-template.pkr.hcl with variable, source, and build blocks
```

### `stamp init`

Discovers required plugins defined in `packer` blocks and downloads them from the HashiCorp plugin registry or GitHub releases:

```sh
# Download and install required plugins
stamp init template.pkr.hcl

# Force reinstall or upgrade to latest matching versions
stamp init -upgrade template.pkr.hcl
```

### `stamp plugins`

Manages external plugin binaries installed on the host system:

```sh
# List all discovered installed plugins
stamp plugins installed

# Inspect template and display plugin dependency requirements
stamp plugins required template.pkr.hcl

# Install an individual plugin directly
stamp plugins install github.com/hashicorp/amazon v1.2.0

# Remove an installed plugin
stamp plugins remove github.com/hashicorp/amazon
```

### `stamp inspect`

Analyzes template structures and prints detailed summaries:

```sh
stamp inspect template.pkr.hcl
# Outputs defined variables, default values, descriptions, sources, and provisioners
```

### `stamp test`

Executes automated assertion suites for configuration templates, emitting standard test summaries or JUnit XML reports for CI:

```sh
stamp test --junit-xml=results.xml template.pkr.hcl
```

### `stamp autocomplete`

Generates shell completion scripts:

```sh
stamp autocomplete --shell=bash >> ~/.bashrc
stamp autocomplete --shell=zsh >> ~/.zshrc
stamp autocomplete --shell=fish > ~/.config/fish/completions/stamp.fish
```

### `stamp version`

Displays version details, platform architecture, and optionally checks for updates:

```sh
stamp version -v
stamp version --check-updates
```

---

## Environment Variables

Stamp fully honors all standard HashiCorp Packer environment variables:

| Variable | Description |
| :--- | :--- |
| `PACKER_LOG=1` | Enables detailed debug logging to stderr |
| `PACKER_LOG_PATH=/path/to/log` | Directs execution logs to a specified file |
| `PACKER_PLUGIN_PATH` | Colon-separated directory search list for plugins |
| `PACKER_NO_COLOR=1` | Disables ANSI escape codes in output |
| `PKR_VAR_<name>` | Sets template variable `var.<name>` from environment |
| `HCP_CLIENT_ID` | OAuth2 Client ID for HashiCorp Cloud Platform integration |
| `HCP_CLIENT_SECRET` | OAuth2 Client Secret for HCP integration |
| `HCP_ORGANIZATION_ID` | HCP Organization ID |
| `HCP_PROJECT_ID` | HCP Project ID |

---

## Interactive Debugging & Error Handling

When running with `-debug`:
1. Stamp inserts pause checkpoints before and after every lifecycle step.
2. The user is prompted: `"Pausing after run of step ... Press enter to continue."`
3. Builders keep temporary infrastructure active until manual confirmation.

With `-on-error=ask`:
1. If a step fails, teardown pauses immediately.
2. The user can select:
   - `[c] clean up`: Executes step cleanup handlers and terminates.
   - `[a] abort`: Exits immediately without cleaning up resources (leaving VMs/instances intact for post-mortem debugging).
   - `[r] retry`: Re-runs the failed step directly.

With `-on-error=run-cleanup-provisioner`:
- Provisioners flagged with `error-cleanup = true` are invoked upon failure before teardown.

---

## Machine-Readable Output

By passing `-machine-readable`, Stamp emits structured CSV records:

```text
<timestamp>,<target>,<type>,<data...>
```

Key record types:
- `ui,say,<message>`
- `ui,message,<message>`
- `ui,error,<message>`
- `artifact-count,<count>`
- `artifact,<builder-idx>,builder-id,<id>`
- `artifact,<builder-idx>,id,<artifact-id>`
- `artifact,<builder-idx>,string,<human-readable>`
- `artifact,<builder-idx>,file,<file-idx>,<path>`

---

## HCP Packer & Policy Enforcement

Stamp provides native support for HashiCorp Cloud Platform (HCP) Packer and policy evaluation engines:

- **HCP Lineage Tracking:** Automatically registers iterations, channels (`channels = ["staging", "production"]`), Git metadata, and publishes created artifact IDs.
- **Base Image Revocation Check:** Ensures that builds fail if the base image artifact has been revoked in HCP.
- **Sentinel & Open Policy Agent (OPA):** Templates can be evaluated against Sentinel rules or Rego policies before builds begin or artifacts publish.
  - Bypass with `--skip-enforcement` when running authorized override builds.

---

## Authoring Standalone Plugins (SDK)

The `stamp-plugin-sdk` crate enables authoring custom external plugins in pure Rust:

```rust
use stamp_plugin_sdk::{
    Builder, Artifact, Communicator, Ui,
    serve_plugin, PluginServerBuilder, StampError
};
use async_trait::async_trait;

#[derive(Default)]
pub struct MyCustomBuilder;

#[async_trait]
impl Builder for MyCustomBuilder {
    fn name(&self) -> &'static str { "custom-builder" }
    async fn prepare(&mut self, _config: &std::collections::HashMap<String, String>) -> Result<(), StampError> {
        Ok(())
    }
    async fn run(&mut self, _ui: &mut dyn Ui, _comm: &mut dyn Communicator) -> Result<Box<dyn Artifact>, StampError> {
        // Build logic...
        Ok(Box::new(libstamp::artifact::MockArtifact::new("custom-artifact-123")))
    }
    async fn cancel(&mut self) -> Result<(), StampError> { Ok(()) }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    serve_plugin!(
        builder => ("custom-builder", MyCustomBuilder::default())
    );
    Ok(())
}
```

---

## Embedding `libstamp` in Rust Projects

```rust
use libstamp::engine::packer::build_concurrently;
use libstamp::builder::null::{NullBuilder, NullConfig};

#[tokio::main]
async fn main() -> Result<(), libstamp::error::StampError> {
    let builder1 = Box::new(NullBuilder::new(NullConfig {
        name: "test-builder-1".into()
    }));
    let builder2 = Box::new(NullBuilder::new(NullConfig {
        name: "test-builder-2".into()
    }));

    build_concurrently(vec![builder1, builder2]).await?;
    Ok(())
}
```

---

## Auditing and Verification

```sh
# Run tests and verify 100% rustdoc coverage (fails on warnings)
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps

# Verify code coverage across lines, branches, and functions
cargo llvm-cov --workspace

# Run the strict linting suite
cargo clippy --workspace --all-targets --all-features -- -D warnings -D clippy::pedantic

# Grounding parity assertion against official HashiCorp Packer reference schema
cargo run -p stamp --bin grounding
```
