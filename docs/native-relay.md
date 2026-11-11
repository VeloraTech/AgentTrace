# NativeRelay Integration

## Overview

**NativeRelay is an independently maintained companion project**, not an AgentTrace component. It provides operating-system process and filesystem observations through a versioned CLI and JSONL protocol. AgentTrace can launch it as an optional child collector and merge its records into the AgentTrace history.

- Repository: https://github.com/VeloraTech/NativeRelay
- PyPI: https://pypi.org/project/nativerelay/
- License: MIT

AgentTrace and NativeRelay are independently versioned and released. NativeRelay is never installed automatically by AgentTrace.

## Compatibility

The current AgentTrace integration accepts NativeRelay versions:

```
0.3.0 <= nativerelay < 0.5.0
```

NativeRelay **0.4.0 is the current tested release**. AgentTrace's Windows integration workflow installs exactly `nativerelay==0.4.0` from PyPI and verifies live process and scoped filesystem observations.

Install the tested release:

```bash
python -m pip install "nativerelay==0.4.0"
nativerelay --version
```

AgentTrace reports versions outside the supported range as too old or newer than the tested range. A newer version may work, but is not considered compatible until the integration is reviewed and tested.

## Platform support

| Platform | NativeRelay collector | Observations | Important limits |
|---|---|---|---|
| Linux | Available | Process lifecycle via CN_PROC; scoped filesystem events via fanotify | Best effort; process monitoring requires root or `CAP_NET_ADMIN`; fanotify requires `CAP_SYS_ADMIN` |
| Windows 10/11 | Available in 0.4.x | Process lifecycle via WMI when available, with Toolhelp polling fallback; scoped create/modify/delete/rename notifications | Toolhelp can miss short-lived processes; WMI may be unavailable; file-open events are unsupported |
| macOS | Not available | AgentTrace's built-in process polling remains available | No NativeRelay macOS collector |
| WSL2 | Depends on guest kernel | Linux collector where required kernel interfaces are present | Kernel and privilege support vary |

AgentTrace currently reports Windows NativeRelay platform support as **Degraded** even though the 0.4.x Windows collector can run. This status is intentionally conservative: WMI availability and polling coverage vary by host. The integration test on Windows verifies that the collector starts and that representative file-created, file-modified, process-started, and process-exited records reach AgentTrace.

Linux observations are also best effort. Kernel buffer overflow and other detected collection losses are reported; NativeRelay cannot replay events that the kernel or collector has already dropped.

## Installation and verification

Install NativeRelay separately on the machine where AgentTrace will run:

```bash
python -m pip install "nativerelay==0.4.0"
nativerelay --version
```

On Windows, ensure the Python Scripts directory containing `nativerelay.exe` is on `PATH`. AgentTrace also checks common per-user Python Scripts locations.

On Linux, full process and filesystem visibility requires the relevant privileges and kernel features. Without them, NativeRelay may start in degraded mode with reduced capabilities.

## Usage

Enable NativeRelay in watch mode:

```bash
agenttrace watch --native-relay
```

The collector is opt-in. AgentTrace's ordinary process discovery and polling do not require NativeRelay.

Supported options:

- `--native-relay-scope PATH` — directory scope for filesystem observations (default: current directory)
- `--native-relay-include-command` — include process command arguments; arguments may contain secrets
- `--native-relay-include-paths PATTERN` — include matching paths; repeatable
- `--native-relay-exclude-paths PATTERN` — exclude matching paths; repeatable

Check installation and collector status:

```bash
agenttrace status --native-relay
agenttrace status --native-relay --json
agenttrace doctor
agenttrace doctor --json
```

Status and diagnostics report the discovered executable, version, compatibility, platform support, and whether a test invocation can start.

## Integration contract

AgentTrace communicates with NativeRelay through its command-line interface and JSONL stdout:

1. Locate `nativerelay` or `nativerelay.exe` on `PATH` (including supported per-user Python Scripts locations on Windows).
2. Read its version and report compatibility against the supported range.
3. Launch `nativerelay run --format json --scope <directory>` with the selected options.
4. Read JSONL records incrementally and capture stderr for diagnostics.
5. Request graceful shutdown, drain buffered output, and record the final collector state.

AgentTrace does not import NativeRelay's Python modules. The integration depends on the CLI and schema-versioned records, not on the implementation language.

### Records

AgentTrace preserves NativeRelay's event fields in its history, including:

- `nativerelay.status` — collector state, capabilities, and health
- `nativerelay.loss` — detected collection losses
- `process.started` and `process.exited`
- `file.opened`, `file.created`, `file.modified`, `file.deleted`, and `file.renamed` where supported by the host collector

Windows does not currently emit file-open observations. NativeRelay Windows file notifications do not identify the process responsible for the filesystem change. Linux process and file attribution and coverage are best effort.

AgentTrace marks a recording incomplete when it detects reported loss, sequence gaps, malformed records, degraded or unavailable collector status, or an unexpected collector exit. The final session record includes a NativeRelay summary.

### Protocol compatibility

The integration currently consumes schema version 1. Unknown event types are handled as unsupported records rather than being treated as known observations. Additional fields are preserved or ignored where they do not affect required parsing. A breaking schema change requires an AgentTrace integration update.

## Troubleshooting

### NativeRelay executable not found

Install the tested release and ensure its executable directory is available:

```bash
python -m pip install "nativerelay==0.4.0"
nativerelay --version
```

On Windows, add the active Python installation's Scripts directory to `PATH`, then restart the terminal so the updated environment is inherited.

### Version is too old or newer than tested

Check the reported version and install the supported release:

```bash
python -m pip install --upgrade "nativerelay==0.4.0"
```

### Collector starts in degraded mode

- On Linux, check kernel support and root/`CAP_NET_ADMIN`/`CAP_SYS_ADMIN` permissions.
- On Windows, WMI may be unavailable due to host policy or permissions. NativeRelay can use Toolhelp polling as a fallback, but short-lived processes may be missed.
- Review the capabilities and health details in `agenttrace status --native-relay` and the NativeRelay status records in the history.

### Events are missing

Use a disposable test directory within the configured scope. Filesystem notifications are scoped; events outside that directory are not expected. Short-lived processes can be missed by polling fallback. Review loss and health records before interpreting missing events as proof that no activity occurred.

## Privacy and safety

NativeRelay is local-first and does not require an account or remote telemetry service. Use a narrow, disposable scope when testing. Process command-line capture is opt-in and may expose credentials or other sensitive arguments. Paths can also reveal private information. Do not publish unredacted logs, command lines, or sensitive paths in issue reports.

## Related links

- [AgentTrace README](../README.md)
- [NativeRelay repository](https://github.com/VeloraTech/NativeRelay)
- [NativeRelay v0.4.0 release](https://github.com/VeloraTech/NativeRelay/releases/tag/v0.4.0)
- [NativeRelay on PyPI](https://pypi.org/project/nativerelay/)
