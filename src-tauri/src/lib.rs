use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::ffi::OsStr;
use std::fs::{create_dir_all, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Manager};

const DEFAULT_TIMEOUT_MS: u128 = 12_000;
const LOG_TIMEOUT_MS: u128 = 15_000;
const MAX_STDOUT_BYTES: usize = 1_500_000;
const MAX_STDERR_BYTES: usize = 80_000;
const ACTIVITY_LIMIT: usize = 150;

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CommandFailure {
    kind: String,
    message: String,
    command: String,
    exit_code: Option<i32>,
    stderr: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct ActivityRecord {
    id: String,
    action: String,
    command: String,
    started_at_ms: u128,
    duration_ms: u128,
    success: bool,
    exit_code: Option<i32>,
    stderr: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PanelResult {
    ok: bool,
    data: Option<Value>,
    error: Option<CommandFailure>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Snapshot {
    generated_at_ms: u128,
    cli_version: PanelResult,
    system_status: PanelResult,
    containers: PanelResult,
    images: PanelResult,
    volumes: PanelResult,
    networks: PanelResult,
    stats: PanelResult,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct TextResult {
    text: String,
    command: String,
    duration_ms: u128,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct OperationResult {
    stdout: String,
    stderr: String,
    command: String,
    duration_ms: u128,
}

struct CommandOutput {
    stdout: String,
    stderr: String,
    exit_code: Option<i32>,
    duration_ms: u128,
    command: String,
}

#[tauri::command]
fn get_snapshot(app: AppHandle) -> Snapshot {
    Snapshot {
        generated_at_ms: now_ms(),
        cli_version: capture_json_or_text(&app, "cli_version", &["--version"]),
        system_status: capture_json(
            &app,
            "system_status",
            &["system", "status", "--format", "json"],
        ),
        containers: capture_json(
            &app,
            "containers_list",
            &["list", "--all", "--format", "json"],
        ),
        images: capture_json(
            &app,
            "images_list",
            &["image", "list", "--format", "json", "--verbose"],
        ),
        volumes: capture_json(
            &app,
            "volumes_list",
            &["volume", "list", "--format", "json"],
        ),
        networks: capture_json(
            &app,
            "networks_list",
            &["network", "list", "--format", "json"],
        ),
        stats: capture_json(
            &app,
            "stats_once",
            &["stats", "--format", "json", "--no-stream"],
        ),
    }
}

#[tauri::command]
fn inspect_container(app: AppHandle, container_id: String) -> Result<Value, CommandFailure> {
    let id = validate_container_id(&container_id)?;
    let output = run_container(
        &app,
        "container_inspect",
        &["inspect", &id],
        DEFAULT_TIMEOUT_MS,
    )?;
    parse_json_output(&output)
}

#[tauri::command]
fn get_container_logs(
    app: AppHandle,
    container_id: String,
    boot: bool,
    lines: u16,
) -> Result<TextResult, CommandFailure> {
    let id = validate_container_id(&container_id)?;
    let bounded_lines = lines.clamp(1, 2_000).to_string();
    let args = if boot {
        vec!["logs", "--boot", "-n", bounded_lines.as_str(), id.as_str()]
    } else {
        vec!["logs", "-n", bounded_lines.as_str(), id.as_str()]
    };
    let output = run_container(&app, "container_logs", &args, LOG_TIMEOUT_MS)?;
    Ok(TextResult {
        text: output.stdout,
        command: output.command,
        duration_ms: output.duration_ms,
    })
}

#[tauri::command]
fn start_container(
    app: AppHandle,
    container_id: String,
) -> Result<OperationResult, CommandFailure> {
    let id = validate_container_id(&container_id)?;
    run_operation(&app, "container_start", &["start", &id])
}

#[tauri::command]
fn stop_container(app: AppHandle, container_id: String) -> Result<OperationResult, CommandFailure> {
    let id = validate_container_id(&container_id)?;
    run_operation(&app, "container_stop", &["stop", &id])
}

#[tauri::command]
fn get_activity(app: AppHandle) -> Vec<ActivityRecord> {
    read_activity(&app).unwrap_or_default()
}

fn run_operation(
    app: &AppHandle,
    action: &str,
    args: &[&str],
) -> Result<OperationResult, CommandFailure> {
    let output = run_container(app, action, args, DEFAULT_TIMEOUT_MS)?;
    Ok(OperationResult {
        stdout: output.stdout,
        stderr: output.stderr,
        command: output.command,
        duration_ms: output.duration_ms,
    })
}

fn capture_json(app: &AppHandle, action: &str, args: &[&str]) -> PanelResult {
    match run_container(app, action, args, DEFAULT_TIMEOUT_MS)
        .and_then(|output| parse_json_output(&output))
    {
        Ok(data) => PanelResult {
            ok: true,
            data: Some(data),
            error: None,
        },
        Err(error) => PanelResult {
            ok: false,
            data: None,
            error: Some(error),
        },
    }
}

fn capture_json_or_text(app: &AppHandle, action: &str, args: &[&str]) -> PanelResult {
    match run_container(app, action, args, DEFAULT_TIMEOUT_MS) {
        Ok(output) => PanelResult {
            ok: true,
            data: Some(json!({ "text": output.stdout.trim() })),
            error: None,
        },
        Err(error) => PanelResult {
            ok: false,
            data: None,
            error: Some(error),
        },
    }
}

fn parse_json_output(output: &CommandOutput) -> Result<Value, CommandFailure> {
    serde_json::from_str::<Value>(&output.stdout).map_err(|err| CommandFailure {
        kind: "invalid_json".to_string(),
        message: format!("container returned output that was not valid JSON: {err}"),
        command: output.command.clone(),
        exit_code: output.exit_code,
        stderr: output.stderr.clone(),
    })
}

fn run_container(
    app: &AppHandle,
    action: &str,
    args: &[&str],
    timeout_ms: u128,
) -> Result<CommandOutput, CommandFailure> {
    let container_bin = resolve_container_bin()?;
    let display_command = command_label(&container_bin, args);
    let started_at_ms = now_ms();
    let started = SystemTime::now();
    let mut command = Command::new(&container_bin);
    command
        .args(args.iter().map(OsStr::new))
        .env("NO_COLOR", "true")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = command.spawn().map_err(|err| CommandFailure {
        kind: "spawn_failed".to_string(),
        message: format!("failed to start container CLI: {err}"),
        command: display_command.clone(),
        exit_code: None,
        stderr: String::new(),
    })?;

    let output = loop {
        match child.try_wait() {
            Ok(Some(_)) => break child.wait_with_output(),
            Ok(None) => {
                if elapsed_ms(started) > timeout_ms {
                    let _ = child.kill();
                    let output = child.wait_with_output();
                    let duration_ms = elapsed_ms(started);
                    let stderr = output
                        .as_ref()
                        .ok()
                        .map(|out| trim_bytes(&out.stderr, MAX_STDERR_BYTES))
                        .unwrap_or_default();
                    let failure = CommandFailure {
                        kind: "timeout".to_string(),
                        message: format!("container command timed out after {timeout_ms} ms"),
                        command: display_command.clone(),
                        exit_code: None,
                        stderr: stderr.clone(),
                    };
                    append_activity(
                        app,
                        ActivityRecord {
                            id: activity_id(started_at_ms, action),
                            action: action.to_string(),
                            command: display_command.clone(),
                            started_at_ms,
                            duration_ms,
                            success: false,
                            exit_code: None,
                            stderr: mask_sensitive(&stderr),
                        },
                    );
                    return Err(failure);
                }
                thread::sleep(Duration::from_millis(50));
            }
            Err(err) => {
                return Err(CommandFailure {
                    kind: "wait_failed".to_string(),
                    message: format!("failed while waiting for container CLI: {err}"),
                    command: display_command.clone(),
                    exit_code: None,
                    stderr: String::new(),
                });
            }
        }
    }
    .map_err(|err| CommandFailure {
        kind: "output_failed".to_string(),
        message: format!("failed to read container CLI output: {err}"),
        command: display_command.clone(),
        exit_code: None,
        stderr: String::new(),
    })?;

    let duration_ms = elapsed_ms(started);
    let stdout = trim_bytes(&output.stdout, MAX_STDOUT_BYTES);
    let stderr = trim_bytes(&output.stderr, MAX_STDERR_BYTES);
    let exit_code = output.status.code();
    let success = output.status.success();

    append_activity(
        app,
        ActivityRecord {
            id: activity_id(started_at_ms, action),
            action: action.to_string(),
            command: display_command.clone(),
            started_at_ms,
            duration_ms,
            success,
            exit_code,
            stderr: mask_sensitive(&stderr),
        },
    );

    if success {
        Ok(CommandOutput {
            stdout,
            stderr,
            exit_code,
            duration_ms,
            command: display_command,
        })
    } else {
        Err(CommandFailure {
            kind: "command_failed".to_string(),
            message: "container CLI returned a non-zero exit code".to_string(),
            command: display_command,
            exit_code,
            stderr,
        })
    }
}

fn resolve_container_bin() -> Result<PathBuf, CommandFailure> {
    if let Some(path) = std::env::var_os("CONTAINER_UI_CONTAINER_BIN") {
        let candidate = PathBuf::from(path);
        if candidate.is_file() {
            return Ok(candidate);
        }
    }

    for candidate in [
        "/usr/local/bin/container",
        "/opt/homebrew/bin/container",
        "/usr/bin/container",
    ] {
        let path = PathBuf::from(candidate);
        if path.is_file() {
            return Ok(path);
        }
    }

    Err(CommandFailure {
        kind: "not_installed".to_string(),
        message: "container CLI was not found in a supported install location".to_string(),
        command: "container".to_string(),
        exit_code: None,
        stderr: String::new(),
    })
}

fn validate_container_id(input: &str) -> Result<String, CommandFailure> {
    let id = input.trim();
    let valid = !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b':'));

    if valid {
        Ok(id.to_string())
    } else {
        Err(CommandFailure {
            kind: "invalid_container_id".to_string(),
            message: "container id contains unsupported characters".to_string(),
            command: "container".to_string(),
            exit_code: None,
            stderr: String::new(),
        })
    }
}

fn command_label(bin: &PathBuf, args: &[&str]) -> String {
    let mut parts = vec![bin.display().to_string()];
    parts.extend(args.iter().map(|arg| arg.to_string()));
    parts.join(" ")
}

fn trim_bytes(bytes: &[u8], max_bytes: usize) -> String {
    let text = String::from_utf8_lossy(bytes);
    if text.len() <= max_bytes {
        text.into_owned()
    } else {
        format!(
            "{}\n[output truncated to {} bytes]",
            &text[..max_bytes],
            max_bytes
        )
    }
}

fn mask_sensitive(input: &str) -> String {
    input
        .lines()
        .map(|line| {
            let upper = line.to_ascii_uppercase();
            if ["TOKEN", "SECRET", "PASSWORD", "PRIVATE_KEY", "ACCESS_KEY"]
                .iter()
                .any(|needle| upper.contains(needle))
            {
                "[masked sensitive line]".to_string()
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn activity_id(started_at_ms: u128, action: &str) -> String {
    format!("{started_at_ms}-{action}")
}

fn activity_path(app: &AppHandle) -> Option<PathBuf> {
    let dir = app.path().app_data_dir().ok()?;
    Some(dir.join("activity.jsonl"))
}

fn append_activity(app: &AppHandle, record: ActivityRecord) {
    let Some(path) = activity_path(app) else {
        return;
    };
    let Some(parent) = path.parent() else {
        return;
    };
    if create_dir_all(parent).is_err() {
        return;
    }
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
        if let Ok(line) = serde_json::to_string(&record) {
            let _ = writeln!(file, "{line}");
        }
    }
}

fn read_activity(app: &AppHandle) -> Result<Vec<ActivityRecord>, std::io::Error> {
    let Some(path) = activity_path(app) else {
        return Ok(Vec::new());
    };
    let file = File::open(path)?;
    let reader = BufReader::new(file);
    let mut records = VecDeque::with_capacity(ACTIVITY_LIMIT);

    for line in reader.lines().map_while(Result::ok) {
        if let Ok(record) = serde_json::from_str::<ActivityRecord>(&line) {
            if records.len() == ACTIVITY_LIMIT {
                records.pop_front();
            }
            records.push_back(record);
        }
    }

    Ok(records.into_iter().rev().collect())
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn elapsed_ms(started: SystemTime) -> u128 {
    SystemTime::now()
        .duration_since(started)
        .unwrap_or_default()
        .as_millis()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            get_snapshot,
            inspect_container,
            get_container_logs,
            start_container,
            stop_container,
            get_activity
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::{mask_sensitive, validate_container_id};

    #[test]
    fn validates_safe_container_ids() {
        assert!(validate_container_id("buildkit").is_ok());
        assert!(validate_container_id("agentab-dev-up").is_ok());
        assert!(validate_container_id("name_1.2:debug").is_ok());
    }

    #[test]
    fn rejects_unsafe_container_ids() {
        assert!(validate_container_id("../etc/passwd").is_err());
        assert!(validate_container_id("bad id").is_err());
        assert!(validate_container_id("$(rm -rf /)").is_err());
    }

    #[test]
    fn masks_sensitive_log_lines() {
        let masked = mask_sensitive("ok\nTOKEN=abc\npassword leaked\nstill ok");
        assert!(masked.contains("ok"));
        assert!(!masked.contains("abc"));
        assert!(!masked.contains("leaked"));
    }
}
