use std::env;
use std::ffi::OsStr;
use std::collections::{HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader, BufWriter, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use std::process::{Command, Stdio};

use serde_json::{json, Value};
use sysinfo::{Process, System};

const DEFAULT_HISTORY_PATH: &str = ".agenttrace/history.jsonl";
const DEFAULT_INTERVAL_MS: u64 = 1_000;
const MIN_INTERVAL_MS: u64 = 100;
const MAX_INTERVAL_MS: u64 = 60_000;
const MAX_CAPTURE_BYTES_PER_STREAM: usize = 10 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AgentType {
    ClaudeCode,
    Codex,
    GeminiCli,
}

impl AgentType {
    fn label(self) -> &'static str {
        match self {
            Self::ClaudeCode => "Claude Code",
            Self::Codex => "Codex",
            Self::GeminiCli => "Gemini CLI",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ProcessRecord {
    agent_type: Option<AgentType>,
    pid: String,
    parent_pid: Option<String>,
    name: String,
    working_directory: Option<String>,
    executable: Option<String>,
    start_time: u64,
    parent_start_time: Option<u64>,
    command_args: Option<Vec<String>>,
}

struct TraceProcess {
    process: ProcessRecord,
    depth: usize,
}

struct AgentTrace {
    trace_id: String,
    root: ProcessRecord,
    descendants: Vec<TraceProcess>,
}

fn main() {
    let arguments: Vec<String> = env::args().skip(1).collect();
    let trace_help = arguments.first().is_some_and(|command| command == "trace")
        && arguments
            .iter()
            .take_while(|argument| argument.as_str() != "--")
            .any(|argument| argument == "--help" || argument == "-h");
    if trace_help || (arguments.first().is_none_or(|command| command != "trace")
        && arguments.iter().any(|arg| arg == "--help" || arg == "-h"))
    {
        print_help();
        return;
    }
    let result = match arguments.first().map(String::as_str) {
        None => {
            discover_agents();
            Ok(0)
        }
        Some("run") => match run_watch_options(&arguments[1..]) {
            Ok(Some(watch_arguments)) => watch_agents(watch_arguments).map(|()| 0),
            Ok(None) => {
                discover_agents();
                Ok(0)
            }
            Err(error) => Err(error),
        },
        Some("watch") => watch_agents(&arguments[1..]).map(|()| 0),
        Some("history") => print_history(&arguments[1..]).map(|()| 0),
        Some("trace") => trace_command(&arguments[1..]),
        Some(command) => {
            eprintln!("Unknown command: {command}");
            print_help();
            std::process::exit(2);
        }
    };
    match result {
        Ok(exit_code) if exit_code != 0 => std::process::exit(exit_code),
        Ok(_) => {}
        Err(error) => {
            eprintln!("AgentTrace: {error}");
            std::process::exit(1);
        }
    }
}

fn run_watch_options(arguments: &[String]) -> Result<Option<&[String]>, String> {
    match arguments.first().map(String::as_str) {
        None => Ok(None),
        Some("--watch") => Ok(Some(&arguments[1..])),
        Some(option) => Err(format!("unknown run option: {option}; use `agenttrace watch` for live polling")),
    }
}

fn discover_agents() {
    let mut system = System::new_all();
    system.refresh_all();
    let processes = snapshot_processes(&system, false);
    let traces = build_traces(&processes);
    print_traces(&traces);
}

fn snapshot_processes(system: &System, include_command_args: bool) -> Vec<ProcessRecord> {
    let start_times: HashMap<String, u64> = system
        .processes()
        .iter()
        .map(|(pid, process)| (pid.to_string(), process.start_time()))
        .collect();
    system
        .processes()
        .iter()
        .map(|(pid, process)| {
            let parent_pid = process.parent().map(|parent| parent.to_string());
            ProcessRecord {
                agent_type: identify_agent(process),
                pid: pid.to_string(),
                parent_start_time: parent_pid
                    .as_ref()
                    .and_then(|parent| start_times.get(parent).copied()),
                parent_pid,
                name: process.name().to_string_lossy().into_owned(),
                working_directory: process.cwd().map(|path| path.display().to_string()),
                executable: process
                    .exe()
                    .and_then(Path::file_name)
                    .map(|name| name.to_string_lossy().into_owned()),
                start_time: process.start_time(),
                command_args: include_command_args.then(|| {
                    process
                        .cmd()
                        .iter()
                        .map(|argument| argument.to_string_lossy().into_owned())
                        .collect()
                }),
            }
        })
        .collect()
}

fn print_traces(traces: &[AgentTrace]) {
    println!("AgentTrace\n");
    println!("{} agent instance{} tracked", traces.len(), if traces.len() == 1 { "" } else { "s" });
    if traces.is_empty() {
        println!("\nNo supported agent processes found.");
        println!("Supported detectors: Claude Code, Codex, Gemini CLI.");
        return;
    }

    for (index, trace) in traces.iter().enumerate() {
        let agent_type = trace.root.agent_type.expect("trace roots are detected agents");
        println!("\n{}. {}", index + 1, agent_type.label());
        println!("   Trace ID: {}", trace.trace_id);
        println!("   PID: {}", trace.root.pid);
        println!("   Parent PID: {}", trace.root.parent_pid.as_deref().unwrap_or("unavailable"));
        println!("   Working directory: {}", trace.root.working_directory.as_deref().unwrap_or("unavailable"));
        println!("   Executable: {}", trace.root.executable.as_deref().unwrap_or("unavailable"));
        println!("   Started: {}", format_local_datetime(trace.root.start_time));
        println!("   Process tree:");
        if trace.descendants.is_empty() {
            println!("     (no observed child processes)");
        } else {
            for descendant in &trace.descendants {
                let indent = "  ".repeat(descendant.depth.saturating_sub(1));
                println!("     {indent}{} (PID {}, PPID {})", descendant.process.name, descendant.process.pid, descendant.process.parent_pid.as_deref().unwrap_or("unavailable"));
            }
        }
    }
    println!("\nTrace attribution follows observed parent-process relationships.");
    println!("This snapshot does not record file, command, or network activity.");
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ObservedProcess {
    trace_id: String,
    depth: usize,
    process: ProcessRecord,
}

struct WatchOptions {
    output: PathBuf,
    interval: Duration,
    duration: Option<Duration>,
    include_command_args: bool,
    format: WatchOutputFormat,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WatchOutputFormat {
    Pretty,
    Json,
    PrettyJson,
}

struct TraceOptions {
    output: PathBuf,
    working_directory: Option<PathBuf>,
    capture_streams: bool,
    include_command_args: bool,
    watch_files: bool,
    check: bool,
    program: String,
    arguments: Vec<String>,
}

struct TraceEventLog {
    writer: BufWriter<File>,
    session_id: String,
    sequence: u64,
}

impl TraceEventLog {
    fn emit(&mut self, event_type: &str, data: Value) -> io::Result<()> {
        let timestamp = unix_time_millis();
        let timestamp_iso8601 = local_timestamp();
        let mut event = json!({
            "schema_version": 1,
            "session_id": self.session_id,
            "cycle_id": self.session_id,
            "sequence": self.sequence,
            "timestamp": timestamp_iso8601,
            "timestamp_unix_ms": timestamp,
            "observed_at_unix_ms": timestamp,
            "collector": "supervised_execution",
            "event_type": event_type,
            "data": data
        });
        if event_type.starts_with("check.") {
            if let Some(fields) = event["data"].as_object().cloned() {
                let object = event.as_object_mut().expect("event envelope is an object");
                object.insert("type".to_string(), json!(event_type));
                object.remove("event_type");
                object.remove("data");
                object.extend(fields);
            }
        }
        serde_json::to_writer(&mut self.writer, &event)?;
        self.writer.write_all(b"\n")?;
        self.writer.flush()?;
        if matches!(event_type, "process_discovered" | "process_updated" | "process_unobserved" | "file_changed" | "collector_status")
            && io::stderr().is_terminal()
        {
            let color = env::var_os("NO_COLOR").is_none() && env::var("TERM").as_deref() != Ok("dumb");
            eprintln!("{}", render_history_event(&event, color));
        }
        self.sequence += 1;
        Ok(())
    }
}

fn trace_command(arguments: &[String]) -> Result<i32, String> {
    let options = parse_trace_options(arguments)?;
    if let Some(parent) = options.output.parent().filter(|path| !path.as_os_str().is_empty()) {
        fs::create_dir_all(parent).map_err(|error| format!("cannot create history directory: {error}"))?;
    }
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&options.output)
        .map_err(|error| format!("cannot open history file {}: {error}", options.output.display()))?;
    let cwd_path = options
        .working_directory
        .clone()
        .or_else(|| env::current_dir().ok());
    let mut file_snapshot = if options.watch_files {
        Some(scan_workspace_files(cwd_path.as_deref().unwrap_or(Path::new("."))))
    } else {
        None
    };
    let session_id = format!("{}-{}", std::process::id(), unix_time_nanos());
    let log = Arc::new(Mutex::new(TraceEventLog {
        writer: BufWriter::new(file),
        session_id: session_id.clone(),
        sequence: 0,
    }));

    log.lock()
        .map_err(|_| "event logger lock was poisoned".to_string())?
        .emit("session_started", json!({
            "collector": "supervised_process",
            "history_file": options.output,
            "command_args_recorded": options.include_command_args,
            "stream_contents_recorded": options.capture_streams,
            "coverage": {
                "observes_only_launched_process": true,
                "child_process_lifecycle": "polled_descendants; short-lived children may be missed",
                "file_events": if options.watch_files { "workspace metadata changes only" } else { "disabled" },
                "file_open_read_events": false,
                "network_events": false
            }
        }))
        .map_err(|error| format!("cannot write session start event: {error}"))?;

    let mut command = Command::new(&options.program);
    command
        .args(&options.arguments)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(directory) = &options.working_directory {
        command.current_dir(directory);
    }
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            if let Ok(mut logger) = log.lock() {
                let _ = logger.emit("session_failed", json!({
                    "reason": "process_spawn_failed",
                    "program": options.program,
                    "error": error.to_string()
                }));
                let _ = logger.emit("session_stopped", json!({ "reason": "process_spawn_failed" }));
            }
            return Err(format!("cannot launch {}: {error}", options.program));
        }
    };
    let pid = child.id();
    let started_at = unix_time_millis();
    let trace_id = format!("trace-{pid}:{started_at}");
    let cwd = cwd_path.as_ref().map(|path| path.display().to_string());
    let mut process_data = json!({
        "trace_id": trace_id,
        "process_identity": { "pid": pid, "start_time_unix_ms": started_at },
        "parent_identity": { "pid": std::process::id() },
        "executable": options.program,
        "working_directory": cwd,
        "command_args_recorded": options.include_command_args,
        "stream_contents_recorded": options.capture_streams
    });
    if options.include_command_args {
        process_data["command_args"] = json!(options.arguments);
    }
    log.lock()
        .map_err(|_| "event logger lock was poisoned".to_string())?
        .emit("process_started", process_data)
        .map_err(|error| format!("cannot write process start event: {error}"))?;
    let check_started_at = local_timestamp();
    if options.check {
        let command = traced_command(&options.program, &options.arguments, options.include_command_args);
        log.lock()
            .map_err(|_| "event logger lock was poisoned".to_string())?
            .emit("check.started", check_started_data(&command, pid, check_started_at.clone()))
        .map_err(|error| format!("cannot write check start event: {error}"))?;
    }
    if options.capture_streams {
        eprintln!("Warning: stream contents are being recorded verbatim and may contain secrets or private source code.");
    }
    let terminal_color = io::stderr().is_terminal()
        && env::var_os("NO_COLOR").is_none()
        && env::var("TERM").as_deref() != Ok("dumb");
    let process_label = traced_command(&options.program, &options.arguments, options.include_command_args);
    eprintln!("{}", paint(
        &format!("{} process.started  {process_label}  PID {pid}", format_local_time(unix_time_millis())),
        "36",
        terminal_color,
    ));
    if options.check {
        eprintln!("{}", paint(
            &format!("{} check.started    {process_label}  PID {pid}", format_local_time(unix_time_millis())),
            "33",
            terminal_color,
        ));
    }
    eprintln!("Events: {}", options.output.display());

    let stdin = child.stdin.take().expect("piped child stdin");
    let stdin_log = Arc::clone(&log);
    let stream_error = Arc::new(AtomicBool::new(false));
    let stdin_error = Arc::clone(&stream_error);
    let stdin_trace_id = trace_id.clone();
    let capture_stdin = options.capture_streams;
    let stdin_thread = thread::spawn(move || {
        let mut child_stdin = stdin;
        let mut input = io::stdin().lock();
        let mut buffer = [0_u8; 8192];
        let mut captured = 0_usize;
        loop {
            let count = match input.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(count) => count,
            };
            if child_stdin.write_all(&buffer[..count]).is_err() {
                break;
            }
            let allowed = if capture_stdin {
                count.min(MAX_CAPTURE_BYTES_PER_STREAM.saturating_sub(captured))
            } else {
                0
            };
            captured += allowed;
            let mut data = json!({ "trace_id": stdin_trace_id, "stream": "stdin", "byte_count": count, "captured": allowed > 0 });
            if allowed > 0 {
                data["encoding"] = json!("hex");
                data["data_hex"] = json!(hex_encode(&buffer[..allowed]));
            }
            if capture_stdin && allowed < count {
                data["truncated"] = json!(true);
            }
            if let Ok(mut logger) = stdin_log.lock() {
                if logger.emit("stream_activity", data).is_err() {
                    stdin_error.store(true, Ordering::SeqCst);
                }
            } else {
                stdin_error.store(true, Ordering::SeqCst);
            }
        }
    });

    let stdout_thread = spawn_output_forwarder(
        child.stdout.take().expect("piped child stdout"),
        io::stdout(),
        Arc::clone(&log),
        Arc::clone(&stream_error),
        trace_id.clone(),
        "stdout",
        options.capture_streams,
    );
    let stderr_thread = spawn_output_forwarder(
        child.stderr.take().expect("piped child stderr"),
        io::stderr(),
        Arc::clone(&log),
        Arc::clone(&stream_error),
        trace_id.clone(),
        "stderr",
        options.capture_streams,
    );

    let mut system = System::new_all();
    let mut previous_descendants = HashMap::new();
    let status = loop {
        system.refresh_all();
        let records = snapshot_processes(&system, options.include_command_args);
        let descendants = supervised_descendants(&records, &pid.to_string(), &trace_id);
        emit_trace_process_changes(&log, &previous_descendants, &descendants)
            .map_err(|error| format!("cannot write child process event: {error}"))?;
        previous_descendants = descendants;
        if let (Some(root), Some(previous)) = (cwd_path.as_deref(), file_snapshot.as_mut()) {
            let current = scan_workspace_files(root);
            emit_file_changes(&log, &trace_id, previous, &current)
                .map_err(|error| format!("cannot write file event: {error}"))?;
            *previous = current;
        }
        if let Some(exit_status) = child
            .try_wait()
            .map_err(|error| format!("cannot wait for traced process: {error}"))?
        {
            break exit_status;
        }
        thread::sleep(Duration::from_millis(250));
    };
    system.refresh_all();
    let records = snapshot_processes(&system, options.include_command_args);
    let descendants = supervised_descendants(&records, &pid.to_string(), &trace_id);
    emit_trace_process_changes(&log, &previous_descendants, &descendants)
        .map_err(|error| format!("cannot write child process event: {error}"))?;
    if let (Some(root), Some(previous)) = (cwd_path.as_deref(), file_snapshot.as_mut()) {
        let current = scan_workspace_files(root);
        emit_file_changes(&log, &trace_id, previous, &current)
            .map_err(|error| format!("cannot write file event: {error}"))?;
    }
    let ended_at = unix_time_millis();
    for handle in [stdout_thread, stderr_thread] {
        if handle.join().is_err() {
            eprintln!("Warning: a stream forwarding thread ended unexpectedly.");
        }
    }
    drop(stdin_thread);
    if stream_error.load(Ordering::SeqCst) {
        eprintln!("Warning: one or more stream bytes or stream events may not have been recorded.");
        log.lock()
            .map_err(|_| "event logger lock was poisoned".to_string())?
            .emit("collector_status", json!({
                "collector": "stream_forwarder",
                "status": "incomplete",
                "reason": "stream_forward_or_event_write_failed"
            }))
            .map_err(|error| format!("cannot write stream collector status: {error}"))?;
    }
    let exit_code = status.code();
    log.lock()
        .map_err(|_| "event logger lock was poisoned".to_string())?
        .emit("process_exited", json!({
            "trace_id": trace_id,
            "executable": options.program,
            "process_identity": { "pid": pid, "start_time_unix_ms": started_at },
            "ended_at_unix_ms": ended_at,
            "duration_ms": ended_at.saturating_sub(started_at),
            "exit_code": exit_code,
            "exit_code_available": exit_code.is_some()
        }))
        .map_err(|error| format!("cannot write process exit event: {error}"))?;
    if options.check {
        let completed_at = local_timestamp();
        log.lock()
            .map_err(|_| "event logger lock was poisoned".to_string())?
            .emit("check.completed", check_completed_data(
                &traced_command(&options.program, &options.arguments, options.include_command_args),
                pid,
                exit_code,
                check_started_at,
                completed_at,
                ended_at.saturating_sub(started_at),
            ))
            .map_err(|error| format!("cannot write check completion event: {error}"))?;
    }
    let exit_label = exit_code.map(|code| format!("exit {code}")).unwrap_or_else(|| "exit status unavailable".to_string());
    eprintln!("{}", paint(
        &format!("{} process.exited   {process_label}  PID {pid}  {exit_label}  {} ms", format_local_time(ended_at), ended_at.saturating_sub(started_at)),
        if exit_code.is_some_and(|code| code != 0) { "31" } else { "36" },
        terminal_color,
    ));
    if options.check {
        eprintln!("{}", paint(
            &format!("{} check.completed {process_label}  exit {}  {} ms", format_local_time(ended_at), exit_code.map_or_else(|| "unknown".to_string(), |code| code.to_string()), ended_at.saturating_sub(started_at)),
            if exit_code == Some(0) { "32" } else { "31" },
            terminal_color,
        ));
    }
    log.lock()
        .map_err(|_| "event logger lock was poisoned".to_string())?
        .emit("session_stopped", json!({ "reason": "process_exited" }))
        .map_err(|error| format!("cannot write session stop event: {error}"))?;
    Ok(exit_code.unwrap_or(1))
}

fn spawn_output_forwarder<R, W>(
    mut source: R,
    mut destination: W,
    log: Arc<Mutex<TraceEventLog>>,
    error_flag: Arc<AtomicBool>,
    trace_id: String,
    stream: &'static str,
    capture: bool,
) -> thread::JoinHandle<()>
where
    R: Read + Send + 'static,
    W: Write + Send + 'static,
{
    thread::spawn(move || {
        let mut buffer = [0_u8; 8192];
        let mut captured = 0_usize;
        loop {
            let count = match source.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(count) => count,
            };
            if destination.write_all(&buffer[..count]).is_err() || destination.flush().is_err() {
                error_flag.store(true, Ordering::SeqCst);
                break;
            }
            let allowed = if capture {
                count.min(MAX_CAPTURE_BYTES_PER_STREAM.saturating_sub(captured))
            } else {
                0
            };
            captured += allowed;
            let mut data = json!({ "trace_id": trace_id, "stream": stream, "byte_count": count, "captured": allowed > 0 });
            if allowed > 0 {
                data["encoding"] = json!("hex");
                data["data_hex"] = json!(hex_encode(&buffer[..allowed]));
            }
            if capture && allowed < count {
                data["truncated"] = json!(true);
            }
            if let Ok(mut logger) = log.lock() {
                if logger.emit("stream_activity", data).is_err() {
                    error_flag.store(true, Ordering::SeqCst);
                }
            } else {
                error_flag.store(true, Ordering::SeqCst);
            }
        }
    })
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct FileStamp {
    size_bytes: u64,
    modified_unix_ms: Option<u64>,
}

struct WorkspaceSnapshot {
    files: HashMap<String, FileStamp>,
    scan_errors: usize,
    capped: bool,
}

fn scan_workspace_files(root: &Path) -> WorkspaceSnapshot {
    const MAX_FILES: usize = 10_000;
    let mut snapshot = WorkspaceSnapshot {
        files: HashMap::new(),
        scan_errors: 0,
        capped: false,
    };
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(_) => {
                snapshot.scan_errors += 1;
                continue;
            }
        };
        for entry in entries {
            if snapshot.files.len() >= MAX_FILES {
                snapshot.capped = true;
                return snapshot;
            }
            let entry = match entry {
                Ok(entry) => entry,
                Err(_) => {
                    snapshot.scan_errors += 1;
                    continue;
                }
            };
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if [".git", ".agenttrace", "node_modules", "target", ".venv"]
                .contains(&name.as_ref())
            {
                continue;
            }
            let file_type = match entry.file_type() {
                Ok(file_type) => file_type,
                Err(_) => {
                    snapshot.scan_errors += 1;
                    continue;
                }
            };
            if file_type.is_symlink() {
                continue;
            }
            if file_type.is_dir() {
                pending.push(entry.path());
                continue;
            }
            if !file_type.is_file() {
                continue;
            }
            let metadata = match entry.metadata() {
                Ok(metadata) => metadata,
                Err(_) => {
                    snapshot.scan_errors += 1;
                    continue;
                }
            };
            let relative_path = entry
                .path()
                .strip_prefix(root)
                .unwrap_or(entry.path().as_path())
                .to_string_lossy()
                .into_owned();
            let modified_unix_ms = metadata
                .modified()
                .ok()
                .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                .map(|duration| duration.as_millis() as u64);
            snapshot.files.insert(
                relative_path,
                FileStamp {
                    size_bytes: metadata.len(),
                    modified_unix_ms,
                },
            );
        }
    }
    snapshot
}

