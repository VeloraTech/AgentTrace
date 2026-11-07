use serde_json::Value;
use std::env;
use std::ffi::OsStr;
use std::io::{self, BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

const SHUTDOWN_GRACE_PERIOD: Duration = Duration::from_secs(2);

/// Minimum compatible NativeRelay version
pub const MIN_NATIVERELAY_VERSION: &str = "0.3.0";
/// Maximum tested NativeRelay version (exclusive upper bound for semver compatibility)
pub const MAX_NATIVERELAY_VERSION: &str = "0.4.0";

/// Parsed NativeRelay version information
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeRelayVersion {
    pub version: String,
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl NativeRelayVersion {
    /// Parse a version string like "0.3.2" into components
    pub fn parse(version: &str) -> Result<Self, String> {
        let parts: Vec<&str> = version.trim().split('.').collect();
        if parts.len() < 3 {
            return Err(format!("invalid version format: {}", version));
        }
        let major = parts[0].parse().map_err(|_| format!("invalid major version: {}", parts[0]))?;
        let minor = parts[1].parse().map_err(|_| format!("invalid minor version: {}", parts[1]))?;
        let patch = parts[2].parse().map_err(|_| format!("invalid patch version: {}", parts[2]))?;
        Ok(Self {
            version: version.to_string(),
            major,
            minor,
            patch,
        })
    }

    /// Check if this version is compatible with AgentTrace
    pub fn is_compatible(&self) -> bool {
        let min = Self::parse(MIN_NATIVERELAY_VERSION).unwrap();
        let max = Self::parse(MAX_NATIVERELAY_VERSION).unwrap();
        self >= &min && self < &max
    }

    /// Get a human-readable compatibility status
    pub fn compatibility_status(&self) -> CompatibilityStatus {
        let min = Self::parse(MIN_NATIVERELAY_VERSION).unwrap();
        let max = Self::parse(MAX_NATIVERELAY_VERSION).unwrap();
        if self < &min {
            CompatibilityStatus::TooOld
        } else if self >= &max {
            CompatibilityStatus::TooNew
        } else {
            CompatibilityStatus::Compatible
        }
    }
}

impl PartialOrd for NativeRelayVersion {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for NativeRelayVersion {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (self.major, self.minor, self.patch).cmp(&(other.major, other.minor, other.patch))
    }
}

/// Compatibility status of a NativeRelay version
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompatibilityStatus {
    Compatible,
    TooOld,
    TooNew,
    Unavailable,
}

/// NativeRelay collector capabilities as reported by the status record
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeRelayCapability {
    pub event_type: String,
    pub status: String,
    pub reason: String,
    pub scope: String,
}

/// NativeRelay health information
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeRelayHealth {
    pub dropped: u64,
    pub losses: serde_json::Value,
}

/// Complete NativeRelay status information
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeRelayStatus {
    pub state: String,
    pub capabilities: Vec<NativeRelayCapability>,
    pub health: NativeRelayHealth,
}

/// NativeRelay discovery and diagnostic information
#[derive(Debug, Clone)]
pub struct NativeRelayInfo {
    pub executable_path: Option<PathBuf>,
    pub version: Option<NativeRelayVersion>,
    pub compatibility: CompatibilityStatus,
    pub platform_support: PlatformSupport,
    pub can_start: bool,
    pub start_error: Option<String>,
}

/// Platform support information for NativeRelay
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlatformSupport {
    Full,
    Degraded,
    Unavailable,
}

#[derive(Debug)]
pub enum Message {
    Record(Value),
    Malformed {
        line: u64,
        error: String,
        raw: String,
    },
    Stderr(String),
    StdoutEof,
    StderrEof,
    ReadError(String),
}
pub struct Process {
    child: Child,
    messages: Receiver<Message>,
}

impl Process {
    pub fn spawn(scope: &Path) -> Result<Self, String> {
        let exe = find_executable().ok_or_else(missing_error)?;
        Self::spawn_command(exe.as_os_str(), &["run", "--format", "json", "--scope", scope.to_str().ok_or("scope path not valid UTF-8")?])
    }

    pub fn spawn_with_options(
        scope: &Path,
        include_command: bool,
        include_paths: &[String],
        exclude_paths: &[String],
    ) -> Result<Self, String> {
        let exe = find_executable().ok_or_else(missing_error)?;
        let mut args = vec!["run", "--format", "json", "--scope", scope.to_str().ok_or("scope path not valid UTF-8")?];
        if include_command {
            args.push("--include-command");
        }
        for pattern in include_paths {
            args.push("--include-paths");
            args.push(pattern);
        }
        for pattern in exclude_paths {
            args.push("--exclude-paths");
            args.push(pattern);
        }
        Self::spawn_command(exe.as_os_str(), &args)
    }
    fn spawn_command(program: &OsStr, args: &[&str]) -> Result<Self, String> {
        Self::spawn_command_env(program, args, &[])
    }
    fn spawn_command_env(
        program: &OsStr,
        args: &[&str],
        variables: &[(&str, &str)],
    ) -> Result<Self, String> {
        let mut command = Command::new(program);
        command
            .args(args)
            .envs(variables.iter().copied())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn().map_err(|e| {
            if e.kind() == io::ErrorKind::NotFound {
                missing_error()
            } else {
                format!(
                    "cannot start NativeRelay at {}: {e}",
                    Path::new(program).display()
                )
            }
        })?;
        let (tx, messages) = mpsc::channel();
        stdout_reader(child.stdout.take().expect("piped stdout"), tx.clone());
        stderr_reader(child.stderr.take().expect("piped stderr"), tx);
        Ok(Self { child, messages })
    }
    pub fn try_message(&self) -> Option<Message> {
        self.messages.try_recv().ok()
    }
    pub fn recv_timeout(&self, timeout: std::time::Duration) -> Option<Message> {
        self.messages.recv_timeout(timeout).ok()
    }
    pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        self.child.try_wait()
    }
    #[cfg(test)]
    pub fn wait(&mut self) -> io::Result<ExitStatus> {
        self.child.wait()
    }
    pub fn id(&self) -> u32 {
        self.child.id()
    }
    pub fn shutdown(&mut self) -> io::Result<ExitStatus> {
        if self.child.try_wait()?.is_none() {
            #[cfg(unix)]
            {
                let result = unsafe { libc::kill(self.child.id() as libc::pid_t, libc::SIGTERM) };
                if result == -1 {
                    let error = io::Error::last_os_error();
                    // The child can exit between try_wait and kill. Collect its status
                    // below for ESRCH; return other signal failures instead of waiting forever.
                    if error.raw_os_error() != Some(libc::ESRCH) {
                        return Err(error);
                    }
                }
            }
            #[cfg(windows)]
            self.child.kill()?;

            let deadline = Instant::now() + SHUTDOWN_GRACE_PERIOD;
            while Instant::now() < deadline {
                if let Some(status) = self.child.try_wait()? {
                    return Ok(status);
                }
                thread::sleep(Duration::from_millis(20));
            }

            // NativeRelay normally exits after SIGTERM. If it is stuck, force
            // termination so AgentTrace cannot hang forever or leave it behind.
            if self.child.try_wait()?.is_none() {
                self.child.kill()?;
            }
        }
        self.child.wait()
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        // Cover early returns after spawn (for example recorder or signal-handler
        // setup failures). Normal shutdown has already reaped the child, so this
        // path is only a last-resort cleanup.
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}



fn stdout_reader(stdout: impl io::Read + Send + 'static, tx: Sender<Message>) {
    thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        let mut n = 0;
        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) => {
                    let _ = tx.send(Message::StdoutEof);
                    break;
                }
                Ok(_) => {
                    n += 1;
                    let raw = line.trim_end_matches(['\r', '\n']).to_string();
                    if raw.trim().is_empty() {
                        continue;
                    }
                    match parse_record(&raw) {
                        Ok(v) => {
                            if tx.send(Message::Record(v)).is_err() {
                                break;
                            }
                        }
                        Err(error) => {
                            if tx
                                .send(Message::Malformed {
                                    line: n,
                                    error,
                                    raw,
                                })
                                .is_err()
                            {
                                break;
                            }
                        }
                    }
                }
                Err(e) => {
                    let _ = tx.send(Message::ReadError(e.to_string()));
                    break;
                }
            }
        }
    });
}
fn stderr_reader(stderr: impl io::Read + Send + 'static, tx: Sender<Message>) {
    thread::spawn(move || {
        for line in BufReader::new(stderr).lines() {
            match line {
                Ok(s) => {
                    if tx.send(Message::Stderr(s)).is_err() {
                        break;
                    }
                }
                Err(e) => {
                    let _ = tx.send(Message::Stderr(format!("could not read diagnostics: {e}")));
                    break;
                }
            }
        }
        let _ = tx.send(Message::StderrEof);
    });
}

