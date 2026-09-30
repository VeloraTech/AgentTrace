# AgentTrace

![Rust](https://img.shields.io/badge/Rust-2024-orange?logo=rust)
![Status](https://img.shields.io/badge/status-phase%202-blue)
![License](https://img.shields.io/badge/license-MIT-green)
![Registry](https://img.shields.io/badge/registry-Cargo%20package%20in%20progress-lightgrey)

AgentTrace is a local CLI for discovering AI coding-agent processes. It is being built as a system-level flight recorder: observed evidence should remain distinct from agent claims and derived conclusions.

## Current status

Phase 2 takes a one-time process snapshot, builds parent-child relationships, and assigns each observed descendant to its nearest detected agent process. Each agent process has an independent trace ID derived from its PID and start time, so same-agent instances stay separate even when they share a working directory. The display includes each trace's observed subprocess tree.

This phase does not record file, command, or network activity. It does not yet maintain persistent traces or a live process view. Process details may be unavailable when the operating system restricts access. Windows command-line access can require elevated permissions; AgentTrace does not request elevation.

Command arguments are inspected in memory for detection but are never printed or persisted.

## Requirements

- Rust stable and Cargo
- On Windows, the Rust MSVC toolchain and its Visual Studio C++ build tools

## Run locally

```powershell
cargo run -- run
```

Omitting `run` takes the same discovery snapshot. Start an agent first, leave it running, then launch this command in another terminal to see whether it is detected.

## Test

```powershell
cargo test
```

The tests cover agent classification, separate same-agent traces, nearest-ancestor attribution, and a real spawned child process. They do not yet exercise file, command, or network observation.

## Build and package

Build the executable:

```powershell
cargo build --release
```

The executable is `target\release\agenttrace.exe` on Windows and `target/release/agenttrace` on Unix-like systems.

Inspect and create the Cargo registry archive:

```powershell
cargo package --list --allow-dirty
cargo package --allow-dirty
```

Cargo creates a compressed `.crate` source archive under `target/package/`. That is the Rust registry package; users can install its binary with `cargo install agenttrace-cli`, which provides the `agenttrace` command. For crates.io, commit the release changes, verify with `cargo publish --dry-run`, then publish with `cargo publish` after the package metadata and registry account are ready. Publishing is not automated by this repository.

Standalone `.zip` or `.tar.gz` archives containing compiled binaries for each operating system can be added to release automation later. The Cargo `.crate` archive is source for Cargo, not a precompiled executable.

## Clean generated files

Preview the cleanup:

```powershell
.\clean.ps1 -WhatIf
```

Remove generated build and release output:

```powershell
.\clean.ps1
```

The script removes only the project's `target` and `dist` directories. It keeps source files, `Cargo.lock`, documentation, and local AgentTrace data.

## Supported agent detectors

- Claude Code (`claude` or Claude Code package markers)
- Codex (`codex` or Codex package markers)
- Gemini CLI (`gemini` or Gemini CLI package markers)

Detection rules may need updates as agents change their packaging or executable names.