fn emit_file_changes(
    log: &Arc<Mutex<TraceEventLog>>,
    trace_id: &str,
    previous: &WorkspaceSnapshot,
    current: &WorkspaceSnapshot,
) -> io::Result<()> {
    let mut paths: Vec<_> = current.files.keys().collect();
    paths.sort();
    for path in paths {
        let stamp = &current.files[path];
        let action = match previous.files.get(path) {
            None => Some("created"),
            Some(previous_stamp) if previous_stamp != stamp => Some("modified"),
            Some(_) => None,
        };
        if let Some(action) = action {
            log.lock()
                .map_err(|_| io::Error::other("event logger lock was poisoned"))?
                .emit(
                    "file_changed",
                    json!({
                        "trace_id": trace_id,
                        "path": path,
                        "action": action,
                        "size_bytes": stamp.size_bytes,
                        "modified_unix_ms": stamp.modified_unix_ms,
                        "attribution": "observed_workspace_delta_during_supervised_command",
                        "content_recorded": false
                    }),
                )?;
        }
    }
    let mut deleted: Vec<_> = previous
        .files
        .keys()
        .filter(|path| !current.files.contains_key(*path))
        .collect();
    deleted.sort();
    for path in deleted {
        log.lock()
            .map_err(|_| io::Error::other("event logger lock was poisoned"))?
            .emit(
                "file_changed",
                json!({
                    "trace_id": trace_id,
                    "path": path,
                    "action": "deleted",
                    "attribution": "observed_workspace_delta_during_supervised_command",
                    "content_recorded": false
                }),
            )?;
    }
    if current.scan_errors > 0 || current.capped {
        log.lock()
            .map_err(|_| io::Error::other("event logger lock was poisoned"))?
            .emit(
                "collector_status",
                json!({
                    "collector": "workspace_metadata_polling",
                    "status": "incomplete",
                    "scan_errors": current.scan_errors,
                    "file_limit_reached": current.capped,
                    "coverage": "metadata changes only; file open/read events are unavailable"
                }),
            )?;
    }
    Ok(())
}

