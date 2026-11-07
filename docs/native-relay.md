# NativeRelay Integration

## Overview

**NativeRelay is an independent external project**, not an internal component of AgentTrace. It provides Linux kernel-level telemetry (process lifecycle and file system events) via CN_PROC and fanotify, which AgentTrace can optionally integrate with for enhanced observability.

- **Repository**: https://github.com/VeloraTech/NativeRelay
- **PyPI Package**: https://pypi.org/project/nativerelay/
- **License**: MIT (same as AgentTrace)

AgentTrace and NativeRelay are **independently versioned and independently releasable**. This document describes the integration contract, not the internal implementation of either project.

---

## Role in AgentTrace

AgentTrace's default collector uses periodic process polling via `sysinfo`. This works cross-platform but has limitations:

- Cannot detect short-lived processes between polls
- Cannot capture file open/read/write events
- Cannot capture exact process exit codes for unobserved processes

When NativeRelay is available and enabled (`--native-relay`), AgentTrace spawns it as a child process and consumes its JSONL output, merging kernel-level events into the same timeline. This provides:

- Process start/exit events via CN_PROC (Linux)
- File opened/created/modified/deleted/renamed events via fanotify (Linux)
- Best-effort scoped file watching within a directory
- Loss reporting when kernel buffers overflow

---

## Installation

```bash
pip install nativerelay==0.3.2
```

**Requirements:**
- Python 3.10+
- Linux kernel with CN_PROC and fanotify support
- Root privileges or `CAP_NET_ADMIN` + `CAP_SYS_ADMIN` for full functionality

After installation, verify:

```bash
nativerelay --version
# Should output: 0.3.2 (or compatible version)
```

---

## Platform Support

| Platform | Process Discovery | Process Tracing | File Watching | NativeRelay Integration |
|----------|-------------------|-----------------|---------------|------------------------|
| **Linux** | ✅ Full | ✅ Full | ✅ Polling | ✅ **Full** (CN_PROC + fanotify) |
| **macOS** | ✅ Full | ✅ Full | ✅ Polling | ❌ Not available |
| **Windows** | ✅ Full | ✅ Full | ✅ Polling | ❌ Not available |
| **WSL2** | ✅ Full | ✅ Full | ✅ Polling | ⚠️ Limited (kernel features may be missing) |

**Linux Requirements for Full Support:**
- Root access **OR** both capabilities:
  - `CAP_NET_ADMIN` (for CN_PROC process events)
  - `CAP_SYS_ADMIN` (for fanotify file events)
- Without these, NativeRelay runs in **degraded mode** with limited event visibility

---

## AgentTrace ↔ NativeRelay Compatibility

AgentTrace declares a **tested compatibility range** for NativeRelay versions:

| AgentTrace Version | NativeRelay Range | Notes |
|--------------------|-------------------|-------|
| 0.3.x | `0.3.0 ≤ version < 0.4.0` | Tested with 0.3.2 |

**Compatibility Policy:**
- AgentTrace uses **semantic versioning** for the integration protocol
- Patch versions (0.3.x) within the range are expected to work
- Minor version bumps (0.4.0) may introduce protocol changes requiring AgentTrace updates
- AgentTrace will not silently depend on undocumented NativeRelay internals

---

## Usage

### Enable NativeRelay in Watch Mode

```bash
agenttrace watch --native-relay
```

Options:
- `--native-relay-scope PATH` — Directory to watch for file events (default: current directory)
- `--native-relay-include-command` — Include command arguments in process events (may expose secrets)
- `--native-relay-include-paths PATTERN` — fnmatch pattern to include (repeatable)
- `--native-relay-exclude-paths PATTERN` — fnmatch pattern to exclude (repeatable)

### Check NativeRelay Status

```bash
agenttrace status --native-relay
```

Output includes:
- Detected executable path
- Version
- Compatibility status (Compatible / Too Old / Too New / Unavailable)
- Platform support level (Full / Degraded / Unavailable)
- Whether the collector can actually start

JSON output available with `--json`.

### Diagnose NativeRelay Installation

```bash
agenttrace doctor
```

Performs comprehensive checks:
- Executable found in PATH
- Version within compatibility range
- Platform support level
- Collector can start (test invocation)
- Actionable troubleshooting guidance

JSON output available with `--json`.

---

## Architecture

### Integration Boundary

AgentTrace communicates with NativeRelay **exclusively through its CLI and JSONL stdout**:

1. **Discovery**: AgentTrace searches PATH for `nativerelay` (or `nativerelay.exe` on Windows)
2. **Version Check**: Runs `nativerelay --version` to verify compatibility
3. **Spawn**: Launches `nativerelay run --format json --scope <dir> [options]`
4. **Consume**: Reads JSONL from stdout, stderr for diagnostics
5. **Lifecycle**: Sends SIGTERM on shutdown, force-kills after grace period

