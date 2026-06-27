use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use std::ffi::OsStr;
use std::fs::{create_dir_all, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Manager, State};
use uuid::Uuid;

const DEFAULT_TIMEOUT_MS: u128 = 12_000;
const LOG_TIMEOUT_MS: u128 = 15_000;
const MAX_STDOUT_BYTES: usize = 1_500_000;
const MAX_STDERR_BYTES: usize = 80_000;
const ACTIVITY_LIMIT: usize = 150;
const STOP_APPROVAL_TTL_MS: u128 = 120_000;

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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    requested_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    approval_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    approval_reason: Option<String>,
}

#[derive(Default)]
struct ApprovalStore {
    stop_tokens: Mutex<HashMap<String, StopApprovalGrant>>,
}

#[derive(Debug, Clone)]
struct StopApprovalGrant {
    container_id: String,
    required_phrase: String,
    expires_at_ms: u128,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct StopApprovalChallenge {
    approval_id: String,
    container_id: String,
    required_phrase: String,
    expires_at_ms: u128,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StopApprovalInput {
    approval_id: String,
    acknowledgement: String,
    reason: String,
}

#[derive(Debug, Clone)]
struct ApprovalAudit {
    approval_id: String,
    reason: String,
    requested_by: String,
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
fn request_stop_approval(
    state: State<'_, ApprovalStore>,
    container_id: String,
) -> Result<StopApprovalChallenge, CommandFailure> {
    let id = validate_container_id(&container_id)?;
    let approval_id = Uuid::new_v4().to_string();
    let required_phrase = expected_stop_phrase(&id);
    let expires_at_ms = now_ms() + STOP_APPROVAL_TTL_MS;
    let grant = StopApprovalGrant {
        container_id: id.clone(),
        required_phrase: required_phrase.clone(),
        expires_at_ms,
    };

    let mut tokens = state.stop_tokens.lock().map_err(|_| CommandFailure {
        kind: "approval_store_unavailable".to_string(),
        message: "failed to access approval token store".to_string(),
        command: "container stop".to_string(),
        exit_code: None,
        stderr: String::new(),
    })?;
    remove_expired_approvals(&mut tokens);
    tokens.insert(approval_id.clone(), grant);

    Ok(StopApprovalChallenge {
        approval_id,
        container_id: id,
        required_phrase,
        expires_at_ms,
    })
}

#[tauri::command]
fn stop_container(
    app: AppHandle,
    state: State<'_, ApprovalStore>,
    container_id: String,
    approval: StopApprovalInput,
) -> Result<OperationResult, CommandFailure> {
    let id = validate_container_id(&container_id)?;
    let audit = validate_stop_approval(&state, &id, approval)?;
    run_operation_with_audit(&app, "container_stop", &["stop", &id], Some(&audit))
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
    run_operation_with_audit(app, action, args, None)
}

fn run_operation_with_audit(
    app: &AppHandle,
    action: &str,
    args: &[&str],
    approval: Option<&ApprovalAudit>,
) -> Result<OperationResult, CommandFailure> {
    let output = run_container_with_audit(app, action, args, DEFAULT_TIMEOUT_MS, approval)?;
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
        stderr: mask_sensitive(&output.stderr),
    })
}

fn run_container(
    app: &AppHandle,
    action: &str,
    args: &[&str],
    timeout_ms: u128,
) -> Result<CommandOutput, CommandFailure> {
    run_container_with_audit(app, action, args, timeout_ms, None)
}

fn run_container_with_audit(
    app: &AppHandle,
    action: &str,
    args: &[&str],
    timeout_ms: u128,
    approval: Option<&ApprovalAudit>,
) -> Result<CommandOutput, CommandFailure> {
    let container_bin = resolve_container_bin()?;
    let display_command = command_label(&container_bin, args);
    let started_at_ms = now_ms();
    let started = SystemTime::now();
    let mut command = Command::new(&container_bin);
    command
        .args(args.iter().map(OsStr::new))
        .env_clear()
        .env("HOME", std::env::var_os("HOME").unwrap_or_default())
        .env("PATH", "/usr/local/bin:/opt/homebrew/bin:/usr/bin:/bin")
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
                        stderr: mask_sensitive(&stderr),
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
                            requested_by: approval.map(|item| item.requested_by.clone()),
                            approval_id: approval.map(|item| item.approval_id.clone()),
                            approval_reason: approval.map(|item| item.reason.clone()),
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
            requested_by: approval.map(|item| item.requested_by.clone()),
            approval_id: approval.map(|item| item.approval_id.clone()),
            approval_reason: approval.map(|item| item.reason.clone()),
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
            stderr: mask_sensitive(&stderr),
        })
    }
}

