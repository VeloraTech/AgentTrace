use std::env;
use std::ffi::OsStr;
use std::path::Path;
use std::collections::{HashMap, HashSet};

use sysinfo::{Process, System};

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

#[derive(Clone)]
struct ProcessRecord {
    agent_type: Option<AgentType>,
    pid: String,
    parent_pid: Option<String>,
    name: String,
    working_directory: Option<String>,
    executable: Option<String>,
    start_time: u64,
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
    if arguments.iter().any(|arg| arg == "--help" || arg == "-h") {
        print_help();
        return;
    }
    if !arguments.is_empty() && arguments[0] != "run" {
        eprintln!("Unknown command: {}", arguments[0]);
        print_help();
        std::process::exit(2);
    }
    discover_agents();
}

fn discover_agents() {
    let mut system = System::new_all();
    system.refresh_all();
    let processes = snapshot_processes(&system);
    let traces = build_traces(&processes);
    print_traces(&traces);
}

fn snapshot_processes(system: &System) -> Vec<ProcessRecord> {
    system
        .processes()
        .iter()
        .map(|(pid, process)| ProcessRecord {
            agent_type: identify_agent(process),
            pid: pid.to_string(),
            parent_pid: process.parent().map(|parent| parent.to_string()),
            name: process.name().to_string_lossy().into_owned(),
            working_directory: process.cwd().map(|path| path.display().to_string()),
            executable: process
                .exe()
                .and_then(Path::file_name)
                .map(|name| name.to_string_lossy().into_owned()),
            start_time: process.start_time(),
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
        println!("   Started (Unix time): {}", trace.root.start_time);
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
    println!("AgentTrace — discover running AI coding-agent processes\n");
    println!("USAGE:\n    agenttrace [run]\n\nOPTIONS:\n    -h, --help    Print this help message");
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
        let child_record = snapshot_processes(&system)
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
        }
    }
}
