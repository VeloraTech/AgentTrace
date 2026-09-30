use std::env;
use std::ffi::OsStr;
use std::path::Path;

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

struct AgentProcess {
    agent_type: AgentType,
    pid: String,
    parent_pid: Option<String>,
    working_directory: Option<String>,
    executable: Option<String>,
    start_time: u64,
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

    let mut agents: Vec<AgentProcess> = system
        .processes()
        .iter()
        .filter_map(|(pid, process)| {
            Some(AgentProcess {
                agent_type: identify_agent(process)?,
                pid: pid.to_string(),
                parent_pid: process.parent().map(|parent| parent.to_string()),
                working_directory: process.cwd().map(|path| path.display().to_string()),
                executable: process
                    .exe()
                    .and_then(Path::file_name)
                    .map(|name| name.to_string_lossy().into_owned()),
                start_time: process.start_time(),
            })
        })
        .collect();

    agents.sort_by(|left, right| {
        left.agent_type
            .label()
            .cmp(right.agent_type.label())
            .then_with(|| left.start_time.cmp(&right.start_time))
            .then_with(|| left.pid.cmp(&right.pid))
    });

    println!("AgentTrace\n");
    println!("{} candidate agent process{} detected", agents.len(), if agents.len() == 1 { "" } else { "es" });
    if agents.is_empty() {
        println!("\nNo supported agent processes found.");
        println!("Supported detectors: Claude Code, Codex, Gemini CLI.");
        return;
    }

    for (index, agent) in agents.iter().enumerate() {
        println!("\n{}. {}", index + 1, agent.agent_type.label());
        println!("   PID: {}", agent.pid);
        println!("   Parent PID: {}", agent.parent_pid.as_deref().unwrap_or("unavailable"));
        println!("   Working directory: {}", agent.working_directory.as_deref().unwrap_or("unavailable"));
        println!("   Executable: {}", agent.executable.as_deref().unwrap_or("unavailable"));
        println!("   Started (Unix time): {}", agent.start_time);
        println!("   Instance key: {}", instance_key(&agent.pid, agent.start_time));
    }
    println!("\nDetection uses process names, executable paths, and command markers.");
    println!("This is discovery only; no file, command, or network activity is recorded.");
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
}