pub fn parse_record(line: &str) -> Result<Value, String> {
    let value: Value = serde_json::from_str(line).map_err(|e| e.to_string())?;
    let obj = value
        .as_object()
        .ok_or_else(|| "record must be a JSON object".to_string())?;
    if let Some(kind) = obj.get("record_type").and_then(Value::as_str) {
        if obj.get("schema_version").and_then(Value::as_u64) != Some(1)
            || obj.get("timestamp").and_then(Value::as_str).is_none()
        {
            return Err(format!(
                "unsupported or incomplete NativeRelay control record {kind:?}"
            ));
        }
        return Ok(value);
    }
    let fields = ["id", "timestamp", "type", "platform", "collector"];
    if obj.get("schema_version").and_then(Value::as_u64) != Some(1)
        || fields
            .iter()
            .any(|k| obj.get(*k).and_then(Value::as_str).is_none())
        || !obj.get("process").is_some_and(Value::is_object)
    {
        return Err("unsupported or incomplete NativeRelay event record".to_string());
    }
    Ok(value)
}

pub fn find_executable() -> Option<PathBuf> {
    let name = if cfg!(windows) {
        "nativerelay.exe"
    } else {
        "nativerelay"
    };
    if let Some(paths) = env::var_os("PATH") {
        for dir in env::split_paths(&paths) {
            let p = dir.join(name);
            if p.is_file() {
                return Some(p);
            }
        }
    }
    #[cfg(windows)]
    {
        let roots = [
            env::var_os("APPDATA").map(|v| PathBuf::from(v).join("Python")),
            env::var_os("LOCALAPPDATA").map(|v| PathBuf::from(v).join("Programs").join("Python")),
        ];
        let mut candidates = Vec::new();
        for root in roots.into_iter().flatten() {
            if let Ok(entries) = fs::read_dir(root) {
                candidates.extend(
                    entries
                        .flatten()
                        .map(|entry| entry.path().join("Scripts").join(name)),
                );
            }
        }
        candidates.sort();
        if let Some(candidate) = candidates.into_iter().find(|path| path.is_file()) {
            return Some(candidate);
        }
    }
    #[cfg(unix)]
    if let Some(home) = env::var_os("HOME") {
        let p = PathBuf::from(home).join(".local/bin").join(name);
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

/// Get the version of NativeRelay by running `nativerelay --version`
pub fn get_version(executable: &Path) -> Option<String> {
    let output = Command::new(executable)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .ok()?;
    if output.status.success() {
        let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !version.is_empty() {
            return Some(version);
        }
    }
    // Fallback: try -V
    let output = Command::new(executable)
        .arg("-V")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .ok()?;
    if output.status.success() {
        let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !version.is_empty() {
            return Some(version);
        }
    }
    None
}

/// Discover and diagnose NativeRelay installation
pub fn discover_nativerelay() -> NativeRelayInfo {
    let executable_path = find_executable();
    let mut info = NativeRelayInfo {
        executable_path: executable_path.clone(),
        version: None,
        compatibility: CompatibilityStatus::Unavailable,
        platform_support: detect_platform_support(),
        can_start: false,
        start_error: None,
    };

    if let Some(ref path) = executable_path {
        if let Some(version_str) = get_version(path) {
            if let Ok(version) = NativeRelayVersion::parse(&version_str) {
                info.compatibility = version.compatibility_status();
                info.version = Some(version);
            }
        }

        // Test if we can actually start the collector
        let test_result = test_start_collector(path);
        info.can_start = test_result.is_ok();
        info.start_error = test_result.err();
    }

    info
}

/// Test if NativeRelay can start a collector (dry-run with --help to verify it works)
fn test_start_collector(executable: &Path) -> Result<(), String> {
    // Use --help as a quick test that the executable works
    let output = Command::new(executable)
        .arg("--help")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| format!("failed to execute NativeRelay: {e}"))?;

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(format!("NativeRelay failed to start: {}", stderr.trim()))
    }
}

