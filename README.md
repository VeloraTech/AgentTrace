# AgentTrace

![Rust](https://img.shields.io/badge/Rust-2024-orange?logo=rust)
![Status](https://img.shields.io/badge/status-phase%203%20complete-brightgreen)
![License](https://img.shields.io/badge/license-MIT-green)
![npm](https://img.shields.io/npm/v/%40coachlogic%2Fagenttrace)
![Release](https://img.shields.io/github/v/release/VeloraTech/AgentTrace)

## See what your AI coding agents actually do.

That is the destination. AgentTrace is being built to detect every running coding agent on your machine, separate their processes, and trace their activity.

```powershell
npm install -g @coachlogic/agenttrace
agenttrace run
```

Phase 3's portable trace-recorder milestone is complete. AgentTrace can take a process snapshot, poll running agent process trees into JSONL history, or launch a command under supervision and record its process lifecycle, observed descendants, streams, and optionally changed workspace files. Exact file-open/read and network monitoring are not available; they are planned for Phase 4.

## Current status

Snapshot mode takes a one-time process snapshot, builds parent-child relationships, and assigns each observed descendant to its nearest detected agent process. Each agent process has an independent trace ID derived from its PID and start time, so same-agent instances stay separate even when they share a working directory.

`watch` polls agent process trees and appends discovery, metadata-change, and no-longer-observed events to `.agenttrace/history.jsonl`. Polling can miss short-lived processes and cannot establish their exit codes. `trace` launches a command, forwards stdin/stdout/stderr, and records process start/end times, exact exit status when available, and stream byte counts. It also polls observed descendants every 250 ms; short-lived children can be missed and their exit status is unavailable. Each JSONL event has a recorder `session_id`, sequence, timestamp, observation timestamp, and collector; supervised sessions also use their session ID as the execution `cycle_id`. These are recorder/process boundaries, not internal AI conversation or tool-call cycles.

Stream contents and raw command arguments are omitted unless explicitly enabled. `--capture-streams` stores up to 10 MiB per stream as hex, without redaction; captured data can contain credentials or private source. Supervised `trace` applies only to the launched command and observed descendants; it cannot collect arbitrary processes' streams. `--watch-files` opts into polling file metadata under the command working directory and reports created, modified, and deleted paths. It skips `.git`, `.agenttrace`, `node_modules`, `target`, and `.venv`, scans at most 10,000 files, and can miss transient changes. It does not establish that a process opened or read a file, and path names may be sensitive. File contents, exact file reads/opens, and network activity are not collected. This is not kernel-level system monitoring.

Native system-wide file-open/read and network monitoring remain future work; the current recorder reports only the process and workspace metadata it can actually observe.

For process discovery, command arguments are inspected in memory for detection but are not printed or persisted by default.

## Requirements

- Rust stable and Cargo
- On Windows, the Rust MSVC toolchain and its Visual Studio C++ build tools

## Run locally

```powershell
cargo run -- run
```

Omitting `run` takes the same discovery snapshot. Start an agent first, leave it running, then launch this command in another terminal to see whether it is detected.

Record and stream process-observation events until Ctrl+C, then inspect the JSONL history:

```powershell
cargo run -- watch
cargo run -- run --watch
cargo run -- history
```

Watch starts with a grouped summary of each detected agent, including its trace ID, PID, parent PID, working directory, executable, start time, and observed child process tree. It then prints timestamped live process changes rather than repeating the whole snapshot. Output is indented and color-coded in interactive terminals; color is disabled when output is piped or `NO_COLOR` is set. Use `--format json` for JSONL on stdout. The `.agenttrace/history.jsonl` file always remains JSONL; each event's `session_id` identifies one watch run, and process events' `trace_id` groups an agent and its observed descendants. Use `--duration-ms 10000` for a bounded 10-second session, `--interval-ms 500` to change the polling interval, or `--output PATH` to select another history file. `--include-command-args` records raw process arguments verbatim; they may contain secrets.

Launch and trace a command, preserving its exit code and forwarding its standard streams:

```powershell
cargo run -- trace --output .agenttrace/command.jsonl -- PROGRAM ARG1 ARG2
```

For example, on Windows use `cargo run -- trace -- powershell -NoProfile -Command "Write-Output hello; exit 7"`; on macOS/Linux use `cargo run -- trace -- sh -c "echo hello; exit 7"`. AgentTrace returns the child exit code. Add `--watch-files` before `--` to detect workspace file metadata changes, `--capture-streams` to record raw stream bytes, or `--include-command-args` to persist arguments. Each may expose sensitive data. `--cwd PATH` sets the command working directory.

## Test

```powershell
cargo test
```

The tests cover agent classification, separate same-agent traces, nearest-ancestor attribution, process polling, supervised descendant attribution, workspace file-delta events, option parsing, and stream-byte encoding. Native file-open/read and network observation is Phase 4 work, not a Phase 3 capability.

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

The latest release is `0.1.5` (`v0.1.5` is already tagged). The next release is `0.1.6`; keep versions in `Cargo.toml`, `Cargo.lock`, and `package.json` in sync before tagging. After review and commit, push the branch and tag:

```powershell
git push origin main
git tag v0.1.6
git push origin v0.1.6
```

The tag is the release source of truth. GitHub Actions rejects malformed tags and any mismatch with either package version, then runs the six native tests/builds. If all pass, it stages and verifies the npm package, creates or reuses the GitHub Release, uploads the six binaries plus `SHA256SUMS` and the npm tarball, and publishes `@coachlogic/agenttrace` with provenance. Release uploads replace assets on retry; an already-published npm version is not published twice.

### npm trusted publishing setup

No npm token is stored in this repository or required by the release workflow. Configure npm Trusted Publishing for package `@coachlogic/agenttrace` with GitHub owner `VeloraTech`, repository `AgentTrace`, workflow filename `release.yml`, and permission to publish. The workflow requests only GitHub's short-lived OIDC token (`id-token: write`). npm requires Node.js 22.14+ and npm 11.5.1+ for trusted publishing; the publish job installs Node.js 24 and a compatible npm CLI.

npm blocked the unscoped `agenttrace` name as too similar to an existing package, so npm uses `@coachlogic/agenttrace` while the CLI command remains `agenttrace`. If this is the first publish of the scoped package, npm requires one initial publish before Trusted Publishing can be configured: publish the release tarball once with `npm publish --access public <tarball>`, configure OIDC for `@coachlogic/agenttrace`, then rerun that release workflow. For subsequent releases, the workflow publishes the package with provenance. Do not force-move existing version tags.

For a project-local install, run the CLI through npm so its local `node_modules/.bin` directory is on `PATH`:

```bash
npm install @coachlogic/agenttrace
npm exec -- agenttrace run
```

For a global install, the command is available directly in the terminal:

```bash
npm install -g @coachlogic/agenttrace
agenttrace run
```

AgentTrace exposes the `agenttrace` executable through npm's `bin` field. It does not modify the consuming project's `package.json` to add scripts; if you want an `npm run` shortcut, add one in that project yourself, for example: `"agenttrace": "agenttrace run"`.

## crates.io source package

The Rust source crate remains separately publishable as `agenttrace-cli`:

```powershell
cargo test --locked
cargo package --list --allow-dirty
cargo package --allow-dirty
```

For version `0.1.6`, this creates `target\package\agenttrace-cli-0.1.6.crate`. crates.io publication is separate from the GitHub/npm release workflow.

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
