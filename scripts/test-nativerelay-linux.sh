#!/usr/bin/env bash
set -euo pipefail

agenttrace_bin="${GITHUB_WORKSPACE:-$PWD}/target/debug/agenttrace"
test_dir="$(mktemp -d)"
history_file="$test_dir/history.jsonl"
agenttrace_stdout="$test_dir/agenttrace.stdout"
agenttrace_stderr="$test_dir/agenttrace.stderr"
activity_file="$PWD/.nativerelay-integration-activity-$$.txt"
agenttrace_pid=""
agenttrace_process_pid=""

show_logs() {
  for log in "$agenttrace_stdout" "$agenttrace_stderr"; do
    if [[ -f "$log" ]]; then
      echo "--- $log ---" >&2
      cat "$log" >&2
    fi
  done
  if [[ -f "$history_file" ]]; then
    echo "--- AgentTrace history ---" >&2
    cat "$history_file" >&2
  fi
}

cleanup() {
  if [[ -n "$agenttrace_process_pid" ]] && sudo -n kill -0 "$agenttrace_process_pid" 2>/dev/null; then
    sudo -n kill -INT "$agenttrace_process_pid" 2>/dev/null || true
  elif [[ -n "$agenttrace_pid" ]] && sudo -n kill -0 "$agenttrace_pid" 2>/dev/null; then
    sudo -n kill -INT "$agenttrace_pid" 2>/dev/null || true
  fi
  if [[ -n "$agenttrace_pid" ]]; then wait "$agenttrace_pid" 2>/dev/null || true; fi
  rm -f "$activity_file"
  rm -rf "$test_dir"
}
trap cleanup EXIT

if [[ ! -x "$agenttrace_bin" ]]; then
  echo "AgentTrace binary was not built: $agenttrace_bin" >&2
  exit 1
fi

# Root is required by the Linux CN_PROC and fanotify collectors on hosted runners.
# Keep the setup-python executable directory on PATH after sudo switches users.
sudo -n env "PATH=$PATH" "$agenttrace_bin" run --native-relay --json --output "$history_file" >"$agenttrace_stdout" 2>"$agenttrace_stderr" &
agenttrace_pid=$!

native_pid=""
ready=false
for _ in $(seq 1 80); do
  if ! sudo -n kill -0 "$agenttrace_pid" 2>/dev/null; then
    echo "AgentTrace exited before NativeRelay reported a running collector." >&2
    show_logs
    exit 1
  fi
  if [[ -f "$history_file" ]]; then
    if [[ -z "$agenttrace_process_pid" ]]; then
      agenttrace_process_pid="$(python3 - "$history_file" <<'PY'
import json, sys
try:
    for line in open(sys.argv[1], encoding="utf-8"):
        event = json.loads(line)
        if event.get("event_type") == "session_started":
            print(event.get("session_id", "").split("-", 1)[0])
            raise SystemExit(0)
except (OSError, json.JSONDecodeError):
    pass
raise SystemExit(1)
PY
      )" || true
    fi
    native_and_agent_pids="$(python3 - "$history_file" <<'PY'
import json, sys
try:
    for line in open(sys.argv[1], encoding="utf-8"):
        event = json.loads(line)
        if event.get("event_type") == "native_relay.status":
            record = event.get("data", {}).get("native_record", {})
            if record.get("state") == "running":
                started = next((json.loads(row) for row in open(sys.argv[1], encoding="utf-8") if json.loads(row).get("event_type") == "session_started"), {})
                child_pid = started.get("data", {}).get("native_relay", {}).get("child_pid", "")
                agent_pid = started.get("session_id", "").split("-", 1)[0]
                print(f"{child_pid}:{agent_pid}")
                raise SystemExit(0)
except (OSError, json.JSONDecodeError):
    pass
raise SystemExit(1)
PY
    )" && { ready=true; break; }
  fi
  sleep 0.25
done

if [[ "$ready" != true || -z "$native_and_agent_pids" ]]; then
  echo "NativeRelay did not report a running Linux collector." >&2
  show_logs
  exit 1