/// Detect platform support level for NativeRelay
fn detect_platform_support() -> PlatformSupport {
    #[cfg(target_os = "linux")]
    {
        // Check if we have the required capabilities for full support
        if has_cap_net_admin() && has_cap_sys_admin() {
            PlatformSupport::Full
        } else if is_root() {
            PlatformSupport::Full
        } else {
            PlatformSupport::Degraded
        }
    }
    #[cfg(target_os = "macos")]
    {
        PlatformSupport::Unavailable
    }
    #[cfg(target_os = "windows")]
    {
        PlatformSupport::Unavailable
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        PlatformSupport::Unavailable
    }
}

#[cfg(target_os = "linux")]
fn is_root() -> bool {
    // Check if running as root (UID 0)
    unsafe { libc::getuid() == 0 }
}

#[cfg(target_os = "linux")]
fn has_cap_net_admin() -> bool {
    // Check for CAP_NET_ADMIN capability (value 12 on Linux)
    check_capability(12)
}

#[cfg(target_os = "linux")]
fn has_cap_sys_admin() -> bool {
    // Check for CAP_SYS_ADMIN capability (value 21 on Linux)
    check_capability(21)
}

#[cfg(target_os = "linux")]
fn check_capability(_cap: u32) -> bool {
    // Use cap_get_proc to check capabilities
    // This is a simplified check - in practice we'd use libcap
    // For now, just check if we're root
    is_root()
}

