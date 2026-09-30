# AgentTrace

![Rust](https://img.shields.io/badge/Rust-2024-orange?logo=rust)
![Status](https://img.shields.io/badge/status-phase%201-blue)
![License](https://img.shields.io/badge/license-MIT-green)
![Registry](https://img.shields.io/badge/registry-Cargo%20package%20in%20progress-lightgrey)

AgentTrace is a local CLI for discovering AI coding-agent processes. It is being built as a system-level flight recorder: observed evidence should remain distinct from agent claims and derived conclusions.

## Current status

Phase 1 takes a one-time process snapshot and reports candidate Claude Code, Codex, and Gemini CLI instances. Each process is listed separately using its PID and start time, with parent PID, executable, and working directory when available. Detection is best-effort and uses process names, executable paths, and known package markers.

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

The current tests cover agent classification and independent process identity. They do not yet exercise OS-level observation or multiple live agent processes.

## Build and package

Build the executable:

```powershell
cargo build --release
```

The executable is `target\release\agenttrace.exe` on Windows and `target/release/agenttrace` on Unix-like systems.

Inspect and create the Cargo registry archive:

```powershell
cargo package --list
cargo package
```

Cargo creates a compressed `.crate` source archive under `target/package/`. That is the Rust registry package; users can install its binary with `cargo install agenttrace-cli`, which provides the `agenttrace` command. For crates.io, first verify the package with `cargo publish --dry-run`, then publish with `cargo publish` after the package metadata and registry account are ready. Publishing is not automated by this repository.

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