fn supervised_descendants(
    processes: &[ProcessRecord],
    root_pid: &str,
    trace_id: &str,
) -> HashMap<String, ObservedProcess> {
    let by_pid: HashMap<&str, &ProcessRecord> = processes
        .iter()
        .map(|process| (process.pid.as_str(), process))
        .collect();
    let mut observed = HashMap::new();
    for process in processes.iter().filter(|process| process.pid != root_pid) {
        let mut parent = process.parent_pid.as_deref();
        let mut depth = 1;
        let mut visited = HashSet::new();
        while let Some(parent_pid) = parent {
            if parent_pid == root_pid {
                observed.insert(
                    instance_key(&process.pid, process.start_time),
                    ObservedProcess {
                        trace_id: trace_id.to_string(),
                        depth,
                        process: process.clone(),
                    },
                );
                break;
            }
            if !visited.insert(parent_pid) {
                break;
            }
            let Some(ancestor) = by_pid.get(parent_pid) else {
                break;
            };
            depth += 1;
            parent = ancestor.parent_pid.as_deref();
        }
    }
    observed
}

fn emit_trace_process_changes(
    log: &Arc<Mutex<TraceEventLog>>,
    previous: &HashMap<String, ObservedProcess>,
    current: &HashMap<String, ObservedProcess>,
) -> io::Result<()> {
    let mut identities: Vec<_> = current.keys().collect();
    identities.sort();
    for identity in identities {
        let process = &current[identity];
        let event_type = match previous.get(identity) {
            None => Some("process_discovered"),
            Some(old) if old != process => Some("process_updated"),
            Some(_) => None,
        };
        if let Some(event_type) = event_type {
            log.lock()
                .map_err(|_| io::Error::other("event logger lock was poisoned"))?
                .emit(event_type, process_event_data(process))?;
        }
    }
    let mut disappeared: Vec<_> = previous
        .keys()
        .filter(|identity| !current.contains_key(*identity))
        .collect();
    disappeared.sort();
    for identity in disappeared {
        let process = &previous[identity];
        let mut data = process_event_data(process);
        data["exit_code"] = Value::Null;
        data["exit_code_available"] = Value::Bool(false);
        data["exit_observation"] = Value::String(
            "no longer observed by polling; exact exit status unavailable".to_string(),
        );
        log.lock()
            .map_err(|_| io::Error::other("event logger lock was poisoned"))?
            .emit("process_unobserved", data)?;
    }
    Ok(())
}

fn parse_trace_options(arguments: &[String]) -> Result<TraceOptions, String> {
    let separator = arguments.iter().position(|argument| argument == "--")
        .ok_or("trace requires `-- PROGRAM [ARGS...]`")?;
    let mut output = PathBuf::from(DEFAULT_HISTORY_PATH);
    let mut working_directory = None;
    let mut capture_streams = false;
    let mut include_command_args = false;
    let mut watch_files = false;
    let mut check = false;
    let mut index = 0;
    while index < separator {
        match arguments[index].as_str() {
            "--output" | "--file" => {
                index += 1;
                output = PathBuf::from(arguments.get(index).filter(|_| index < separator).ok_or("--output requires a path")?);
            }
            "--cwd" | "--working-directory" => {
                index += 1;
                working_directory = Some(PathBuf::from(arguments.get(index).filter(|_| index < separator).ok_or("--cwd requires a path")?));
            }
            "--capture-streams" => capture_streams = true,
            "--include-command-args" => include_command_args = true,
            "--watch-files" => watch_files = true,
            "--check" => check = true,
            option => return Err(format!("unknown trace option: {option}")),
        }
        index += 1;
    }
    let command_parts = arguments.get(separator + 1..).ok_or("trace requires a command after `--`")?;
    let (program, command_arguments) = command_parts.split_first().ok_or("trace requires a command after `--`")?;
    Ok(TraceOptions {
        output,
        working_directory,
        capture_streams,
        include_command_args,
        watch_files,
        check,
        program: program.clone(),
        arguments: command_arguments.to_vec(),
    })
}

fn watch_agents(arguments: &[String]) -> Result<(), String> {
    let options = parse_watch_options(arguments)?;
    if let Some(parent) = options.output.parent().filter(|path| !path.as_os_str().is_empty()) {
        fs::create_dir_all(parent).map_err(|error| format!("cannot create history directory: {error}"))?;
    }
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&options.output)
        .map_err(|error| format!("cannot open history file {}: {error}", options.output.display()))?;
    let mut history = BufWriter::new(file);
    let running = Arc::new(AtomicBool::new(true));
    let signal_state = Arc::clone(&running);
    ctrlc::set_handler(move || signal_state.store(false, Ordering::SeqCst))
        .map_err(|error| format!("cannot install Ctrl+C handler: {error}"))?;

    let session_id = format!("{}-{}", std::process::id(), unix_time_nanos());
    let mut sequence = 0_u64;
    let terminal = io::stdout();
    let color = terminal.is_terminal()
        && env::var_os("NO_COLOR").is_none()
        && env::var("TERM").as_deref() != Ok("dumb");
    let mut stdout = terminal.lock();
    write_event(
        &mut history,
        &mut stdout,
        &session_id,
        &mut sequence,
        "session_started",
        json!({
            "collector": "sysinfo_polling",
            "history_file": options.output,
            "poll_interval_ms": options.interval.as_millis(),
            "duration_ms": options.duration.map(|duration| duration.as_millis()),
            "command_args_recorded": options.include_command_args,
            "coverage": {
                "short_lived_processes_may_be_missed": true,
                "exit_codes": false,
                "stdin": false,
                "stdout": false,
                "stderr": false,
                "file_events": false
            }
        }),
        options.format,
        color,
        options.format != WatchOutputFormat::PrettyJson,
    )
    .map_err(|error| format!("cannot write session start event: {error}"))?;
    if options.format == WatchOutputFormat::Json {
        eprintln!("Recording process events to {}. Press Ctrl+C to stop.", options.output.display());
    }
    if options.include_command_args {
        eprintln!("Command arguments are being written verbatim and may contain secrets.");
    }

    let mut system = System::new_all();
    let mut previous = HashMap::new();
    let started_at = Instant::now();
    let mut stop_reason = "ctrl_c";
    let mut initial_snapshot = true;
    loop {
        if !running.load(Ordering::SeqCst) {
            break;
        }
        if options.duration.is_some_and(|duration| started_at.elapsed() >= duration) {
            stop_reason = "duration_elapsed";
            break;
        }
        system.refresh_all();
        let process_records = snapshot_processes(&system, options.include_command_args);
        let traces = build_traces(&process_records);
        let mut current = observed_processes(&traces);
        retain_live_attributions(&mut current, &previous, &process_records);
        write_process_changes(
            &mut history,
            &mut stdout,
            &session_id,
            &mut sequence,
            &previous,
            &current,
            options.format,
            color,
            initial_snapshot,
        )
        .map_err(|error| format!("cannot write process event: {error}"))?;
        if initial_snapshot && options.format == WatchOutputFormat::Pretty {
            stdout
                .write_all(render_watch_snapshot(&traces, &session_id, options.interval, color).as_bytes())
                .map_err(|error| format!("cannot write initial process snapshot: {error}"))?;
            stdout.flush().map_err(|error| format!("cannot flush initial process snapshot: {error}"))?;
        } else if initial_snapshot && options.format == WatchOutputFormat::PrettyJson {
            serde_json::to_writer_pretty(&mut stdout, &watch_snapshot_json(&traces))
                .map_err(|error| format!("cannot format initial JSON snapshot: {error}"))?;
            stdout.write_all(b"\n\n")
                .and_then(|()| stdout.flush())
                .map_err(|error| format!("cannot write initial JSON snapshot: {error}"))?;
        }
        initial_snapshot = false;
        previous = current;

        let mut remaining = options.interval;
        if let Some(duration) = options.duration {
            remaining = remaining.min(duration.saturating_sub(started_at.elapsed()));
        }
        while !remaining.is_zero() && running.load(Ordering::SeqCst) {
            let pause = remaining.min(Duration::from_millis(100));
            thread::sleep(pause);
            remaining = remaining.saturating_sub(pause);
        }
    }

    write_event(
        &mut history,
        &mut stdout,
        &session_id,
        &mut sequence,
        "session_stopped",
        json!({ "reason": stop_reason }),
        options.format,
        color,
        true,
    )
    .map_err(|error| format!("cannot write session stop event: {error}"))?;
    Ok(())
}

