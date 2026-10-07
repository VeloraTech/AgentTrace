use serde_json::Value;
use std::env;
use std::ffi::OsStr;
use std::fs;
use std::io::{self, BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

const SHUTDOWN_GRACE_PERIOD: Duration = Duration::from_secs(2);

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
    pub fn spawn() -> Result<Self, String> {
        let exe = find_executable().ok_or_else(missing_error)?;
        Self::spawn_command(exe.as_os_str(), &["run", "--format", "json"])
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

fn missing_error() -> String {
    "NativeRelay is required but was not found. Install NativeRelay 0.2.0 with:\n    pip install nativerelay==0.2.0\nIf it is already installed, add its Scripts/bin directory to PATH.".to_string()
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

fn find_executable() -> Option<PathBuf> {
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
        assert!(missing_error().contains("pip install nativerelay==0.2.0"));
    }
}
