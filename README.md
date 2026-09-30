# AgentTrace

![Rust](https://img.shields.io/badge/Rust-2024-orange?logo=rust)
![Status](https://img.shields.io/badge/status-phase%202-blue)
![License](https://img.shields.io/badge/license-MIT-green)
![npm](https://img.shields.io/npm/v/%40coachlogic%2Fagenttrace)
![Release](https://img.shields.io/github/v/release/VeloraTech/AgentTrace)

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

## Development commands

Node.js 18 or newer and Rust stable/Cargo are needed for development. End users installing the published npm package do not need Rust: the package contains prebuilt binaries for each published target.

```powershell
npm.cmd run build
npm.cmd run build:release
npm.cmd test
npm.cmd run pack
```

`npm run pack` builds and packages only the current machine's binary for local testing. It is not the cross-platform release package. The authoritative package is assembled in GitHub Actions from all successful platform builds.

## Release platforms

The release workflow attempts native build-and-test jobs for:

| npm platform | Rust target | CI runner |
| --- | --- | --- |
| Windows x64 | `x86_64-pc-windows-msvc` | `windows-2025` |
| Windows ARM64 | `aarch64-pc-windows-msvc` | `windows-11-arm` |
| macOS x64 | `x86_64-apple-darwin` | `macos-15-intel` |
| macOS ARM64 | `aarch64-apple-darwin` | `macos-15` |
| Linux x64 | `x86_64-unknown-linux-gnu` | `ubuntu-22.04` |
| Linux ARM64 | `aarch64-unknown-linux-gnu` | `ubuntu-22.04-arm` |

These are configured targets, not a claim that every target has already passed. A tag produces no release or npm publish unless every native runner builds and tests its target. Linux binaries are built on Ubuntu 22.04 and require a compatible glibc system. Verify the resulting release workflow before describing a target as supported.

## Create a release

Set the same stable semantic version in `Cargo.toml` and `package.json`, commit the change, and push the matching version tag:

```bash
git tag v0.1.1
git push origin v0.1.1
```

The tag is the release source of truth. GitHub Actions rejects malformed tags and any mismatch with either package version, then runs the six native tests/builds. If all pass, it stages and verifies the npm package, creates or reuses the GitHub Release, uploads the six binaries plus `SHA256SUMS` and the npm tarball, and publishes `@coachlogic/agenttrace` with provenance. Release uploads replace assets on retry; an already-published npm version is not published twice.

### npm trusted publishing setup

No npm token is stored in this repository or required by the release workflow. Configure npm Trusted Publishing for package `@coachlogic/agenttrace` with GitHub owner `VeloraTech`, repository `AgentTrace`, workflow filename `release.yml`, and permission to publish. The workflow requests only GitHub's short-lived OIDC token (`id-token: write`). npm requires Node.js 22.14+ and npm 11.5.1+ for trusted publishing; the publish job installs Node.js 24 and a compatible npm CLI.

npm blocked the unscoped `agenttrace` name as too similar to an existing package. The authenticated npm account is `coachlogic`, so the package now uses `@coachlogic/agenttrace`; the CLI command remains `agenttrace`. Since `v0.1.0` is already tagged with the old metadata, the next release is `v0.1.1`. npm requires the package to exist before Trusted Publishing can be configured. For the first scoped publish, let the `v0.1.1` workflow build and attach its all-platform tarball, publish that tarball once with `npm publish --access public <tarball>`, configure OIDC for `@coachlogic/agenttrace`, then rerun the same tag workflow. It reuses the GitHub Release and skips the already-published npm version. Do not force-move the existing `v0.1.0` tag.

The resulting install command is:

```bash
npm install -g @coachlogic/agenttrace
agenttrace run
```

## crates.io source package

The Rust source crate remains separately publishable as `agenttrace-cli`:

```powershell
cargo test --locked
cargo package --list --allow-dirty
cargo package --allow-dirty
```

This creates `target\package\agenttrace-cli-0.1.1.crate`. crates.io publication is separate from the GitHub/npm release workflow.

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