fn parse_watch_options(arguments: &[String]) -> Result<WatchOptions, String> {
    let mut output = PathBuf::from(DEFAULT_HISTORY_PATH);
    let mut interval_ms = DEFAULT_INTERVAL_MS;
    let mut duration_ms = None;
    let mut include_command_args = false;
    let mut format = WatchOutputFormat::Pretty;
    let mut index = 0;
    while index < arguments.len() {
        match arguments[index].as_str() {
            "--output" | "--file" => {
                index += 1;
                let value = arguments.get(index).ok_or("--output requires a path")?;
                output = PathBuf::from(value);
            }
            "--interval-ms" => {
                index += 1;
                let value = arguments.get(index).ok_or("--interval-ms requires a number")?;
                interval_ms = value
                    .parse::<u64>()
                    .map_err(|_| "--interval-ms must be a positive integer")?;
                if interval_ms < MIN_INTERVAL_MS {
                    return Err(format!("--interval-ms must be at least {MIN_INTERVAL_MS}"));
                }
                if interval_ms > MAX_INTERVAL_MS {
                    return Err(format!("--interval-ms must be at most {MAX_INTERVAL_MS}"));
                }
            }
            "--duration-ms" => {
                index += 1;
                let value = arguments.get(index).ok_or("--duration-ms requires a number")?;
                let duration = value
                    .parse::<u64>()
                    .map_err(|_| "--duration-ms must be a positive integer")?;
                if duration == 0 {
                    return Err("--duration-ms must be greater than zero".to_string());
                }
                duration_ms = Some(duration);
            }
            "--include-command-args" => include_command_args = true,
            "--json" => format = WatchOutputFormat::Json,
            "--pretty-json" => format = WatchOutputFormat::PrettyJson,
            "--format" => {
                index += 1;
                format = match arguments.get(index).map(String::as_str) {
                    Some("pretty") => WatchOutputFormat::Pretty,
                    Some("json") => WatchOutputFormat::Json,
                    Some("pretty-json") => WatchOutputFormat::PrettyJson,
                    Some(_) => return Err("--format must be `pretty`, `json`, or `pretty-json`".to_string()),
                    None => return Err("--format requires `pretty`, `json`, or `pretty-json`".to_string()),
                };
            }
            option => return Err(format!("unknown watch option: {option}")),
        }
        index += 1;
    }
    Ok(WatchOptions {
        output,
        interval: Duration::from_millis(interval_ms),
        duration: duration_ms.map(Duration::from_millis),
        include_command_args,
        format,
    })
}

fn observed_processes(traces: &[AgentTrace]) -> HashMap<String, ObservedProcess> {
    let mut observed = HashMap::new();
    for trace in traces {
        observed.insert(
            instance_key(&trace.root.pid, trace.root.start_time),
            ObservedProcess {
                trace_id: trace.trace_id.clone(),
                depth: 0,
                process: trace.root.clone(),
            },
        );
        for descendant in &trace.descendants {
            observed.insert(
                instance_key(&descendant.process.pid, descendant.process.start_time),
                ObservedProcess {
                    trace_id: trace.trace_id.clone(),
                    depth: descendant.depth,
                    process: descendant.process.clone(),
                },
            );
        }
    }
    observed
}

fn retain_live_attributions(
    current: &mut HashMap<String, ObservedProcess>,
    previous: &HashMap<String, ObservedProcess>,
    live_processes: &[ProcessRecord],
) {
    let live_by_identity: HashMap<String, &ProcessRecord> = live_processes
        .iter()
        .map(|process| (instance_key(&process.pid, process.start_time), process))
        .collect();
    for (identity, previous_observation) in previous {
        if current.contains_key(identity) {
            continue;
        }
        let Some(live_process) = live_by_identity.get(identity) else {
            continue;
        };
        let mut process = (**live_process).clone();
        if process.agent_type.is_none() {
            process.agent_type = previous_observation.process.agent_type;
        }
        current.insert(
            identity.clone(),
            ObservedProcess {
                trace_id: previous_observation.trace_id.clone(),
                depth: previous_observation.depth,
                process,
            },
        );
    }
}

fn write_process_changes(
    history: &mut impl Write,
    stdout: &mut impl Write,
    session_id: &str,
    sequence: &mut u64,
    previous: &HashMap<String, ObservedProcess>,
    current: &HashMap<String, ObservedProcess>,
    format: WatchOutputFormat,
    color: bool,
    initial_snapshot: bool,
) -> io::Result<()> {
    let mut identities: Vec<_> = current.keys().collect();
    identities.sort_by(|left, right| {
        current[*left]
            .trace_id
            .cmp(&current[*right].trace_id)
            .then_with(|| current[*left].depth.cmp(&current[*right].depth))
            .then_with(|| current[*left].process.pid.cmp(&current[*right].process.pid))
            .then_with(|| current[*left].process.start_time.cmp(&current[*right].process.start_time))
    });
    for identity in identities {
        let process = &current[identity];
        let event_type = match previous.get(identity) {
            None => Some("process_discovered"),
            Some(previous_process) if previous_process != process => Some("process_updated"),
            Some(_) => None,
        };
        if let Some(event_type) = event_type {
            write_event(
                history,
                stdout,
                session_id,
                sequence,
                event_type,
                process_event_data(process),
                format,
                color,
                !initial_snapshot,
            )?;
        }
    }

    let mut disappeared: Vec<_> = previous
        .keys()
        .filter(|identity| !current.contains_key(*identity))
        .collect();
    disappeared.sort_by(|left, right| {
        previous[*left]
            .trace_id
            .cmp(&previous[*right].trace_id)
            .then_with(|| previous[*left].depth.cmp(&previous[*right].depth))
            .then_with(|| previous[*left].process.pid.cmp(&previous[*right].process.pid))
    });
    for identity in disappeared {
        let process = &previous[identity];
        let mut data = process_event_data(process);
        data["exit_code"] = Value::Null;
        data["exit_code_available"] = Value::Bool(false);
        data["exit_observation"] = Value::String("not observed in this poll; may have exited or become inaccessible".to_string());
        write_event(
            history,
            stdout,
            session_id,
            sequence,
            "process_unobserved",
            data,
            format,
            color,
            true,
        )?;
    }
    Ok(())
}

fn process_event_data(observed: &ObservedProcess) -> Value {
    let process = &observed.process;
    let mut data = json!({
        "trace_id": observed.trace_id,
        "process_identity": {
            "pid": process.pid,
            "start_time_unix_s": process.start_time
        },
        "parent_identity": process.parent_pid.as_ref().map(|pid| json!({
            "pid": pid,
            "start_time_unix_s": process.parent_start_time
        })),
        "process_name": process.name,
        "agent_type": process.agent_type.map(AgentType::label),
        "depth_from_agent": observed.depth,
        "executable": process.executable,
        "working_directory": process.working_directory
    });
    if let Some(arguments) = &process.command_args {
        data["command_args"] = json!(arguments);
    }
    data
}

fn write_event(
    history: &mut impl Write,
    stdout: &mut impl Write,
    session_id: &str,
    sequence: &mut u64,
    event_type: &str,
    data: Value,
    format: WatchOutputFormat,
    color: bool,
    emit_pretty_output: bool,
) -> io::Result<()> {
    let timestamp = unix_time_millis();
    let timestamp_iso8601 = local_timestamp();
    let event = json!({
        "schema_version": 1,
        "session_id": session_id,
        "sequence": *sequence,
        "timestamp": timestamp_iso8601,
        "timestamp_unix_ms": timestamp,
        "observed_at_unix_ms": timestamp,
        "collector": "sysinfo_polling",
        "event_type": event_type,
        "data": data
    });
    serde_json::to_writer(&mut *history, &event)?;
    history.write_all(b"\n")?;
    history.flush()?;
    let emit_output = match format {
        WatchOutputFormat::Pretty | WatchOutputFormat::PrettyJson => emit_pretty_output,
        WatchOutputFormat::Json => true,
    };
    if emit_output {
        match format {
            WatchOutputFormat::Json => serde_json::to_writer(&mut *stdout, &event)?,
            WatchOutputFormat::PrettyJson => {
                serde_json::to_writer_pretty(&mut *stdout, &event)?;
                stdout.write_all(b"\n")?;
            }
            WatchOutputFormat::Pretty => {
                stdout.write_all(render_watch_event(&event, color).as_bytes())?
            }
        }
        stdout.write_all(b"\n")?;
        stdout.flush()?;
    }
    *sequence += 1;
    Ok(())
}

fn render_watch_event(event: &Value, color: bool) -> String {
    let sequence = event["sequence"].as_u64().unwrap_or_default();
    let event_type = event["event_type"].as_str().unwrap_or("event");
    let data = &event["data"];
    let (line, color_code) = match event_type {
        "session_started" => (
            format!(
                "History: {}",
                data["history_file"].as_str().unwrap_or(DEFAULT_HISTORY_PATH),
            ),
            "36",
        ),
        "session_stopped" => (
            format!("Polling stopped ({})", data["reason"].as_str().unwrap_or("unknown reason")),
            "36",
        ),
        "collector_status" => (
            format!(
                "{} collector {}{}",
                data["collector"].as_str().unwrap_or("unknown"),
                data["status"].as_str().unwrap_or("status unknown"),
                data["reason"].as_str().map(|reason| format!(": {reason}")).unwrap_or_default()
            ),
            "33",
        ),
        "process_discovered" | "process_updated" | "process_unobserved" => {
            let depth = data["depth_from_agent"].as_u64().unwrap_or_default() as usize;
            let indent = "  ".repeat(depth);
            let process_name = data["process_name"].as_str().unwrap_or("process");
            let agent_type = data["agent_type"].as_str();
            let label = if depth == 0 {
                agent_type.unwrap_or(process_name).to_string()
            } else if let Some(arguments) = data["command_args"].as_array() {
                arguments.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(" ")
            } else {
                process_name.to_string()
            };
            let pid = data["process_identity"]["pid"]
                .as_str()
                .map(str::to_string)
                .or_else(|| data["process_identity"]["pid"].as_u64().map(|pid| pid.to_string()))
                .unwrap_or_else(|| "?".to_string());
            let suffix = if event_type == "process_unobserved" {
                " (no longer observed; exit status unavailable)"
            } else if event_type == "process_updated" {
                " (metadata updated)"
            } else {
                ""
            };
            let parent_pid = data["parent_identity"]["pid"].as_str();
            let parent = parent_pid.map(|pid| format!(", PPID {pid}")).unwrap_or_default();
            let trace = if depth > 0 {
                agent_type.map(|agent| format!(" · {agent}")).unwrap_or_default()
            } else {
                String::new()
            };
            let event_label = match event_type {
                "process_discovered" => "process.started",
                "process_updated" => "process.updated",
                _ => "process.unobserved",
            };
            let time = event_time(event, false);
            (
                format!(
                    "{} {indent}{} {} (PID {}{}){}{}",
                    paint(&time, "90", color),
                    paint(&format!("{event_label:<18}"), "36", color),
                    paint(&label, process_role_color(&label, if depth == 0 { agent_type } else { None }), color),
                    paint(&pid, "90", color),
                    paint(&parent, "90", color),
                    paint(&trace, "90", color),
                    paint(suffix, if event_type == "process_unobserved" { "31" } else { "90" }, color),
                ),
                "",
            )
        }
        _ => (format!("{event_type}: {data}"), "90"),
    };
    let line = format!("[{sequence:05}] {line}");
    if color && !color_code.is_empty() {
        format!("\x1b[{color_code}m{line}\x1b[0m")
    } else {
        line
    }
}