**No Python-specific coupling exists.** The integration works with any executable that:
- Accepts the same CLI arguments
- Outputs JSONL with schema version 1
- Implements the same record types

### Two Operating Modes (Future-Ready)

The architecture accommodates two modes without requiring a major refactor:

| Mode | Description | Status |
|------|-------------|--------|
| **Managed Collector** (current) | AgentTrace spawns and manages NativeRelay process | ✅ Implemented |
| **External Collector** (planned) | AgentTrace connects to independently running NativeRelay | 🔧 Interface defined |

**External Collector Interface (for future implementation):**
- `NativeRelayInfo` — discovery and diagnostics
- `NativeRelayStatus` — health, capabilities, losses
- `parse_status_record()` — protocol parsing
- Connection via stdio, Unix socket, or TCP (TBD)

### Protocol Stability

**Record Schema Version: 1**

AgentTrace parses these NativeRelay record types:
- `nativerelay.status` — capabilities, health, state
- `nativerelay.loss` — buffer overflow notifications
- Event records: `process.started`, `process.exited`, `file.opened`, `file.created`, `file.modified`, `file.deleted`, `file.renamed`

**Compatibility Considerations:**
- New event types → AgentTrace logs as `native_relay.unsupported_record` (non-fatal)
- New fields in existing records → Ignored (forward compatible)
- Removed required fields → Parse error (handled gracefully)
- Schema version bump → Requires AgentTrace update

---

## Troubleshooting

### "NativeRelay not found"
```
NativeRelay is required but was not found. Install NativeRelay 0.3.0 ≤ version < 0.4.0 with:
    pip install nativerelay==0.3.2
If it is already installed, add its Scripts/bin directory to PATH.
```
**Fix:** Install NativeRelay or add its installation directory to PATH.

### "Version too old"
```
NativeRelay version 0.2.0 is too old. AgentTrace requires 0.3.0 ≤ version < 0.4.0.
    Upgrade with: pip install --upgrade nativerelay==0.3.2
```
**Fix:** Upgrade NativeRelay to a compatible version.

### "Version newer than tested range"
```
NativeRelay version 0.4.0 is newer than the tested compatibility range (0.3.0 ≤ version < 0.4.0).
This version may work but is not officially tested. Consider pinning: pip install nativerelay==0.3.2
```
**Fix:** Pin to tested version or test compatibility manually.

### "Cannot start NativeRelay"
```
NativeRelay found at /usr/local/bin/nativerelay but failed to start: permission denied
NativeRelay may have limited functionality on this platform.
On Linux, full support requires root or CAP_NET_ADMIN + CAP_SYS_ADMIN.
```
**Fix:** Run with sufficient privileges, or accept degraded mode.

### "Platform not supported"
```
NativeRelay collector is not available on this platform.
Supported platforms: Linux (with CN_PROC + fanotify)
```
**Fix:** NativeRelay only works on Linux. Use AgentTrace's built-in polling collector on other platforms.

---

## Limitations

- **Linux-only**: NativeRelay's kernel collectors require Linux-specific interfaces
- **Privileges required**: Full functionality needs root or specific capabilities
- **Best-effort**: Kernel buffers can overflow under high load; losses are reported but events are lost
- **Scoped file watching**: fanotify watches a directory tree, not individual files
- **No Windows/macOS support**: Planned but not implemented in NativeRelay

---

## Future Directions

### Rust-Native Core
NativeRelay may transition to a Rust implementation. AgentTrace's integration is designed to accommodate this:
- No Python-specific assumptions in AgentTrace
- CLI protocol remains stable
- Distribution via both PyPI and npm possible
- Same binary interface, different implementation

### npm Distribution
Future NativeRelay releases may be distributed via npm (`@veloratech/nativerelay`) alongside PyPI. AgentTrace's discovery mechanism (PATH-based executable lookup) works identically for both.

### External Collector Mode
Planned support for connecting to a pre-running NativeRelay instance (e.g., as a systemd service), enabling:
- Shared collector across multiple AgentTrace invocations
- Persistent collector for lower startup latency
- Centralized telemetry in CI/CD environments

---

## Related Commands

| Command | Description |
|---------|-------------|
| `agenttrace watch --native-relay` | Start watching with NativeRelay |
| `agenttrace status --native-relay` | Show NativeRelay status and capabilities |
| `agenttrace doctor` | Diagnose NativeRelay installation |
| `agenttrace run --watch --native-relay` | Alias for watch with NativeRelay |

---

## See Also

- [AgentTrace README](../README.md) — Main project documentation
- [NativeRelay GitHub](https://github.com/VeloraTech/NativeRelay) — NativeRelay source and issues
- [NativeRelay PyPI](https://pypi.org/project/nativerelay/) — Package releases