fn resolve_container_bin() -> Result<PathBuf, CommandFailure> {
    #[cfg(debug_assertions)]
    if let Some(path) = std::env::var_os("CONTAINER_UI_CONTAINER_BIN") {
        let candidate = PathBuf::from(path);
        if allowed_container_bin(&candidate) {
            return Ok(candidate);
        }
    }

    for candidate in [
        "/usr/local/bin/container",
        "/opt/homebrew/bin/container",
        "/usr/bin/container",
    ] {
        let path = PathBuf::from(candidate);
        if allowed_container_bin(&path) {
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

fn allowed_container_bin(path: &PathBuf) -> bool {
    path.is_file()
        && matches!(
            path.to_str(),
            Some("/usr/local/bin/container" | "/opt/homebrew/bin/container" | "/usr/bin/container")
        )
}

fn expected_stop_phrase(container_id: &str) -> String {
    format!("STOP {container_id}")
}

fn validate_stop_reason(reason: &str) -> Result<String, CommandFailure> {
    let trimmed = reason.trim();
    if (4..=180).contains(&trimmed.len()) {
        Ok(mask_sensitive(trimmed))
    } else {
        Err(CommandFailure {
            kind: "invalid_approval_reason".to_string(),
            message: "stop approval reason must be between 4 and 180 characters".to_string(),
            command: "container stop".to_string(),
            exit_code: None,
            stderr: String::new(),
        })
    }
}

fn validate_stop_approval(
    state: &State<'_, ApprovalStore>,
    container_id: &str,
    input: StopApprovalInput,
) -> Result<ApprovalAudit, CommandFailure> {
    let reason = validate_stop_reason(&input.reason)?;
    let mut tokens = state.stop_tokens.lock().map_err(|_| CommandFailure {
        kind: "approval_store_unavailable".to_string(),
        message: "failed to access approval token store".to_string(),
        command: "container stop".to_string(),
        exit_code: None,
        stderr: String::new(),
    })?;
    remove_expired_approvals(&mut tokens);

    let Some(grant) = tokens.get(&input.approval_id).cloned() else {
        return Err(CommandFailure {
            kind: "missing_stop_approval".to_string(),
            message: "stop operation requires a fresh approval challenge".to_string(),
            command: "container stop".to_string(),
            exit_code: None,
            stderr: String::new(),
        });
    };

    if grant.expires_at_ms < now_ms() {
        tokens.remove(&input.approval_id);
        return Err(CommandFailure {
            kind: "expired_stop_approval".to_string(),
            message: "stop approval challenge has expired".to_string(),
            command: "container stop".to_string(),
            exit_code: None,
            stderr: String::new(),
        });
    }

    if grant.container_id != container_id || input.acknowledgement.trim() != grant.required_phrase {
        return Err(CommandFailure {
            kind: "invalid_stop_approval".to_string(),
            message: "stop approval did not match the target container and required phrase"
                .to_string(),
            command: "container stop".to_string(),
            exit_code: None,
            stderr: String::new(),
        });
    }

    tokens.remove(&input.approval_id);
    Ok(ApprovalAudit {
        approval_id: input.approval_id,
        reason,
        requested_by: "local-user".to_string(),
    })
}

fn remove_expired_approvals(tokens: &mut HashMap<String, StopApprovalGrant>) {
    let now = now_ms();
    tokens.retain(|_, grant| grant.expires_at_ms >= now);
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
            if has_sensitive_marker(line) {
                "[masked sensitive line]".to_string()
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn has_sensitive_marker(line: &str) -> bool {
    let upper = line.to_ascii_uppercase();
    if upper.contains("BEGIN PRIVATE KEY") {
        return true;
    }

    [
        "TOKEN",
        "SECRET",
        "PASSWORD",
        "PRIVATE_KEY",
        "ACCESS_KEY",
        "ACCESSKEY",
    ]
    .iter()
    .any(|needle| {
        upper.contains(&format!("{needle}="))
            || upper.contains(&format!("{needle}:"))
            || upper.contains(&format!("\"{needle}\""))
    })
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
    let _ = compact_activity(app);
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

fn compact_activity(app: &AppHandle) -> Result<(), std::io::Error> {
    let Some(path) = activity_path(app) else {
        return Ok(());
    };
    let mut records = read_activity(app)?;
    records.reverse();
    let mut file = File::create(path)?;
    for record in records {
        if let Ok(line) = serde_json::to_string(&record) {
            writeln!(file, "{line}")?;
        }
    }
    Ok(())
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
        .manage(ApprovalStore::default())
        .invoke_handler(tauri::generate_handler![
            get_snapshot,
            inspect_container,
            get_container_logs,
            request_stop_approval,
            start_container,
            stop_container,
            get_activity
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::{
        expected_stop_phrase, mask_sensitive, validate_container_id, validate_stop_reason,
    };

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
        let masked = mask_sensitive("ok\nTOKEN=abc\nPASSWORD=leaked\nstill ok");
        assert!(masked.contains("ok"));
        assert!(!masked.contains("abc"));
        assert!(!masked.contains("leaked"));
    }

    #[test]
    fn builds_required_stop_phrase() {
        assert_eq!(
            expected_stop_phrase("container-ui-smoke"),
            "STOP container-ui-smoke"
        );
    }

    #[test]
    fn validates_stop_reason_length_and_masks_secret_lines() {
        assert!(validate_stop_reason("QA stop verification").is_ok());
        assert!(validate_stop_reason("bad").is_err());
        let masked = validate_stop_reason("PASSWORD=secret").expect("valid length");
        assert_eq!(masked, "[masked sensitive line]");
    }
}