fn render_watch_snapshot(traces: &[AgentTrace], session_id: &str, interval: Duration, color: bool) -> String {
    let mut output = String::new();
    let heading = format!("Watching {} agent{} · polling every {} ms · run {session_id}", traces.len(), if traces.len() == 1 { "" } else { "s" }, interval.as_millis());
    output.push_str(&paint(&heading, "36", color));
    output.push('\n');
    if traces.is_empty() {
        output.push_str("  No supported agent processes detected yet; watching for new processes.\n");
        return output;
    }

    for trace in traces {
        let label = trace.root.agent_type.expect("trace roots are detected agents").label();
        output.push_str(&format!("\n{}\n", paint(&format!("Agent: {label}"), "36", color)));
        output.push_str(&format!("PID: {}\n", trace.root.pid));
        output.push_str(&format!("Started: {}\n", format_local_time(trace.root.start_time.saturating_mul(1_000))));
        output.push_str("Status: Running\n");
        output.push_str(&format!("Working directory: {}\n", trace.root.working_directory.as_deref().unwrap_or("unavailable")));
        output.push_str(&paint("\nLive Processes\n", "36", color));
        output.push_str(&format!("{:<8} {:<30} {}\n", "PID", "Process", "Status"));
        output.push_str(&format!("{:<8} {:<30} {}\n", trace.root.pid, label, paint("running", "32", color)));
        let mut descendants: Vec<_> = trace.descendants.iter().collect();
        descendants.sort_by(|left, right| left.depth.cmp(&right.depth)
            .then_with(|| left.process.start_time.cmp(&right.process.start_time))
            .then_with(|| left.process.pid.cmp(&right.process.pid)));
        for descendant in descendants {
            let process_name = process_display_name(&descendant.process);
            let indentation = "  ".repeat(descendant.depth);
            let name = format!("{indentation}{process_name}");
            output.push_str(&format!(
                "{:<8} {:<30} {}\n",
                descendant.process.pid,
                paint(&name, process_role_color(&process_name, None), color),
                paint("running", "32", color),
            ));
        }
        output.push_str(&format!("Trace ID: {}\n", trace.trace_id));
    }
    output.push_str("\nLive activity (new process changes appear below)\n");
    output
}

fn watch_snapshot_json(traces: &[AgentTrace]) -> Value {
    json!({
        "type": "agenttrace.snapshot",
        "agents": traces.iter().map(|trace| {
            let label = trace.root.agent_type.expect("trace roots are detected agents").label();
            let mut processes = vec![json!({
                "pid": trace.root.pid,
                "process": label,
                "status": "running"
            })];
            let mut descendants: Vec<_> = trace.descendants.iter().collect();
            descendants.sort_by(|left, right| left.depth.cmp(&right.depth)
                .then_with(|| left.process.start_time.cmp(&right.process.start_time))
                .then_with(|| left.process.pid.cmp(&right.process.pid)));
            processes.extend(descendants.into_iter().map(|descendant| json!({
                "pid": descendant.process.pid,
                "process": process_display_name(&descendant.process),
                "status": "running"
            })));
            json!({
                "agent": label,
                "pid": trace.root.pid,
                "started_at": format_local_time(trace.root.start_time.saturating_mul(1_000)),
                "status": "running",
                "working_directory": trace.root.working_directory.as_deref().unwrap_or("unavailable"),
                "live_processes": processes
            })
        }).collect::<Vec<_>>()
    })
}

fn process_display_name(process: &ProcessRecord) -> String {
    process.command_args.as_ref()
        .map(|arguments| arguments.join(" "))
        .filter(|arguments| !arguments.is_empty())
        .unwrap_or_else(|| process.name.clone())
}

fn paint(text: &str, color_code: &str, color: bool) -> String {
    if color {
        format!("\x1b[{color_code}m{text}\x1b[0m")
    } else {
        text.to_string()
    }
}

fn local_timestamp() -> String {
    let parts = local_components(unix_time_millis()).unwrap_or_default();
    let offset_minutes = parts.offset_seconds / 60;
    let sign = if offset_minutes < 0 { '-' } else { '+' };
    let absolute_offset = offset_minutes.unsigned_abs();
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}{sign}{:02}:{:02}",
        parts.year,
        parts.month,
        parts.day,
        parts.hour,
        parts.minute,
        parts.second,
        absolute_offset / 60,
        absolute_offset % 60
    )
}

#[cfg(test)]
fn has_iso8601_offset(timestamp: &str) -> bool {
    let bytes = timestamp.as_bytes();
    bytes.len() == 25
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes[10] == b'T'
        && bytes[13] == b':'
        && bytes[16] == b':'
        && (bytes[19] == b'+' || bytes[19] == b'-')
        && bytes[22] == b':'
        && bytes.iter().enumerate().all(|(index, byte)| {
            matches!(index, 4 | 7 | 10 | 13 | 16 | 19 | 22) || byte.is_ascii_digit()
        })
}

fn format_local_time(timestamp_unix_ms: u64) -> String {
    local_components(timestamp_unix_ms)
        .map(|parts| format!("{:02}:{:02}:{:02}", parts.hour, parts.minute, parts.second))
        .unwrap_or_else(|| "--:--:--".to_string())
}

fn format_local_datetime(timestamp_unix_s: u64) -> String {
    local_components(timestamp_unix_s.saturating_mul(1_000))
        .map(|parts| format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}", parts.year, parts.month, parts.day, parts.hour, parts.minute, parts.second))
        .unwrap_or_else(|| "unknown".to_string())
}

fn event_time(event: &Value, include_date: bool) -> String {
    if let Some(timestamp) = event["timestamp"].as_str() {
        if let Some(local_timestamp) = timestamp.get(..19) {
            let separator = if include_date { ' ' } else { 'T' };
            let value = local_timestamp.replace('T', &separator.to_string());
            return if include_date { value } else { value.get(11..).unwrap_or("--:--:--").to_string() };
        }
    }
    event["timestamp_unix_ms"]
        .as_u64()
        .map(|timestamp| {
            if include_date {
                local_components(timestamp)
                    .map(|parts| format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}", parts.year, parts.month, parts.day, parts.hour, parts.minute, parts.second))
                    .unwrap_or_else(|| "unknown".to_string())
            } else {
                format_local_time(timestamp)
            }
        })
        .unwrap_or_else(|| "unknown".to_string())
}

#[derive(Default)]
struct LocalDateTime {
    year: i32,
    month: u32,
    day: u32,
    hour: u32,
    minute: u32,
    second: u32,
    offset_seconds: i32,
}

#[cfg(unix)]
fn local_components(timestamp_unix_ms: u64) -> Option<LocalDateTime> {
    let seconds = (timestamp_unix_ms / 1_000) as libc::time_t;
    let mut result: libc::tm = unsafe { std::mem::zeroed() };
    let converted = unsafe { libc::localtime_r(&seconds, &mut result) };
    if converted.is_null() {
        return None;
    }
    Some(LocalDateTime {
        year: result.tm_year + 1900,
        month: (result.tm_mon + 1) as u32,
        day: result.tm_mday as u32,
        hour: result.tm_hour as u32,
        minute: result.tm_min as u32,
        second: result.tm_sec as u32,
        offset_seconds: result.tm_gmtoff as i32,
    })
}

#[cfg(windows)]
fn local_components(timestamp_unix_ms: u64) -> Option<LocalDateTime> {
    let seconds = (timestamp_unix_ms / 1_000) as i64;
    let mut result: libc::tm = unsafe { std::mem::zeroed() };
    let converted = unsafe { _localtime64_s(&mut result, &seconds) };
    if converted != 0 {
        return None;
    }
    let mut timezone = 0 as libc::c_long;
    let mut dst_bias = 0 as libc::c_long;
    if unsafe { _get_timezone(&mut timezone) } != 0 || unsafe { _get_dstbias(&mut dst_bias) } != 0 {
        return None;
    }
    let offset = -timezone - if result.tm_isdst > 0 { dst_bias } else { 0 };
    Some(LocalDateTime {
        year: result.tm_year + 1900,
        month: (result.tm_mon + 1) as u32,
        day: result.tm_mday as u32,
        hour: result.tm_hour as u32,
        minute: result.tm_min as u32,
        second: result.tm_sec as u32,
        offset_seconds: offset as i32,
    })
}

#[cfg(windows)]
unsafe extern "C" {
    fn _localtime64_s(result: *mut libc::tm, time: *const i64) -> i32;
    fn _get_timezone(seconds_west: *mut libc::c_long) -> i32;
    fn _get_dstbias(seconds: *mut libc::c_long) -> i32;
}

#[cfg(not(any(unix, windows)))]
fn local_components(_: u64) -> Option<LocalDateTime> {
    None
}

fn traced_command(program: &str, arguments: &[String], include_arguments: bool) -> String {
    if include_arguments && !arguments.is_empty() {
        std::iter::once(program)
            .chain(arguments.iter().map(String::as_str))
            .collect::<Vec<_>>()
            .join(" ")
    } else {
        program.to_string()
    }
}

fn check_started_data(command: &str, pid: u32, started_at: String) -> Value {
    json!({
        "command": command,
        "requested_by": { "pid": std::process::id(), "process": "agenttrace" },
        "pid": pid,
        "started_at": started_at
    })
}

fn check_completed_data(
    command: &str,
    pid: u32,
    exit_code: Option<i32>,
    started_at: String,
    completed_at: String,
    duration_ms: u64,
) -> Value {
    json!({
        "command": command,
        "requested_by": { "pid": std::process::id(), "process": "agenttrace" },
        "pid": pid,
        "exit_code": exit_code,
        "started_at": started_at,
        "completed_at": completed_at,
        "duration_ms": duration_ms
    })
}

fn unix_time_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn unix_time_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
}

fn print_history(arguments: &[String]) -> Result<(), String> {
    let mut path = PathBuf::from(DEFAULT_HISTORY_PATH);
    let mut raw = false;
    let mut pretty_json = false;
    let mut index = 0;
    while index < arguments.len() {
        match arguments[index].as_str() {
            "--file" | "--input" => {
                index += 1;
                path = PathBuf::from(arguments.get(index).ok_or("--file requires a path")?);
            }
            "--json" | "--raw" => raw = true,
            "--pretty-json" => pretty_json = true,
            option => return Err(format!("unknown history option: {option}")),
        }
        index += 1;
    }
    let file = File::open(&path).map_err(|error| format!("cannot open history file {}: {error}", path.display()))?;
    let mut reader = BufReader::new(file);
    if raw && pretty_json {
        return Err("choose either --json or --pretty-json".to_string());
    }
    if raw {
        io::copy(&mut reader, &mut io::stdout())
            .map_err(|error| format!("cannot read history file {}: {error}", path.display()))?;
        return Ok(());
    }
    let mut contents = String::new();
    reader.read_to_string(&mut contents)
        .map_err(|error| format!("cannot read history file {}: {error}", path.display()))?;
    let stdout = io::stdout();
    let color = stdout.is_terminal() && env::var_os("NO_COLOR").is_none() && env::var("TERM").as_deref() != Ok("dumb");
    let mut output = stdout.lock();
    if pretty_json {
        for (line_number, line) in contents.lines().enumerate() {
            if line.trim().is_empty() { continue; }
            let event: Value = serde_json::from_str(line)
                .map_err(|error| format!("invalid JSONL at line {}: {error}", line_number + 1))?;
            serde_json::to_writer_pretty(&mut output, &event)
                .map_err(|error| format!("cannot format JSON event: {error}"))?;
            writeln!(output, "\n").map_err(|error| format!("cannot write JSON separator: {error}"))?;
        }
        return Ok(());
    }
    writeln!(output, "{}\n", paint("AgentTrace History", "36", color))
        .map_err(|error| format!("cannot write history header: {error}"))?;
    for (line_number, line) in contents.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let event: Value = serde_json::from_str(line)
            .map_err(|error| format!("invalid JSONL at line {}: {error}", line_number + 1))?;
        writeln!(output, "{}", render_history_event(&event, color))
            .map_err(|error| format!("cannot write history event: {error}"))?;
    }
    Ok(())
}

