# Stamp Usage & Reference Guide

Stamp is a high-performance, Rust-native machine image building framework and CLI orchestrator providing 100% functional and wire-level parity with **HashiCorp Packer**. Under the hood, Stamp acts as an orchestrator for the `libstamp` core engine, delegates all HCL2 parsing, formatting, dynamic blocks, and expression evaluations to [**`hashicorp-configuration-language-rs`**](https://github.com/SamuelMarks/hashicorp-configuration-language-rs), and provides seamless compatibility with upstream `packer-plugin-*` external binaries.

---

## Table of Contents

1. [Installation & Setup](#1-installation--setup)
2. [CLI Command Reference](#2-cli-command-reference)
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
3. [Environment Variables Reference](#3-environment-variables-reference)
4. [Interactive Debugging & Error Handling](#4-interactive-debugging--error-handling)
   - [Step-by-Step Debugging (-debug)](#step-by-step-debugging--debug)
   - [Error Response Strategies (-on-error)](#error-response-strategies--on-error)
   - [Signal Handling & Graceful Cancellation](#signal-handling--graceful-cancellation)
5. [Machine-Readable Output Formats](#5-machine-readable-output-formats)
6. [Enterprise Governance: HCP Packer & Policy Enforcement](#6-enterprise-governance-hcp-packer--policy-enforcement)
   - [HCP Packer Registry](#hcp-packer-registry)
   - [Policy Enforcement (OPA Rego & Sentinel)](#policy-enforcement-opa-rego--sentinel)
7. [End-to-End Real-World Examples](#7-end-to-end-real-world-examples)
   - [Multi-Target HCL2 Production Template](#multi-target-hcl2-production-template)
   - [Migrating Legacy JSON Templates](#migrating-legacy-json-templates)
8. [Authoring Standalone Plugins (SDK)](#8-authoring-standalone-plugins-sdk)
9. [Embedding libstamp in Rust Applications](#9-embedding-libstamp-in-rust-applications)
10. [Auditing, Linting & Verification](#10-auditing-linting--verification)

---

## 1. Installation & Setup

### Building from Source

```sh
# Clone repository
git clone https://github.com/SamuelMarks/stamp.git
cd stamp

# Build optimized binary
cargo build --release

# Install locally into ~/.cargo/bin
cargo install --path ./stamp
```

Ensure `~/.cargo/bin` is in your shell's `$PATH`:

```sh
export PATH="$HOME/.cargo/bin:$PATH"
```

### Shell Autocompletion Setup

Stamp supports autocompletion generation for all major shells:

```sh
# Automatic profile installation (detects user shell)
stamp --autocomplete-install

# Or manual generation for specific shells:
# Bash
stamp autocomplete --shell=bash >> ~/.bashrc

# Zsh
stamp autocomplete --shell=zsh > "${fpath[1]}/_stamp"

# Fish
stamp autocomplete --shell=fish > ~/.config/fish/completions/stamp.fish

# PowerShell
stamp autocomplete --shell=powershell >> $PROFILE
```

To remove autocompletions:
```sh
stamp --autocomplete-uninstall
```

---

## 2. CLI Command Reference

### `stamp build`
**Alias:** `stamp b`

Executes build definitions described in HCL2 (`.pkr.hcl`) or legacy JSON (`.json`) templates:

```sh
stamp build [options] <template_path ...>
```

#### Flags and Options

| Flag | Type | Description |
| :--- | :--- | :--- |
| `-only=<targets>` | String | Comma-separated list of build sources to execute (e.g., `-only=amazon-ebs.ubuntu,qemu.debian`). |
| `-except=<targets>` | String | Comma-separated list of build sources to exclude from execution. |
| `-var "key=value"` | String | Sets an individual template variable. Can be passed multiple times. |
| `-var-file=<path>` | String | Path to HCL2 (`.pkrvars.hcl`) or JSON file containing user variables. |
| `-debug` | Flag | Disables parallelization and pauses before and after each lifecycle step. |
| `-on-error=<strategy>` | String | Action to take upon step failure: `cleanup` (default), `abort`, `ask`, or `run-cleanup-provisioner`. |
| `-parallel-builds=<n>` | Integer | Restricts maximum concurrent builder tasks (default: unlimited). |
| `-force` | Flag | Forces image builds even if target artifacts (AMIs, disk files) already exist, overwriting them. |
| `-timestamp-ui` | Flag | Prepends UTC timestamps to terminal UI log output lines. |
| `-skip-enforcement` | Flag | Skips HCP Packer policy checks (OPA / Sentinel) for emergency maintenance runs. |
| `-use-sequential-evaluation`| Flag | Evaluates data sources and dynamic blocks sequentially instead of concurrently. |
| `-color=false` | Bool | Suppresses ANSI terminal colors (equivalent to `PACKER_NO_COLOR=1`). |
| `-machine-readable` | Flag | Emits machine-parsable CSV stream to stdout. |

#### Examples

```sh
# Simple build
stamp build template.pkr.hcl

# Build with variable overrides and custom variable file
stamp build -var "environment=production" -var-file=prod.pkrvars.hcl template.pkr.hcl

# Interactive step-by-step debug run with pause prompts
stamp build -debug -on-error=ask template.pkr.hcl

# Build only AWS EBS and restrict parallel builds to 1
stamp build -only=amazon-ebs.ubuntu -parallel-builds=1 template.pkr.hcl
```

---

### `stamp console`
**Alias:** `stamp c`

Launches an interactive Read-Eval-Print Loop (REPL) for testing and debugging HCL2 variable values, locals, built-in functions, and data source lookups in real time against a template context. Evaluated using the `Evaluator` and standard library function table from [**`hashicorp-configuration-language-rs`**](https://github.com/SamuelMarks/hashicorp-configuration-language-rs):

```sh
stamp console [options] <template_path>
```

#### Interactive REPL Session Example

```sh
$ stamp console template.pkr.hcl
Stamp HCL2 Interactive Console. Type expressions to evaluate, or Ctrl+D to exit.
> var.region
"us-west-2"

> local.timestamp
"2026-09-09T14:30:00Z"

> upper("nginx-base")
"NGINX-BASE"

> [for s in ["web", "db"]: format("%s-%s", var.environment, s)]
[
  "production-web",
  "production-db",
]

> fileexists("scripts/bootstrap.sh")
true

> bcrypt("supersecret", 10)
"$2b$10$e8wF3QvQe1w6...hash"
```

---

### `stamp validate`
**Alias:** `stamp v`

Statically analyzes template syntax, block hierarchies, variable references, and configuration schemas without starting hypervisors or provisioning cloud infrastructure:

```sh
stamp validate [options] <template_path>
```

#### Flags and Options

| Flag | Type | Description |
| :--- | :--- | :--- |
| `-syntax-only` | Flag | Verifies syntactic validity of HCL2/JSON without validating plugin schemas. |
| `-evaluate-datasources` | Flag | Performs live lookups against cloud APIs and Vault/Consul to validate data sources. |
| `-warn-on-undeclared-var` | Flag | Emits warnings if variables passed via CLI or env files are not defined in the template. |
| `-no-warn-undeclared-var` | Flag | Suppresses warnings for undeclared variables. |
| `-only=<targets>` | String | Validates only the specified build sources. |
| `-except=<targets>` | String | Excludes specific build sources from validation. |

```sh
# Strict validation including remote data source lookups
stamp validate -evaluate-datasources template.pkr.hcl

# Quick syntax-only verification in pre-commit hooks
stamp validate -syntax-only template.pkr.hcl
```

---

### `stamp fmt`

Rewrites HCL2 configuration files to canonical formatting and indentation style, delegated directly to the CST formatter in [**`hashicorp-configuration-language-rs`**](https://github.com/SamuelMarks/hashicorp-configuration-language-rs):

```sh
stamp fmt [options] <target_path ...>
```

#### Flags and Options

| Flag | Type | Description |
| :--- | :--- | :--- |
| `-check` | Flag | Non-zero exit code (1) if files require formatting. Perfect for CI linter checks. |
| `-diff` | Flag | Emits unified diff of formatting changes to stdout without modifying files. |
| `-recursive` | Flag | Recursively formats all `.pkr.hcl` and `.pkrvars.hcl` files in directory hierarchies. |
| `-write=false` | Flag | Disables writing changes back to files (defaults to true). |

```sh
# Format a directory recursively
stamp fmt -recursive ./packer/

# CI verification gate
stamp fmt -check -recursive ./packer/

# Preview changes with unified diff
stamp fmt -diff template.pkr.hcl
```

---

### `stamp fix`

Automatically detects and migrates deprecated syntax, renamed builders, obsolete configuration keys, and outdated provisioner names in legacy JSON templates:

```sh
stamp fix [-validate] legacy-template.json > updated-template.json
```

---

### `stamp hcl2-upgrade`

Performs end-to-end mechanical migration of historical JSON templates into modern, idiomatically structured HCL2 configuration files (`.pkr.hcl`):

```sh
stamp hcl2-upgrade legacy-template.json
# Generates legacy-template.pkr.hcl with variable, source, and build blocks
```

---

### `stamp init`
**Alias:** `stamp i`

Reads the `packer.required_plugins` block in your template and downloads, verifies, and installs external plugin binaries into the local plugin directory (`~/.packer.d/plugins`):

```sh
stamp init [options] <template_path>
```

#### Flags and Options
- `-upgrade`: Upgrades already-installed plugins to the newest matching version constraint.
- `-force`: Re-downloads and overwrites existing installed plugins.

```sh
stamp init -upgrade template.pkr.hcl
```

---

### `stamp plugins`

Subcommands for discovering and maintaining external plugin binaries:

```sh
# List all plugins installed locally
stamp plugins installed

# List all plugins required by a given template
stamp plugins required template.pkr.hcl

# Manually install a plugin directly from GitHub or a registry
stamp plugins install github.com/hashicorp/amazon v1.3.1

# Remove an installed plugin
stamp plugins remove github.com/hashicorp/amazon
```

---

### `stamp inspect`

Analyzes template structures and emits a human-readable or machine-readable breakdown of defined variables, locals, sources, provisioners, and post-processors:

```sh
stamp inspect [-machine-readable] template.pkr.hcl
```

---

### `stamp test`

Executes automated assertion test suites declared in template `test` blocks, ensuring build configuration parameters satisfy structural invariants:

```sh
stamp test [--verbose] [--junit-xml=results.xml] template.pkr.hcl
```

---

### `stamp autocomplete`

Generates shell completion scripts for terminal tab-completion:

```sh
stamp autocomplete --shell=zsh
```

---

### `stamp version`

Displays version details, architecture platform, and checks for updates:

```sh
# Detailed component versions
stamp version -v

# Check for new updates
stamp version --check-updates
```

---

## 3. Environment Variables Reference

Stamp fully adheres to standard HashiCorp Packer conventions and supports the following environment variables:

| Variable | Description |
| :--- | :--- |
| `PACKER_LOG=1` | Enables detailed internal diagnostics output to stderr. |
| `PACKER_LOG_PATH=/path/to/log` | Routes debug logging to a dedicated file instead of stderr. |
| `PACKER_PLUGIN_PATH` | Colon-separated (`:`) list of directories searched for external plugins. |
| `PACKER_NO_COLOR=1` | Disables terminal ANSI colors across all command outputs. |
| `PKR_VAR_<variable_name>` | Dynamically populates template variable `var.<variable_name>`. |
| `HCP_CLIENT_ID` | HashiCorp Cloud Platform service principal Client ID. |
| `HCP_CLIENT_SECRET` | HashiCorp Cloud Platform service principal Client Secret. |
| `HCP_ORGANIZATION_ID` | HCP Organization ID for artifact lineage tracking. |
| `HCP_PROJECT_ID` | HCP Project ID for artifact lineage tracking. |

---

## 4. Interactive Debugging & Error Handling

### Step-by-Step Debugging (`-debug`)

Passing `-debug` disables parallel execution and pauses after each lifecycle step:

```sh
stamp build -debug ubuntu.pkr.hcl
```

Terminal interaction:
```text
==> amazon-ebs.ubuntu: Creating temporary security group...
==> amazon-ebs.ubuntu: Launching source AWS EC2 instance...
==> Pausing after run of step: StepRunSourceInstance. Press enter to continue.
```
While paused, target virtual machines, cloud instances, and security groups remain running. You can open a separate terminal and connect directly via SSH or inspect hypervisor state.

### Error Response Strategies (`-on-error`)

Configure the failure behavior with `-on-error=<strategy>`:

1. **`cleanup`** *(default)*: Immediately runs step cleanup handlers in reverse order to destroy cloud instances, delete temporary keypairs, and release storage volumes.
2. **`abort`**: Halts immediately without executing cleanups, leaving VMs and disks intact for deep debugging.
3. **`ask`**: Interactively prompts the operator upon any error:
   ```text
   ==> amazon-ebs.ubuntu: Error provisioning: script exited with status 1
   ==> amazon-ebs.ubuntu: What would you like to do?
   [c] clean up: Run reverse cleanup handlers and exit
   [a] abort: Keep running resources and exit immediately
   [r] retry: Re-run the failed provisioner or step
   Choice [c/a/r]:
   ```
4. **`run-cleanup-provisioner`**: Triggers any provisioners configured with `error-cleanup = true` before teardown commences.

### Signal Handling & Graceful Cancellation

Stamp traps OS signals (`SIGINT` / Ctrl+C, `SIGTERM`) asynchronously:
- **First Ctrl+C:** Halts active provisioners and commences orderly step rollback and cleanup.
- **Second Ctrl+C:** Triggers an immediate hard abort and exits without waiting for remote cleanup.

---

## 5. Machine-Readable Output Formats

For CI/CD systems, Stamp can emit structured CSV event records by appending `-machine-readable`:

```sh
stamp build -machine-readable template.pkr.hcl
```

### Record Format

```text
<unix_timestamp>,<target>,<type>,<data...>
```

#### Common Record Types

| Type | Format | Meaning |
| :--- | :--- | :--- |
| `ui,say` | `<ts>,<target>,ui,say,<message>` | Informational progress notification. |
| `ui,error` | `<ts>,<target>,ui,error,<error_text>` | Error or failure notification. |
| `artifact-count` | `<ts>,<target>,artifact-count,<count>` | Total number of artifacts generated. |
| `artifact,builder-id` | `<ts>,<target>,artifact,<idx>,builder-id,<id>` | Unique builder identifier. |
| `artifact,id` | `<ts>,<target>,artifact,<idx>,id,<image_id>` | Created image identifier (e.g. `ami-012345`). |
| `artifact,string` | `<ts>,<target>,artifact,<idx>,string,<desc>` | Human-readable artifact summary. |
| `artifact,file` | `<ts>,<target>,artifact,<idx>,file,<idx>,<path>`| File path of emitted artifact (e.g. `.vmdk`). |

---

## 6. Enterprise Governance: HCP Packer & Policy Enforcement

### HCP Packer Registry

Track Golden Image lineage and revoke deprecated base images using HashiCorp Cloud Platform (HCP):

```hcl
packer {
  required_plugins {
    amazon = {
      version = ">= 1.0.0"
      source  = "github.com/hashicorp/amazon"
    }
  }
}

source "amazon-ebs" "base" {
  ami_name      = "golden-ubuntu-{{timestamp}}"
  instance_type = "t3.medium"
  region        = "us-east-1"
  source_ami_filter {
    filters = {
      name                = "ubuntu/images/hvm-ssd/ubuntu-jammy-22.04-amd64-server-*"
      root-device-type    = "ebs"
      virtualization-type = "hvm"
    }
    owners      = ["099720109477"]
    most_recent = true
  }
}

build {
  hcp_packer_registry {
    bucket_name = "ubuntu-base"
    description = "Hardened corporate Golden Ubuntu Image"
    bucket_labels = {
      "tier" = "base"
      "team" = "platform-sec"
    }
    build_labels = {
      "git_sha" = env("GITHUB_SHA")
    }
  }

  sources = ["source.amazon-ebs.base"]
}
```

Stamp automatically registers iterations and prevents building on revoked parent images.

### Policy Enforcement (OPA Rego & Sentinel)

Enforce compliance before machine images are distributed:

```rego
# policy.rego - Enforce encrypted root EBS volumes
package packer.security

default allow = false

allow {
    input.builders[_].encrypt_boot == true
}
```

Run Stamp with automated policy evaluation:
```sh
stamp build template.pkr.hcl
```
To bypass for authorized emergency maintenance:
```sh
stamp build --skip-enforcement template.pkr.hcl
```

---

## 7. End-to-End Real-World Examples

### Multi-Target HCL2 Production Template

A template concurrently generating an AWS AMI and a local QEMU KVM image:

```hcl
packer {
  required_plugins {
    amazon = {
      version = ">= 1.0.0"
      source  = "github.com/hashicorp/amazon"
    }
    qemu = {
      version = ">= 1.0.0"
      source  = "github.com/hashicorp/qemu"
    }
  }
}

variable "app_version" {
  type    = string
  default = "2.4.0"
}

locals {
  image_name = "golden-node-${var.app_version}-${formatdate("YYYYMMDDhhmmss", timestamp())}"
}

source "amazon-ebs" "web" {
  ami_name      = local.image_name
  instance_type = "t3.small"
  region        = "us-west-2"
  source_ami    = "ami-0735c191cf9140753"
  ssh_username  = "ubuntu"
}

source "qemu" "web" {
  iso_url          = "https://releases.ubuntu.com/jammy/ubuntu-22.04.4-live-server-amd64.iso"
  iso_checksum     = "file:https://releases.ubuntu.com/jammy/SHA256SUMS"
  output_directory = "output-qemu"
  shutdown_command = "sudo shutdown -P now"
  disk_size        = "20G"
  format           = "qcow2"
  ssh_username     = "ubuntu"
  ssh_password     = "secret"
  ssh_timeout      = "20m"
  vm_name          = "${local.image_name}.qcow2"
  http_directory   = "http"
  boot_command     = [
    "c<wait>",
    "linux /casper/vmlinuz --- autoinstall ds='nocloud-net;s=http://{{ .HTTPIP }}:{{ .HTTPPort }}/'<enter>",
    "initrd /casper/initrd<enter>",
    "boot<enter>"
  ]
}

build {
  sources = [
    "source.amazon-ebs.web",
    "source.qemu.web"
  ]

  provisioner "shell" {
    inline = [
      "sudo apt-get update -y",
      "sudo apt-get install -y curl nodejs nginx",
      "echo 'Application version: ${var.app_version}' | sudo tee /var/www/html/version.txt"
    ]
  }

  post-processor "manifest" {
    output = "manifest.json"
    strip_path = true
  }
}
```

### Migrating Legacy JSON Templates

Transform legacy Packer JSON templates into modern HCL2:

```sh
# 1. Automatically resolve deprecated keys and builders
stamp fix legacy.json > fixed.json

# 2. Convert JSON into idiomatic HCL2
stamp hcl2-upgrade fixed.json

# 3. Format generated template
stamp fmt fixed.pkr.hcl

# 4. Verify syntax
stamp validate fixed.pkr.hcl
```

---

## 8. Authoring Standalone Plugins (SDK)

The `stamp-plugin-sdk` crate allows you to write standalone plugins in pure Rust:

### `Cargo.toml`

```toml
[package]
name = "packer-plugin-custom"
version = "0.1.0"
edition = "2024"

[dependencies]
stamp-plugin-sdk = { path = "/path/to/stamp/stamp-plugin-sdk" }
async-trait = "0.1"
tokio = { version = "1.0", features = ["full"] }
```

### `src/main.rs`

```rust
use async_trait::async_trait;
use stamp_plugin_sdk::{
    Artifact, Builder, Communicator, PluginServerBuilder, StampError, Ui, serve_plugin,
};
use std::collections::HashMap;

#[derive(Default)]
pub struct CustomBuilder;

#[async_trait]
impl Builder for CustomBuilder {
    fn name(&self) -> &'static str {
        "custom-cloud"
    }

    async fn prepare(&mut self, _config: &HashMap<String, String>) -> Result<(), StampError> {
        Ok(())
    }

    async fn run(
        &mut self,
        ui: &mut dyn Ui,
        _comm: &mut dyn Communicator,
    ) -> Result<Box<dyn Artifact>, StampError> {
        ui.say("Provisioning custom cloud server...");
        // Provisioning logic here...
        Ok(Box::new(libstamp::artifact::MockArtifact::new("cloud-image-12345")))
    }

    async fn cancel(&mut self) -> Result<(), StampError> {
        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    serve_plugin!(builder => ("custom-cloud", CustomBuilder::default()));
    Ok(())
}
```

---

## 9. Embedding libstamp in Rust Applications

Embed the full machine image build engine directly inside your Rust binaries:

```rust
use libstamp::builder::null::{NullBuilder, NullConfig};
use libstamp::engine::packer::build_concurrently;
use libstamp::error::StampError;

#[tokio::main]
async fn main() -> Result<(), StampError> {
    let builder_web = Box::new(NullBuilder::new(NullConfig {
        name: "web-server".into(),
    }));
    let builder_api = Box::new(NullBuilder::new(NullConfig {
        name: "api-server".into(),
    }));

    // Run parallel builds with coordinated error handling and cancellation
    let artifacts = build_concurrently(vec![builder_web, builder_api]).await?;

    for artifact in artifacts {
        println!("Successfully created artifact: {}", artifact.id());
    }

    Ok(())
}
```

---

## 10. Auditing, Linting & Verification

Stamp maintains strict quality standards. Run the full verification suite with:

```sh
# Verify 100% rustdoc coverage (denies any missing docs)
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps

# Verify 100% test coverage across lines, branches, and functions
cargo llvm-cov --workspace

# Run strict clippy with pedantic and unwrap denies
cargo clippy --workspace --all-targets --all-features -- -D warnings -D clippy::pedantic

# Grounding parity assertion against official HashiCorp Packer reference schema
cargo run -p stamp --bin grounding
```