/// Parse NativeRelay status record into structured data
pub fn parse_status_record(record: &Value) -> Result<NativeRelayStatus, String> {
    let state = record["state"]
        .as_str()
        .ok_or("missing state in status record")?
        .to_string();

    let capabilities = record["capabilities"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|cap| {
                    Some(NativeRelayCapability {
                        event_type: cap["event_type"].as_str()?.to_string(),
                        status: cap["status"].as_str()?.to_string(),
                        reason: cap["reason"].as_str().unwrap_or("").to_string(),
                        scope: cap["scope"].as_str().unwrap_or("").to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    let health = NativeRelayHealth {
        dropped: record["health"]["dropped"].as_u64().unwrap_or(0),
        losses: record["health"]["losses"].clone(),
    };

    Ok(NativeRelayStatus {
        state,
        capabilities,
        health,
    })
}

/// Generate actionable error message for missing NativeRelay
pub fn missing_error() -> String {
    format!(
        "NativeRelay is required but was not found. Install NativeRelay {} with:\n    pip install nativerelay=={}\nIf it is already installed, add its Scripts/bin directory to PATH.",
        version_range_string(),
        MIN_NATIVERELAY_VERSION
    )
}

/// Generate actionable error message for incompatible NativeRelay version
pub fn version_error(version: &NativeRelayVersion, status: CompatibilityStatus) -> String {
    match status {
        CompatibilityStatus::TooOld => format!(
            "NativeRelay version {} is too old. AgentTrace requires {} ≤ version < {}.\n    Upgrade with: pip install --upgrade nativerelay=={}",
            version.version, MIN_NATIVERELAY_VERSION, MAX_NATIVERELAY_VERSION, MIN_NATIVERELAY_VERSION
        ),
        CompatibilityStatus::TooNew => format!(
            "NativeRelay version {} is newer than the tested compatibility range ({} ≤ version < {}).\n    This version may work but is not officially tested. Consider pinning: pip install nativerelay=={}",
            version.version, MIN_NATIVERELAY_VERSION, MAX_NATIVERELAY_VERSION, MIN_NATIVERELAY_VERSION
        ),
        CompatibilityStatus::Compatible => String::new(),
        CompatibilityStatus::Unavailable => String::new(),
    }
}

/// Generate actionable error message for NativeRelay that can't start
pub fn start_error(path: &Path, error: &str, platform_support: PlatformSupport) -> String {
    let mut msg = format!(
        "NativeRelay found at {} but failed to start: {}\n",
        path.display(),
        error
    );

    match platform_support {
        PlatformSupport::Unavailable => {
            msg.push_str("NativeRelay collector is not available on this platform.\n");
            msg.push_str("Supported platforms: Linux (with CN_PROC + fanotify)\n");
        }
        PlatformSupport::Degraded => {
            msg.push_str("NativeRelay may have limited functionality on this platform.\n");
            msg.push_str("On Linux, full support requires root or CAP_NET_ADMIN + CAP_SYS_ADMIN.\n");
        }
        PlatformSupport::Full => {
            msg.push_str("This platform should support NativeRelay. Check the error above.\n");
        }
    }

    msg.push_str("\nTroubleshooting:\n");
    msg.push_str("  1. Verify NativeRelay installation: nativerelay --version\n");
    msg.push_str("  2. Check NativeRelay logs: nativerelay run --format json --scope /tmp\n");
    msg.push_str("  3. Ensure Python dependencies are installed\n");
    msg.push_str("  4. On Linux, run with sufficient privileges for kernel collectors\n");

    msg
}

/// Get the version range string for documentation
fn version_range_string() -> String {
    format!("{} ≤ version < {}", MIN_NATIVERELAY_VERSION, MAX_NATIVERELAY_VERSION)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn event_and_loss_control_records_preserve_native_fields() {
        let e = r#"{"schema_version":1,"id":"evt-7","timestamp":"2026-10-07T12:00:00Z","type":"file.opened","platform":"linux","collector":"fanotify","process":{"pid":42,"parent_pid":1},"resource":{"type":"file","path":"/tmp/a"},"metadata":{"x":1},"evidence":"observed","sequence":7}"#;
        let v = parse_record(e).unwrap();
        assert_eq!(v["id"], "evt-7");
        assert_eq!(v["sequence"], 7);
        assert_eq!(v["timestamp"], "2026-10-07T12:00:00Z");
        assert_eq!(v["process"]["pid"], 42);
        assert_eq!(v["resource"]["path"], "/tmp/a");
        let loss = r#"{"schema_version":1,"record_type":"nativerelay.loss","timestamp":"2026-10-07T12:00:00Z","loss_generation":3,"losses":{"fanotify":{"overflow":{"unknown_count":true}}}}"#;
        assert_eq!(
            parse_record(loss).unwrap()["record_type"],
            "nativerelay.loss"
        );
    }
    #[test]
    fn rejects_bad_json_and_unknown_shapes() {
        assert!(parse_record("not json").is_err());
        assert!(parse_record("[]").is_err());
        assert!(parse_record(r#"{"anything":true}"#).is_err());
    }
    #[test]
    fn reports_malformed_jsonl_lines() {
        use std::io::Cursor;
        let (tx, rx) = mpsc::channel();
        stdout_reader(Cursor::new(b"{broken}\n"), tx);
        assert!(matches!(
            rx.recv_timeout(std::time::Duration::from_secs(1)).unwrap(),
            Message::Malformed { line: 1, .. }
        ));
    }
    #[test]
    fn reads_multiple_jsonl_records_and_skips_blank_lines() {
        use std::io::Cursor;
        let (tx, rx) = mpsc::channel();
        let row = r#"{"schema_version":1,"id":"e","timestamp":"t","type":"file.opened","platform":"linux","collector":"fanotify","process":{"pid":1}}"#;
        stdout_reader(Cursor::new(format!("{row}\n\n{row}\n")), tx);
        let mut events = 0;
        let mut eof = false;
        while let Ok(message) = rx.recv_timeout(std::time::Duration::from_secs(1)) {
            match message {
                Message::Record(v) => {
                    assert_eq!(v["id"], "e");
                    events += 1;
                }
                Message::StdoutEof => {
                    eof = true;
                    break;
                }
                other => panic!("unexpected message: {other:?}"),
            }
        }
        assert_eq!(events, 2);
        assert!(eof);
    }
    #[test]
    fn stderr_is_delivered_as_diagnostics_not_json_records() {
        use std::io::Cursor;
        let (tx, rx) = mpsc::channel();
        stderr_reader(Cursor::new(b"diagnostic only\n"), tx);
        assert!(
            matches!(rx.recv_timeout(std::time::Duration::from_secs(1)).unwrap(), Message::Stderr(line) if line == "diagnostic only")
        );
    }
    #[test]
    fn native_relay_child_process_boundary_keeps_streams_separate() {
        let exe = std::env::current_exe().unwrap();
        let mut child = Process::spawn_command_env(
            exe.as_os_str(),
            &[
                "--exact",
                "nativerelay::tests::native_child_fixture",
                "--nocapture",
            ],
            &[("AGENTTRACE_NATIVE_FIXTURE", "1")],
        )
        .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut got_record = false;
        let mut got_stderr = false;
        let mut stdout_eof = false;
        let mut stderr_eof = false;
        while std::time::Instant::now() < deadline && !(stdout_eof && stderr_eof) {
            if let Some(message) = child.recv_timeout(std::time::Duration::from_millis(100)) {
                match message {
                    Message::Record(v) if v["id"] == "child-event" => got_record = true,
                    Message::Stderr(line) if line.contains("fixture diagnostic") => {
                        got_stderr = true
                    }
                    Message::StdoutEof => stdout_eof = true,
                    Message::StderrEof => stderr_eof = true,
                    _ => {}
                }
            }
        }
        assert!(
            stdout_eof && stderr_eof,
            "both child streams should reach EOF"
        );
        assert!(got_record);
        assert!(got_stderr);
        assert!(child.wait().unwrap().success());
    }
    #[test]
    fn native_child_fixture() {
        if std::env::var_os("AGENTTRACE_NATIVE_FIXTURE").is_some() {
            #[cfg(unix)]
            if std::env::var_os("AGENTTRACE_NATIVE_IGNORE_TERM").is_some() {
                unsafe {
                    libc::signal(libc::SIGTERM, libc::SIG_IGN);
                }
            }
            println!(
                "{}",
                r#"{"schema_version":1,"id":"child-event","timestamp":"2026-10-07T12:00:00Z","type":"file.opened","platform":"linux","collector":"fixture","process":{"pid":1}}"#
            );
            eprintln!("fixture diagnostic");
            if std::env::var_os("AGENTTRACE_NATIVE_EXIT").is_some() {
                std::process::exit(7);
            }
            if std::env::var_os("AGENTTRACE_NATIVE_HOLD").is_some() {
                std::thread::sleep(std::time::Duration::from_secs(15));
            }
        }
    }
    #[test]
    fn child_shutdown_reaps_the_child_process() {
        let exe = std::env::current_exe().unwrap();
        let mut child = Process::spawn_command_env(
            exe.as_os_str(),
            &[
                "--exact",
                "nativerelay::tests::native_child_fixture",
                "--nocapture",
            ],
            &[
                ("AGENTTRACE_NATIVE_FIXTURE", "1"),
                ("AGENTTRACE_NATIVE_HOLD", "1"),
            ],
        )
        .unwrap();
        let _ = child.recv_timeout(std::time::Duration::from_secs(2));
        let _status = child.shutdown().unwrap();
        assert!(child.try_wait().unwrap().is_some());
    }
    #[cfg(unix)]
    #[test]
    fn shutdown_forces_reap_if_child_ignores_sigterm() {
        let exe = std::env::current_exe().unwrap();
        let mut child = Process::spawn_command_env(
            exe.as_os_str(),
            &[
                "--exact",
                "nativerelay::tests::native_child_fixture",
                "--nocapture",
            ],
            &[
                ("AGENTTRACE_NATIVE_FIXTURE", "1"),
                ("AGENTTRACE_NATIVE_HOLD", "1"),
                ("AGENTTRACE_NATIVE_IGNORE_TERM", "1"),
            ],
        )
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut fixture_ready = false;
        while Instant::now() < deadline && !fixture_ready {
            fixture_ready = matches!(
                child.recv_timeout(Duration::from_millis(100)),
                Some(Message::Record(record)) if record["id"] == "child-event"
            );
        }
        assert!(fixture_ready, "fixture did not become ready");
        let started = Instant::now();
        let status = child.shutdown().unwrap();
        assert!(!status.success());
        assert!(started.elapsed() < SHUTDOWN_GRACE_PERIOD + Duration::from_secs(2));
        assert!(child.try_wait().unwrap().is_some());
    }

    #[cfg(unix)]
    #[test]
    fn dropping_adapter_terminates_child() {
        let exe = std::env::current_exe().unwrap();
        let child = Process::spawn_command_env(
            exe.as_os_str(),
            &[
                "--exact",
                "nativerelay::tests::native_child_fixture",
                "--nocapture",
            ],
            &[
                ("AGENTTRACE_NATIVE_FIXTURE", "1"),
                ("AGENTTRACE_NATIVE_HOLD", "1"),
            ],
        )
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut fixture_ready = false;
        while Instant::now() < deadline && !fixture_ready {
            fixture_ready = matches!(
                child.recv_timeout(Duration::from_millis(100)),
                Some(Message::Record(record)) if record["id"] == "child-event"
            );
        }
        assert!(fixture_ready, "fixture did not become ready");
        let pid = child.id() as libc::pid_t;
        drop(child);
        assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
        assert_eq!(io::Error::last_os_error().raw_os_error(), Some(libc::ESRCH));
    }

    #[test]
    fn child_nonzero_exit_is_observable() {
        let exe = std::env::current_exe().unwrap();
        let mut child = Process::spawn_command_env(
            exe.as_os_str(),
            &[
                "--exact",
                "nativerelay::tests::native_child_fixture",
                "--nocapture",
            ],
            &[
                ("AGENTTRACE_NATIVE_FIXTURE", "1"),
                ("AGENTTRACE_NATIVE_EXIT", "7"),
            ],
        )
        .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut eof = false;
        while std::time::Instant::now() < deadline && !eof {
            if matches!(
                child.recv_timeout(std::time::Duration::from_millis(100)),
                Some(Message::StdoutEof)
            ) {
                eof = true;
            }
        }
        assert!(eof);
        assert_eq!(child.wait().unwrap().code(), Some(7));
    }
    #[test]
    fn missing_dependency_error_is_actionable() {
        assert!(missing_error().contains("pip install nativerelay==0.3.2"));
    }

    #[test]
    fn native_relay_version_parse_valid() {
        let v = NativeRelayVersion::parse("0.3.2").unwrap();
        assert_eq!(v.major, 0);
        assert_eq!(v.minor, 3);
        assert_eq!(v.patch, 2);
        assert_eq!(v.version, "0.3.2");
    }

    #[test]
    fn native_relay_version_parse_invalid() {
        assert!(NativeRelayVersion::parse("not-a-version").is_err());
        assert!(NativeRelayVersion::parse("1.2").is_err());
        assert!(NativeRelayVersion::parse("").is_err());
    }

    #[test]
    fn native_relay_version_compatibility() {
        let v = NativeRelayVersion::parse("0.3.2").unwrap();
        assert!(v.is_compatible());
        assert_eq!(v.compatibility_status(), CompatibilityStatus::Compatible);

        let v_old = NativeRelayVersion::parse("0.2.0").unwrap();
        assert!(!v_old.is_compatible());
        assert_eq!(v_old.compatibility_status(), CompatibilityStatus::TooOld);

        let v_new = NativeRelayVersion::parse("0.4.0").unwrap();
        assert!(!v_new.is_compatible());
        assert_eq!(v_new.compatibility_status(), CompatibilityStatus::TooNew);

        let v_newer = NativeRelayVersion::parse("1.0.0").unwrap();
        assert!(!v_newer.is_compatible());
        assert_eq!(v_newer.compatibility_status(), CompatibilityStatus::TooNew);
    }

    #[test]
    fn native_relay_version_ordering() {
        let v1 = NativeRelayVersion::parse("0.3.1").unwrap();
        let v2 = NativeRelayVersion::parse("0.3.2").unwrap();
        let v3 = NativeRelayVersion::parse("0.4.0").unwrap();
        assert!(v1 < v2);
        assert!(v2 < v3);
        assert!(v3 > v1);
    }

    #[test]
    fn parse_status_record_valid() {
        let record = serde_json::json!({
            "schema_version": 1,
            "record_type": "nativerelay.status",
            "timestamp": "2026-10-07T12:00:00Z",
            "state": "running",
            "capabilities": [
                {"event_type": "process.started", "status": "available", "reason": "", "scope": "/tmp"},
                {"event_type": "file.opened", "status": "degraded", "reason": "permission_denied", "scope": "/tmp"}
            ],
            "health": {"dropped": 0, "losses": {}}
        });
        let status = parse_status_record(&record).unwrap();
        assert_eq!(status.state, "running");
        assert_eq!(status.capabilities.len(), 2);
        assert_eq!(status.capabilities[0].event_type, "process.started");
        assert_eq!(status.capabilities[0].status, "available");
        assert_eq!(status.capabilities[1].event_type, "file.opened");
        assert_eq!(status.capabilities[1].status, "degraded");
        assert_eq!(status.capabilities[1].reason, "permission_denied");
        assert_eq!(status.health.dropped, 0);
    }

    #[test]
    fn parse_status_record_missing_state() {
        let record = serde_json::json!({
            "schema_version": 1,
            "record_type": "nativerelay.status",
            "timestamp": "2026-10-07T12:00:00Z",
            "capabilities": [],
            "health": {"dropped": 0, "losses": {}}
        });
        assert!(parse_status_record(&record).is_err());
    }

    #[test]
    fn version_error_messages() {
        let v_old = NativeRelayVersion::parse("0.2.0").unwrap();
        let err = version_error(&v_old, CompatibilityStatus::TooOld);
        assert!(err.contains("too old"));
        assert!(err.contains("0.2.0"));
        assert!(err.contains("0.3.0"));
        assert!(err.contains("pip install --upgrade"));

        let v_new = NativeRelayVersion::parse("0.4.0").unwrap();
        let err = version_error(&v_new, CompatibilityStatus::TooNew);
        assert!(err.contains("newer than the tested"));
        assert!(err.contains("0.4.0"));
        assert!(err.contains("pip install nativerelay==0.3.2"));

        let v_compat = NativeRelayVersion::parse("0.3.2").unwrap();
        let err = version_error(&v_compat, CompatibilityStatus::Compatible);
        assert!(err.is_empty());
    }

    #[test]
    fn missing_error_contains_version_range() {
        let err = missing_error();
        assert!(err.contains("0.3.0 ≤ version < 0.4.0"));
        assert!(err.contains("pip install nativerelay==0.3.2"));
    }
}