fn history_event_label(event: &Value) -> (String, String, String) {
    let event_type = event["event_type"].as_str().or(event["type"].as_str()).unwrap_or("event");
    let data = event_payload(event);
    match event_type {
        "process_started" | "process_discovered" => (
            "process.started".to_string(),
            data["executable"].as_str().or(data["process_name"].as_str()).unwrap_or("process").to_string(),
            "36".to_string(),
        ),
        "process_exited" => {
            let exit_code = data["exit_code"].as_i64();
            let color = if exit_code.is_some_and(|code| code != 0) { "31" } else { "32" };
            (
                "process.exited".to_string(),
                data["executable"].as_str().or(data["process_name"].as_str()).unwrap_or("process").to_string(),
                color.to_string(),
            )
        }
        "process_updated" => ("process.updated".to_string(), data["process_name"].as_str().unwrap_or("process").to_string(), "36".to_string()),
        "process_unobserved" => ("process.unobserved".to_string(), data["process_name"].as_str().unwrap_or("process").to_string(), "36".to_string()),
        "check.started" => ("check.started".to_string(), data["command"].as_str().unwrap_or("command").to_string(), "33".to_string()),
        "check.completed" => {
            let code = data["exit_code"].as_i64();
            ("check.completed".to_string(), data["command"].as_str().unwrap_or("command").to_string(), if code == Some(0) { "32" } else { "31" }.to_string())
        }
        "stream_activity" => {
            let stream = data["stream"].as_str().unwrap_or("unknown");
            let bytes = data["byte_count"].as_u64().unwrap_or_default();
            ("stream.activity".to_string(), format!("{stream} · {bytes} bytes"), "90".to_string())
        }
        "file_changed" => {
            let action = data["action"].as_str().unwrap_or("changed");
            (format!("file.{action}"), data["path"].as_str().unwrap_or("file").to_string(), "35".to_string())
        }
        "collector_status" | "session_failed" => (event_type.replace('_', "."), data["reason"].as_str().or(data["status"].as_str()).unwrap_or("status").to_string(), "31".to_string()),
        "session_started" => ("session.started".to_string(), data["collector"].as_str().unwrap_or("session").to_string(), "36".to_string()),
        "session_stopped" => ("session.stopped".to_string(), data["reason"].as_str().unwrap_or("complete").to_string(), "90".to_string()),
        _ => (event_type.replace('_', "."), data.to_string(), "90".to_string()),
    }
}

fn render_history_event(event: &Value, color: bool) -> String {
    let (label, subject, color_code) = history_event_label(event);
    let data = event_payload(event);
    let pid = data["pid"].as_u64().map(|pid| pid.to_string())
        .or_else(|| data["process_identity"]["pid"].as_str().map(str::to_string))
        .or_else(|| data["process_identity"]["pid"].as_u64().map(|pid| pid.to_string()));
    let mut detail = pid.map(|pid| format!("PID {pid}")).unwrap_or_default();
    if let Some(exit_code) = data["exit_code"].as_i64() {
        if !detail.is_empty() { detail.push_str("  "); }
        detail.push_str(&format!("exit {exit_code}"));
    }
    if let Some(duration) = data["duration_ms"].as_u64() {
        if !detail.is_empty() { detail.push_str("  "); }
        detail.push_str(&format!("{} ms", duration));
    }
    let subject_color = match label.as_str() {
        "process.started" | "process.exited" | "process.updated" | "process.unobserved" => {
            process_role_color(subject.as_str(), data["agent_type"].as_str())
        }
        _ => color_code.as_str(),
    };
    let time = paint(&event_time(event, true), "90", color);
    let event_name = paint(&format!("{label:<18}"), &color_code, color);
    let subject = paint(&subject, subject_color, color);
    format!("{time}  {event_name} {subject}{}",
        if detail.is_empty() { String::new() } else { format!("  {}", paint(&detail, "90", color)) })
}

fn event_payload(event: &Value) -> &Value {
    event.get("data").filter(|data| !data.is_null()).unwrap_or(event)
}

fn process_role_color(process: &str, agent_type: Option<&str>) -> &'static str {
    if agent_type.is_some() {
        return "36";
    }
    let process = process.split_whitespace().next().unwrap_or(process).trim_matches(['"', '\'']);
    let executable = Path::new(process)
        .file_name()
        .unwrap_or_else(|| OsStr::new(process))
        .to_string_lossy()
        .trim_end_matches(".exe")
        .to_ascii_lowercase();
    match executable.as_str() {
        "cmd" | "powershell" | "pwsh" | "sh" | "bash" | "zsh" | "npm" | "npx" | "cargo" | "git" => "33",
        "node" | "python" | "python3" | "ruby" | "java" | "deno" | "bun" => "34",
        _ => "90",
    }
}

fn build_traces(processes: &[ProcessRecord]) -> Vec<AgentTrace> {
    let process_by_pid: HashMap<&str, &ProcessRecord> = processes
        .iter()
        .map(|process| (process.pid.as_str(), process))
        .collect();
    let mut traces: Vec<AgentTrace> = processes
        .iter()
        .filter(|process| process.agent_type.is_some())
        .map(|root| AgentTrace {
            trace_id: format!("trace-{}", instance_key(&root.pid, root.start_time)),
            root: root.clone(),
            descendants: Vec::new(),
        })
        .collect();
    traces.sort_by(|left, right| {
        left.root.agent_type.unwrap().label()
            .cmp(right.root.agent_type.unwrap().label())
            .then_with(|| left.root.start_time.cmp(&right.root.start_time))
            .then_with(|| left.root.pid.cmp(&right.root.pid))
    });
    let trace_by_id: HashMap<String, usize> = traces
        .iter()
        .enumerate()
        .map(|(index, trace)| (trace.trace_id.clone(), index))
        .collect();

    for process in processes.iter().filter(|process| process.agent_type.is_none()) {
        let mut ancestor_pid = process.parent_pid.as_deref();
        let mut depth = 1;
        let mut visited = HashSet::new();
        while let Some(pid) = ancestor_pid {
            if !visited.insert(pid) {
                break;
            }
            let Some(ancestor) = process_by_pid.get(pid) else {
                break;
            };
            if ancestor.agent_type.is_some() {
                let trace_id = format!("trace-{}", instance_key(&ancestor.pid, ancestor.start_time));
                if let Some(index) = trace_by_id.get(&trace_id) {
                    traces[*index].descendants.push(TraceProcess {
                        process: process.clone(),
                        depth,
                    });
                }
                break;
            }
            depth += 1;
            ancestor_pid = ancestor.parent_pid.as_deref();
        }
    }

    for trace in &mut traces {
        trace.descendants.sort_by(|left, right| {
            left.depth
                .cmp(&right.depth)
                .then_with(|| left.process.start_time.cmp(&right.process.start_time))
                .then_with(|| left.process.pid.cmp(&right.process.pid))
        });
    }
    traces
}

fn identify_agent(process: &Process) -> Option<AgentType> {
    let mut executables = vec![process.name().to_string_lossy().to_ascii_lowercase()];
    if let Some(executable) = process.exe() {
        executables.push(executable.to_string_lossy().to_ascii_lowercase());
    }
    let command_markers: Vec<String> = process
        .cmd()
        .iter()
        .map(|argument| argument.to_string_lossy().to_ascii_lowercase())
        .collect();
    identify_markers(&executables, &command_markers)
}

fn identify_markers(executables: &[String], command_markers: &[String]) -> Option<AgentType> {
    let has_marker = |marker: &str| command_markers.iter().any(|value| value.contains(marker));
    let has_executable = |name: &str| {
        executables.iter().any(|value| {
            let path = Path::new(value);
            path.file_name()
                .unwrap_or_else(|| OsStr::new(value))
                .to_string_lossy()
                .trim_end_matches(".exe")
                .eq_ignore_ascii_case(name)
        })
    };

    if has_marker("@anthropic-ai/claude-code") || has_marker("claude-code") || has_executable("claude") {
        Some(AgentType::ClaudeCode)
    } else if has_marker("@openai/codex") || has_marker("codex-cli") || has_executable("codex") {
        Some(AgentType::Codex)
    } else if has_marker("@google/gemini-cli") || has_marker("gemini-cli") || has_executable("gemini") {
        Some(AgentType::GeminiCli)
    } else {
        None
    }
}

fn instance_key(pid: &str, start_time: u64) -> String {
    format!("{pid}:{start_time}")
}