fi
IFS=: read -r native_pid agenttrace_process_pid <<< "$native_and_agent_pids"
if [[ -z "$native_pid" || -z "$agenttrace_process_pid" ]]; then
  echo "Could not identify the AgentTrace and NativeRelay child PIDs." >&2
  show_logs
  exit 1
fi
sudo -n kill -0 "$native_pid"
sudo -n kill -0 "$agenttrace_process_pid"

# Generate scoped file open/modify events and kernel process start/exit events.
printf 'initial contents\n' > "$activity_file"
sleep 0.25
printf 'modified contents\n' > "$activity_file"
cat "$activity_file" >/dev/null
sleep 2 &
probe_pid=$!
wait "$probe_pid"

if ! python3 - "$history_file" "$activity_file" "$probe_pid" <<'PY'
import json, os, sys, time
history, activity, probe_pid = sys.argv[1], os.path.realpath(sys.argv[2]), int(sys.argv[3])
deadline = time.monotonic() + 15
while time.monotonic() < deadline:
    observations = []
    try:
        with open(history, encoding="utf-8") as stream:
            for line in stream:
                event = json.loads(line)
                if event.get("event_type") == "native_relay.observation":
                    observations.append(event["data"]["native_record"])
    except (OSError, json.JSONDecodeError, KeyError):
        time.sleep(0.1)
        continue
    file_opened = any(item.get("type") == "file.opened" and item.get("resource", {}).get("path") == activity for item in observations)
    file_modified = any(item.get("type") == "file.modified" and item.get("resource", {}).get("path") == activity for item in observations)
    process_started = any(item.get("type") == "process.started" and item.get("process", {}).get("pid") == probe_pid for item in observations)
    process_exited = any(item.get("type") == "process.exited" and item.get("process", {}).get("pid") == probe_pid for item in observations)
    if file_opened and file_modified and process_started and process_exited:
        print(f"Verified NativeRelay observations: file.opened, file.modified, process.started, process.exited (PID {probe_pid})")
        raise SystemExit(0)
    time.sleep(0.25)
print("Timed out waiting for required real observations.", file=sys.stderr)
print("Observed event types: " + ", ".join(sorted({item.get("type", "?") for item in observations})), file=sys.stderr)
raise SystemExit(1)
PY
then
  show_logs
  exit 1
fi

# SIGINT must drain NativeRelay's final status/events, mark the session complete,
# and leave no NativeRelay process behind.
sudo -n kill -INT "$agenttrace_process_pid"
set +e
wait "$agenttrace_pid"
agenttrace_exit=$?
set -e
agenttrace_pid=""
agenttrace_process_pid=""
if [[ $agenttrace_exit -ne 0 ]]; then
  echo "AgentTrace exited with status $agenttrace_exit after SIGINT." >&2
  show_logs
  exit 1
fi

python3 - "$history_file" "$native_pid" <<'PY'
import json, sys
history, native_pid = sys.argv[1], int(sys.argv[2])
events = [json.loads(line) for line in open(history, encoding="utf-8")]
stopped = next((event for event in reversed(events) if event.get("event_type") == "session_stopped"), None)
statuses = [event.get("data", {}).get("native_record", {}) for event in events if event.get("event_type") == "native_relay.status"]
if stopped is None:
    raise SystemExit("AgentTrace did not record session_stopped")
if stopped.get("data", {}).get("trace_complete") is not True or stopped.get("data", {}).get("observations_lost_or_incomplete") is not False:
    raise SystemExit(f"Successful observation run was marked incomplete: {stopped.get('data')}")
if not any(record.get("state") == "stopped" and record.get("clean_shutdown") is True for record in statuses):
    raise SystemExit("NativeRelay did not report a clean stopped status")
print(f"Verified complete AgentTrace session and NativeRelay clean shutdown (child PID {native_pid})")
PY

if sudo -n kill -0 "$native_pid" 2>/dev/null; then
  echo "NativeRelay child process $native_pid remains alive after AgentTrace shutdown." >&2
  show_logs
  exit 1
fi