fn print_help() {
    println!("TRACE USAGE: agenttrace trace [--check] [--output PATH] [--cwd PATH] [--include-command-args] [--capture-streams] -- PROGRAM [ARGS...]");
    println!("TRACE OPTIONS: --check (record command lifecycle); --output PATH; --cwd PATH; --include-command-args (sensitive); --capture-streams (up to 10 MiB per stream, sensitive); --watch-files (workspace metadata changes)\n");
    println!("WATCH ALIAS: `agenttrace run --watch [WATCH OPTIONS]` is equivalent to `agenttrace watch [WATCH OPTIONS]`.\n");
    println!("WATCH OUTPUT: defaults to readable events; use `--json` for compact JSONL or `--pretty-json` for indented JSON records. Color is disabled in JSON modes and by `NO_COLOR`.\n");
    println!("HISTORY OUTPUT: `agenttrace history` shows a colored timeline; use `--json` for original JSONL or `--pretty-json` for indented JSON records.\n");
    println!("AgentTrace — discover running AI coding-agent processes\n");
    println!("USAGE:\n    agenttrace [run]\n    agenttrace watch [--output PATH] [--interval-ms MS] [--duration-ms MS] [--include-command-args]\n    agenttrace history [--file PATH]\n\nCOMMANDS:\n    run       Take a one-time process snapshot (default)\n    watch     Stream process lifecycle events and append JSONL history\n    history   Print recorded JSONL history\n\nWATCH OPTIONS:\n    --output PATH            History file (default: .agenttrace/history.jsonl)\n    --interval-ms MS         Poll interval, at least 100 ms (default: 1000)\n    --duration-ms MS         Stop after a bounded recording session\n    --include-command-args   Persist raw command arguments; may expose secrets\n\nOPTIONS:\n    -h, --help    Print this help message");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};
    use std::thread;
    use std::time::Duration;

    fn markers(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_ascii_lowercase()).collect()
    }

    #[test]
    fn recognizes_supported_executables_and_package_markers() {
        assert_eq!(identify_markers(&markers(&["claude.exe"]), &[]), Some(AgentType::ClaudeCode));
        assert_eq!(identify_markers(&markers(&["node"]), &markers(&["@openai/codex/bin/codex.js"])), Some(AgentType::Codex));
        assert_eq!(identify_markers(&markers(&["node"]), &markers(&["@google/gemini-cli/dist/index.js"])), Some(AgentType::GeminiCli));
    }

    #[test]
    fn leaves_unrecognized_processes_unclassified() {
        assert_eq!(identify_markers(&markers(&["node"]), &markers(&["server.js"])), None);
        assert_eq!(identify_markers(&markers(&["git"]), &markers(&["status"])), None);
    }

    #[test]
    fn separate_instances_have_distinct_process_identity() {
        assert_ne!(instance_key("412", 1_700_000_000), instance_key("993", 1_700_000_000));
        assert_ne!(instance_key("412", 1_700_000_000), instance_key("412", 1_700_000_100));
    }

    #[test]
    fn same_agent_processes_in_one_workspace_get_separate_traces() {
        let processes = vec![
            process_record("100", None, Some(AgentType::ClaudeCode), 10, Some("C:/repo")),
            process_record("101", Some("100"), None, 11, Some("C:/repo")),
            process_record("200", None, Some(AgentType::ClaudeCode), 20, Some("C:/repo")),
            process_record("201", Some("200"), None, 21, Some("C:/repo")),
        ];

        let traces = build_traces(&processes);

        assert_eq!(traces.len(), 2);
        assert_ne!(traces[0].trace_id, traces[1].trace_id);
        assert_eq!(traces[0].root.working_directory, traces[1].root.working_directory);
        assert_eq!(traces[0].descendants.len(), 1);
        assert_eq!(traces[0].descendants[0].process.pid, "101");
        assert_eq!(traces[1].descendants.len(), 1);
        assert_eq!(traces[1].descendants[0].process.pid, "201");
    }

    #[test]
    fn descendants_belong_to_the_nearest_detected_agent() {
        let processes = vec![
            process_record("100", None, Some(AgentType::ClaudeCode), 10, None),
            process_record("110", Some("100"), Some(AgentType::Codex), 11, None),
            process_record("111", Some("110"), None, 12, None),
        ];

        let traces = build_traces(&processes);
        let claude = traces.iter().find(|trace| trace.root.pid == "100").unwrap();
        let codex = traces.iter().find(|trace| trace.root.pid == "110").unwrap();

        assert!(claude.descendants.is_empty());
        assert_eq!(codex.descendants.len(), 1);
        assert_eq!(codex.descendants[0].process.pid, "111");
        assert_eq!(codex.descendants[0].depth, 1);
    }

    #[test]
    fn attributes_a_real_child_process_to_its_parent_trace() {
        if env::var_os("AGENTTRACE_PROCESS_FIXTURE").is_some() {
            thread::sleep(Duration::from_secs(15));
            return;
        }

        let executable = env::current_exe().unwrap();
        let mut child = Command::new(executable)
            .args(["process_child_fixture", "--nocapture"])
            .env("AGENTTRACE_PROCESS_FIXTURE", "1")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let child_pid = child.id().to_string();
        let root_pid = std::process::id().to_string();
        thread::sleep(Duration::from_millis(200));
        let mut system = System::new_all();
        system.refresh_all();
        let child_record = snapshot_processes(&system, false)
            .into_iter()
            .find(|process| process.pid == child_pid);
        let _ = child.kill();
        let _ = child.wait();

        let child_record = child_record.expect("spawned test process should appear in the process snapshot");
        assert_eq!(child_record.parent_pid.as_deref(), Some(root_pid.as_str()));
        let root_record = process_record(&root_pid, None, Some(AgentType::ClaudeCode), 1, None);
        let traces = build_traces(&[root_record, child_record]);

        assert_eq!(traces.len(), 1);
        assert_eq!(traces[0].descendants.len(), 1);
        assert_eq!(traces[0].descendants[0].process.pid, child_pid);
    }

    #[test]
    fn process_child_fixture() {
        if env::var_os("AGENTTRACE_PROCESS_FIXTURE").is_some() {
            thread::sleep(Duration::from_secs(15));
        }
    }

    #[test]
    fn watch_options_require_a_safe_poll_interval() {
        let too_fast = vec!["--interval-ms".to_string(), "50".to_string()];
        assert!(matches!(parse_watch_options(&too_fast), Err(message) if message.contains("at least")));

        let options = parse_watch_options(&[]).unwrap();
        assert_eq!(options.interval, Duration::from_millis(DEFAULT_INTERVAL_MS));
        assert_eq!(options.output, PathBuf::from(DEFAULT_HISTORY_PATH));
        assert!(options.duration.is_none());
        assert!(!options.include_command_args);
        assert_eq!(options.format, WatchOutputFormat::Pretty);

        let bounded = vec!["--duration-ms".to_string(), "500".to_string()];
        assert_eq!(parse_watch_options(&bounded).unwrap().duration, Some(Duration::from_millis(500)));

        let json_format = vec!["--format".to_string(), "json".to_string()];
        assert_eq!(parse_watch_options(&json_format).unwrap().format, WatchOutputFormat::Json);
        let json_flag = vec!["--json".to_string()];
        assert_eq!(parse_watch_options(&json_flag).unwrap().format, WatchOutputFormat::Json);
        let pretty_json = vec!["--pretty-json".to_string()];
        assert_eq!(parse_watch_options(&pretty_json).unwrap().format, WatchOutputFormat::PrettyJson);
        let invalid_format = vec!["--format".to_string(), "xml".to_string()];
        assert!(parse_watch_options(&invalid_format).is_err());
    }

    #[test]
    fn timestamps_keep_iso8601_offsets_and_render_as_local_clock_time() {
        let timestamp = local_timestamp();
        assert!(has_iso8601_offset(&timestamp));
        assert!(timestamp.as_bytes().get(19).is_some_and(|byte| *byte == b'+' || *byte == b'-'));
        assert_eq!(format_local_time(1_000), local_components(1_000).map(|parts| format!("{:02}:{:02}:{:02}", parts.hour, parts.minute, parts.second)).unwrap());
    }

    #[test]
    fn check_lifecycle_records_success_and_failure_details() {
        for exit_code in [Some(0), Some(7)] {
            let started = "2026-10-04T20:51:12+01:00".to_string();
            let completed = "2026-10-04T20:51:18+01:00".to_string();
            let start = check_started_data("npm test", 8142, started.clone());
            let finish = check_completed_data("npm test", 8142, exit_code, started.clone(), completed.clone(), 6_000);
            assert_eq!(start["command"], "npm test");
            assert_eq!(start["requested_by"]["process"], "agenttrace");
            assert_eq!(start["pid"], 8142);
            assert_eq!(start["started_at"], started);
            assert_eq!(finish["exit_code"], json!(exit_code));
            assert_eq!(finish["completed_at"], completed);
            assert_eq!(finish["duration_ms"], 6_000);
            assert_eq!(history_event_label(&json!({"event_type":"check.completed", "data":finish})).2, if exit_code == Some(0) { "32" } else { "31" });
        }
    }

    #[test]
    fn supervised_check_writes_started_and_completed_events_for_zero_and_nonzero_exit() {
        for exit_code in [0, 7] {
            let history_path = env::temp_dir().join(format!("agenttrace-check-{}-{}-{exit_code}.jsonl", std::process::id(), unix_time_nanos()));
            let mut arguments = vec![
                "--check".to_string(),
                "--output".to_string(),
                history_path.display().to_string(),
                "--".to_string(),
            ];
            #[cfg(windows)]
            arguments.extend(["cmd.exe".to_string(), "/C".to_string(), format!("exit {exit_code}")]);
            #[cfg(unix)]
            arguments.extend(["sh".to_string(), "-c".to_string(), format!("exit {exit_code}")]);

            assert_eq!(trace_command(&arguments).unwrap(), exit_code);
            let events: Vec<Value> = fs::read_to_string(&history_path)
                .unwrap()
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect();
            let started = events.iter().find(|event| event["type"] == "check.started").unwrap();
            let completed = events.iter().find(|event| event["type"] == "check.completed").unwrap();
            assert_eq!(started["pid"], completed["pid"]);
            assert_eq!(completed["exit_code"], exit_code);
            assert!(has_iso8601_offset(started["started_at"].as_str().unwrap()));
            assert!(has_iso8601_offset(completed["started_at"].as_str().unwrap()));
            assert!(has_iso8601_offset(completed["completed_at"].as_str().unwrap()));
            assert!(completed["duration_ms"].is_number());
            assert!(completed.get("data").is_none());
            assert!(completed["command"].as_str().is_some_and(|command| !command.is_empty()));
            assert!(events.iter().all(|event| event["timestamp"].is_string()));
            fs::remove_file(history_path).unwrap();
        }
    }

    #[test]
    fn history_renderer_formats_legacy_and_current_jsonl_events_without_ansi_by_default() {
        let old_event = json!({
            "timestamp_unix_ms": 1_000,
            "event_type": "file_changed",
            "data": { "path": "src/auth.js", "action": "modified" }
        });
        let old_line = render_history_event(&old_event, false);
        assert!(old_line.contains("file.modified"));
        assert!(old_line.contains("src/auth.js"));
        assert!(!old_line.contains("\x1b["));

        let check = json!({
            "timestamp": "2026-10-04T20:51:18+01:00",
            "event_type": "check.completed",
            "data": { "command": "npm test", "pid": 8142, "exit_code": 0, "duration_ms": 6000 }
        });
        let line = render_history_event(&check, false);
        assert!(line.starts_with("2026-10-04 20:51:18"));
        assert!(line.contains("check.completed"));
        assert!(line.contains("npm test"));
        assert!(line.contains("PID 8142"));
        assert!(line.contains("exit 0"));
        assert!(!line.contains("\x1b["));
        let colored = render_history_event(&check, true);
        assert!(colored.contains("\x1b[32mcheck.completed"));
        assert!(colored.contains("\x1b[32mnpm test"));
    }

    #[test]
    fn history_colors_command_and_runtime_process_subjects_differently() {
        let command = json!({
            "timestamp_unix_ms": 1_000,
            "event_type": "process_discovered",
            "data": { "process_name": "cmd.exe", "process_identity": { "pid": "1" } }
        });
        let runtime = json!({
            "timestamp_unix_ms": 1_000,
            "event_type": "process_discovered",
            "data": { "process_name": "node.exe", "process_identity": { "pid": "2" } }
        });
        assert!(render_history_event(&command, true).contains("\x1b[33mcmd.exe"));
        assert!(render_history_event(&runtime, true).contains("\x1b[34mnode.exe"));
        assert_ne!(process_role_color("cmd.exe", None), process_role_color("node.exe", None));
    }

    #[test]
    fn pretty_json_watch_output_indents_each_record_without_changing_jsonl_history() {
        let mut history = Vec::new();
        let mut output = Vec::new();
        let mut sequence = 0;
        write_event(
            &mut history,
            &mut output,
            "session-test",
            &mut sequence,
            "process_discovered",
            json!({ "process_name": "node.exe", "process_identity": { "pid": "42" } }),
            WatchOutputFormat::PrettyJson,
            true,
            true,
        ).unwrap();
        let rendered = String::from_utf8(output).unwrap();
        assert!(rendered.contains("\n  \"event_type\""));
        assert!(!rendered.contains("\x1b["));
        assert_eq!(rendered.lines().filter(|line| line.trim() == "{").count(), 1);
        let history_line = String::from_utf8(history).unwrap();
        assert_eq!(history_line.lines().count(), 1);
        serde_json::from_str::<Value>(history_line.trim()).unwrap();
    }

    #[test]
    fn pretty_watch_output_indents_processes_and_colors_only_when_requested() {
        let event = json!({
            "sequence": 7,
            "timestamp_unix_ms": 1_000,
            "event_type": "process_discovered",
            "data": {
                "depth_from_agent": 2,
                "agent_type": "Codex",
                "process_name": "node",
                "process_identity": { "pid": "42" }
            }
        });
        let plain = render_watch_event(&event, false);
        assert_eq!(plain, format!("[00007] {}     process.started    node (PID 42) · Codex", format_local_time(1_000)));
        assert!(!plain.contains("\x1b["));
        assert!(render_watch_event(&event, true).contains("\x1b[36m"));
    }

    #[test]
    fn watch_snapshot_shows_agent_metadata_and_child_processes() {
        let root = process_record("100", Some("50"), Some(AgentType::Codex), 1_700_000_000, Some("C:/repo"));
        let child = process_record("101", Some("100"), None, 1_700_000_010, None);
        let traces = build_traces(&[root, child]);

        let output = render_watch_snapshot(&traces, "watch-run-1", Duration::from_secs(1), false);

        assert!(output.contains("Watching 1 agent · polling every 1000 ms · run watch-run-1"));
        assert!(output.contains("Trace ID: trace-100:1700000000"));
        assert!(output.contains("Working directory: C:/repo"));
        assert!(output.contains("Live Processes"));
        assert!(output.contains("PID      Process                        Status"));
        assert!(output.contains("running"));
        assert!(output.contains("101"));
        assert!(output.contains("Live activity"));
    }

    #[test]
    fn pretty_json_watch_snapshot_contains_agent_metadata_and_live_processes() {
        let root = process_record("100", Some("50"), Some(AgentType::Codex), 1_700_000_000, Some("C:/repo"));
        let child = process_record("101", Some("100"), None, 1_700_000_010, None);
        let snapshot = watch_snapshot_json(&build_traces(&[root, child]));

        assert_eq!(snapshot["type"], "agenttrace.snapshot");
        assert_eq!(snapshot["agents"][0]["agent"], "Codex");
        assert_eq!(snapshot["agents"][0]["pid"], "100");
        assert_eq!(snapshot["agents"][0]["status"], "running");
        assert_eq!(snapshot["agents"][0]["working_directory"], "C:/repo");
        assert_eq!(snapshot["agents"][0]["live_processes"][0]["pid"], "100");
        assert_eq!(snapshot["agents"][0]["live_processes"][1]["pid"], "101");
        assert_eq!(snapshot["agents"][0]["live_processes"][1]["status"], "running");
    }

    #[test]
    fn watch_snapshot_preserves_nested_child_process_relationships() {
        let processes = vec![
            process_record("100", None, Some(AgentType::Codex), 10, None),
            process_record("101", Some("100"), None, 11, None),
            process_record("102", Some("100"), None, 12, None),
            process_record("103", Some("101"), None, 13, None),
        ];
        let traces = build_traces(&processes);
        let output = render_watch_snapshot(&traces, "watch-run-1", Duration::from_secs(1), false);

        assert!(output.contains("101"));
        assert!(output.contains("103"));
        assert!(output.contains("102"));
    }

    #[test]
    fn watch_event_time_uses_local_clock_format() {
        let parts = local_components(1_000).unwrap();
        let expected = format!("{:02}:{:02}:{:02}", parts.hour, parts.minute, parts.second);
        assert_eq!(format_local_time(1_000), expected);
    }

    #[test]
    fn run_watch_alias_forwards_watch_options_and_rejects_unknown_flags() {
        let arguments = vec![
            "--watch".to_string(),
            "--duration-ms".to_string(),
            "2000".to_string(),
        ];
        let forwarded = run_watch_options(&arguments).unwrap().unwrap();
        assert_eq!(forwarded, &["--duration-ms", "2000"]);
        assert_eq!(parse_watch_options(forwarded).unwrap().duration, Some(Duration::from_secs(2)));
        assert!(run_watch_options(&[]).unwrap().is_none());
        assert!(run_watch_options(&["--unknown".to_string()]).is_err());
    }

    #[test]
    fn trace_options_separate_agenttrace_flags_from_target_arguments() {
        let arguments = vec![
            "--check".to_string(),
            "--capture-streams".to_string(),
            "--watch-files".to_string(),
            "--output".to_string(),
            "events.jsonl".to_string(),
            "--".to_string(),
            "sample".to_string(),
            "--help".to_string(),
        ];
        let options = parse_trace_options(&arguments).unwrap();
        assert!(options.capture_streams);
        assert!(options.watch_files);
        assert!(options.check);
        assert_eq!(options.output, PathBuf::from("events.jsonl"));
        assert_eq!(options.program, "sample");
        assert_eq!(options.arguments, vec!["--help"]);
    }

    #[test]
    fn stream_bytes_are_encoded_as_hex_without_loss() {
        assert_eq!(hex_encode(&[0, 15, 16, 255]), "000f10ff");
        assert!(parse_trace_options(&["--".to_string()]).is_err());
    }

    #[test]
    fn supervised_descendants_keep_parent_depth_and_trace_identity() {
        let processes = vec![
            process_record("10", None, None, 1, None),
            process_record("11", Some("10"), None, 2, None),
            process_record("12", Some("11"), None, 3, None),
            process_record("90", None, None, 4, None),
        ];
        let descendants = supervised_descendants(&processes, "10", "trace-test");
        assert_eq!(descendants.len(), 2);
        assert_eq!(descendants[&instance_key("11", 2)].depth, 1);
        assert_eq!(descendants[&instance_key("12", 3)].depth, 2);
        assert!(descendants.values().all(|process| process.trace_id == "trace-test"));
    }

    #[test]
    fn workspace_diff_records_created_modified_and_deleted_paths() {
        let root = env::temp_dir().join(format!("agenttrace-files-{}-{}", std::process::id(), unix_time_nanos()));
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("modified.txt"), "before").unwrap();
        fs::write(root.join("deleted.txt"), "gone").unwrap();
        let previous = scan_workspace_files(&root);
        fs::write(root.join("modified.txt"), "after with more bytes").unwrap();
        fs::write(root.join("created.txt"), "new").unwrap();
        fs::remove_file(root.join("deleted.txt")).unwrap();
        let current = scan_workspace_files(&root);

        let history_path = env::temp_dir().join(format!("agenttrace-events-{}-{}.jsonl", std::process::id(), unix_time_nanos()));
        let file = File::create(&history_path).unwrap();
        let logger = Arc::new(Mutex::new(TraceEventLog {
            writer: BufWriter::new(file),
            session_id: "test-session".to_string(),
            sequence: 0,
        }));
        emit_file_changes(&logger, "trace-test", &previous, &current).unwrap();
        drop(logger);
        let events: Vec<Value> = fs::read_to_string(&history_path)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        let actions: HashMap<String, String> = events
            .iter()
            .map(|event| (
                event["data"]["path"].as_str().unwrap().to_string(),
                event["data"]["action"].as_str().unwrap().to_string(),
            ))
            .collect();
        assert_eq!(actions["created.txt"], "created");
        assert_eq!(actions["modified.txt"], "modified");
        assert_eq!(actions["deleted.txt"], "deleted");
        assert!(events.iter().all(|event| event["cycle_id"] == "test-session"));
        fs::remove_file(history_path).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn process_change_events_are_jsonl_and_report_unknown_exit_codes() {
        let process = ObservedProcess {
            trace_id: "trace-100:10".to_string(),
            depth: 0,
            process: process_record("100", None, Some(AgentType::Codex), 10, None),
        };
        let identity = instance_key(&process.process.pid, process.process.start_time);
        let current = HashMap::from([(identity, process)]);
        let mut updated_process = current.values().next().unwrap().clone();
        updated_process.depth = 1;
        let updated = HashMap::from([(
            instance_key(&updated_process.process.pid, updated_process.process.start_time),
            updated_process,
        )]);
        let mut history = Vec::new();
        let mut stream = Vec::new();
        let mut sequence = 0;

        write_process_changes(
            &mut history,
            &mut stream,
            "session-test",
            &mut sequence,
            &HashMap::new(),
            &current,
            WatchOutputFormat::Json,
            false,
            false,
        )
        .unwrap();
        write_process_changes(
            &mut history,
            &mut stream,
            "session-test",
            &mut sequence,
            &current,
            &updated,
            WatchOutputFormat::Json,
            false,
            false,
        )
        .unwrap();
        write_process_changes(
            &mut history,
            &mut stream,
            "session-test",
            &mut sequence,
            &updated,
            &HashMap::new(),
            WatchOutputFormat::Json,
            false,
            false,
        )
        .unwrap();

        let events: Vec<Value> = history
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
            .map(|line| serde_json::from_slice(line).unwrap())
            .collect();
        assert_eq!(events.len(), 3);
        assert_eq!(events[0]["event_type"], "process_discovered");
        assert_eq!(events[0]["collector"], "sysinfo_polling");
        assert!(events[0].get("observed_at_unix_ms").is_some());
        assert_eq!(events[0]["data"]["process_identity"]["pid"], "100");
        assert!(has_iso8601_offset(events[0]["timestamp"].as_str().unwrap()));
        assert!(!String::from_utf8(stream).unwrap().contains("\x1b["));
        assert!(events[0]["data"].get("command_args").is_none());
        assert_eq!(events[1]["event_type"], "process_updated");
        assert_eq!(events[2]["event_type"], "process_unobserved");
        assert_eq!(events[2]["data"]["exit_code"], Value::Null);
        assert_eq!(events[2]["data"]["exit_code_available"], false);
        assert_eq!(sequence, 3);
    }

    #[test]
    fn tracked_processes_stay_attributed_while_still_running() {
        let process = process_record("200", Some("100"), None, 20, None);
        let identity = instance_key(&process.pid, process.start_time);
        let previous = HashMap::from([(
            identity.clone(),
            ObservedProcess {
                trace_id: "trace-100:10".to_string(),
                depth: 1,
                process: process.clone(),
            },
        )]);
        let mut current = HashMap::new();

        retain_live_attributions(&mut current, &previous, &[process]);

        assert_eq!(current[&identity].trace_id, "trace-100:10");
        assert!(current.contains_key(&identity));
    }

    fn process_record(
        pid: &str,
        parent_pid: Option<&str>,
        agent_type: Option<AgentType>,
        start_time: u64,
        working_directory: Option<&str>,
    ) -> ProcessRecord {
        ProcessRecord {
            agent_type,
            pid: pid.to_string(),
            parent_pid: parent_pid.map(str::to_string),
            name: "test-process".to_string(),
            working_directory: working_directory.map(str::to_string),
            executable: None,
            start_time,
            parent_start_time: None,
            command_args: None,
        }
    }
}
