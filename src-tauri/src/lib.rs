use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use std::ffi::OsStr;
use std::fs::{create_dir_all, File, OpenOptions};
use std::io::{BufRead, BufReader, ErrorKind, Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::thread;
use std::thread::JoinHandle;
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
    requested_by: String,
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
    audit_approval_id: String,
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
        text: mask_sensitive(&output.stdout),
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
    app: AppHandle,
    state: State<'_, ApprovalStore>,
    container_id: String,
) -> Result<StopApprovalChallenge, CommandFailure> {
    let id = validate_container_id(&container_id)?;
    ensure_stop_allowed(&app, &id)?;
    let approval_id = Uuid::new_v4().to_string();
    let required_phrase = expected_stop_phrase(&id);
    let expires_at_ms = now_ms() + STOP_APPROVAL_TTL_MS;
    let requested_by = audit_actor(&app);
    let grant = StopApprovalGrant {
        container_id: id.clone(),
        required_phrase: required_phrase.clone(),
        expires_at_ms,
        requested_by: requested_by.clone(),
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
    drop(tokens);

    record_activity(
        &app,
        ActivityRecord {
            id: activity_id(now_ms(), "container_stop_approval_requested"),
            action: "container_stop_approval_requested".to_string(),
            command: format!("container stop {id}"),
            started_at_ms: now_ms(),
            duration_ms: 0,
            success: true,
            exit_code: None,
            stderr: String::new(),
            requested_by: Some(requested_by),
            approval_id: Some(audit_approval_id(&approval_id)),
            approval_reason: None,
        },
        true,
    )?;

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
    ensure_stop_allowed(&app, &id)?;
    let audit = validate_stop_approval(&state, &id, approval)?;
    record_activity(
        &app,
        ActivityRecord {
            id: activity_id(now_ms(), "container_stop_pending"),
            action: "container_stop_pending".to_string(),
            command: format!("container stop {id}"),
            started_at_ms: now_ms(),
            duration_ms: 0,
            success: true,
            exit_code: None,
            stderr: String::new(),
            requested_by: Some(audit.requested_by.clone()),
            approval_id: Some(audit.audit_approval_id.clone()),
            approval_reason: Some(audit.reason.clone()),
        },
        true,
    )?;
    run_operation_with_audit(&app, "container_stop", &["stop", &id], Some(&audit))
}

#[tauri::command]
fn get_activity(app: AppHandle) -> Vec<ActivityRecord> {
    match read_activity(&app) {
        Ok(records) => records,
        Err(err) => vec![failed_activity_record(
            "activity_read_failed",
            format!("failed to read activity log: {err}"),
        )],
    }
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
    let mut value =
        serde_json::from_str::<Value>(&output.stdout).map_err(|err| CommandFailure {
            kind: "invalid_json".to_string(),
            message: format!("container returned output that was not valid JSON: {err}"),
            command: output.command.clone(),
            exit_code: output.exit_code,
            stderr: mask_sensitive(&output.stderr),
        })?;
    redact_json_value(&mut value);
    Ok(value)
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
    let home_dir = app.path().home_dir().map_err(|err| CommandFailure {
        kind: "home_dir_unavailable".to_string(),
        message: format!("failed to resolve local home directory: {err}"),
        command: display_command.clone(),
        exit_code: None,
        stderr: String::new(),
    })?;
    let started_at_ms = now_ms();
    let started = SystemTime::now();
    let mut command = Command::new(&container_bin);
    command
        .args(args.iter().map(OsStr::new))
        .env_clear()
        .env("HOME", home_dir)
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

    let stdout = child.stdout.take().ok_or_else(|| CommandFailure {
        kind: "output_failed".to_string(),
        message: "failed to capture container CLI stdout".to_string(),
        command: display_command.clone(),
        exit_code: None,
        stderr: String::new(),
    })?;
    let stderr = child.stderr.take().ok_or_else(|| CommandFailure {
        kind: "output_failed".to_string(),
        message: "failed to capture container CLI stderr".to_string(),
        command: display_command.clone(),
        exit_code: None,
        stderr: String::new(),
    })?;

    let stdout_reader = thread::spawn(move || read_limited_stream(stdout, MAX_STDOUT_BYTES));
    let stderr_reader = thread::spawn(move || read_limited_stream(stderr, MAX_STDERR_BYTES));

    let mut timed_out = false;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if elapsed_ms(started) > timeout_ms {
                    timed_out = true;
                    let _ = child.kill();
                    break child.wait().map_err(|err| CommandFailure {
                        kind: "wait_failed".to_string(),
                        message: format!("failed while waiting for timed-out container CLI: {err}"),
                        command: display_command.clone(),
                        exit_code: None,
                        stderr: String::new(),
                    })?;
                }
                thread::sleep(Duration::from_millis(50));
            }
            Err(err) => {
                let _ = child.kill();
                return Err(CommandFailure {
                    kind: "wait_failed".to_string(),
                    message: format!("failed while waiting for container CLI: {err}"),
                    command: display_command.clone(),
                    exit_code: None,
                    stderr: String::new(),
                });
            }
        }
    };

    let duration_ms = elapsed_ms(started);
    let stdout = join_limited_reader(stdout_reader, "stdout", &display_command)?;
    let stderr = join_limited_reader(stderr_reader, "stderr", &display_command)?;
    let exit_code = status.code();
    let success = status.success() && !timed_out;

    record_activity(
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
            approval_id: approval.map(|item| item.audit_approval_id.clone()),
            approval_reason: approval.map(|item| item.reason.clone()),
        },
        approval.is_some(),
    )?;

    if timed_out {
        return Err(CommandFailure {
            kind: "timeout".to_string(),
            message: format!("container command timed out after {timeout_ms} ms"),
            command: display_command.clone(),
            exit_code,
            stderr: mask_sensitive(&stderr),
        });
    }

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
        audit_approval_id: audit_approval_id(&input.approval_id),
        reason,
        requested_by: grant.requested_by,
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
        && !id.starts_with('-')
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

fn read_limited_stream<R: Read>(mut reader: R, max_bytes: usize) -> std::io::Result<String> {
    let mut collected = Vec::with_capacity(max_bytes.min(8192));
    let mut buffer = [0_u8; 8192];
    let mut truncated = false;

    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }

        if collected.len() < max_bytes {
            let remaining = max_bytes - collected.len();
            let keep = read.min(remaining);
            collected.extend_from_slice(&buffer[..keep]);
            truncated |= keep < read;
        } else {
            truncated = true;
        }
    }

    let mut text = String::from_utf8_lossy(&collected).into_owned();
    if truncated {
        text.push_str(&format!("\n[output truncated to {max_bytes} bytes]"));
    }
    Ok(text)
}

fn join_limited_reader(
    handle: JoinHandle<std::io::Result<String>>,
    stream: &str,
    command: &str,
) -> Result<String, CommandFailure> {
    match handle.join() {
        Ok(Ok(text)) => Ok(text),
        Ok(Err(err)) => Err(CommandFailure {
            kind: "output_failed".to_string(),
            message: format!("failed to read container CLI {stream}: {err}"),
            command: command.to_string(),
            exit_code: None,
            stderr: String::new(),
        }),
        Err(_) => Err(CommandFailure {
            kind: "output_failed".to_string(),
            message: format!("failed to join container CLI {stream} reader"),
            command: command.to_string(),
            exit_code: None,
            stderr: String::new(),
        }),
    }
}

#[cfg(test)]
fn trim_bytes(bytes: &[u8], max_bytes: usize) -> String {
    if bytes.len() <= max_bytes {
        String::from_utf8_lossy(bytes).into_owned()
    } else {
        let text = String::from_utf8_lossy(&bytes[..max_bytes]);
        format!("{}\n[output truncated to {max_bytes} bytes]", text)
    }
}

fn mask_sensitive(input: &str) -> String {
    let mut in_private_key = false;
    let mut output = Vec::new();

    for line in input.lines() {
        let upper = line.to_ascii_uppercase();
        if in_private_key {
            output.push("[masked sensitive line]".to_string());
            if upper.contains("END ") && upper.contains("PRIVATE KEY") {
                in_private_key = false;
            }
            continue;
        }

        if upper.contains("BEGIN ") && upper.contains("PRIVATE KEY") {
            in_private_key = true;
            output.push("[masked sensitive line]".to_string());
        } else if has_sensitive_marker(line) {
            output.push("[masked sensitive line]".to_string());
        } else {
            output.push(line.to_string());
        }
    }

    output.join("\n")
}

fn redact_json_value(value: &mut Value) {
    match value {
        Value::String(text) => {
            *text = mask_sensitive(text);
        }
        Value::Array(items) => {
            for item in items {
                redact_json_value(item);
            }
        }
        Value::Object(map) => {
            for (key, item) in map {
                if is_sensitive_key(key) {
                    *item = Value::String("[masked sensitive value]".to_string());
                } else {
                    redact_json_value(item);
                }
            }
        }
        _ => {}
    }
}

fn has_sensitive_marker(line: &str) -> bool {
    let upper = line.to_ascii_uppercase();
    if upper.contains("BEGIN PRIVATE KEY") {
        return true;
    }

    sensitive_key_markers().iter().any(|needle| {
        upper.contains(&format!("{needle}="))
            || upper.contains(&format!("{needle}:"))
            || upper.contains(&format!("\"{needle}\""))
    }) || upper.contains("BEARER ")
        || has_url_userinfo(line)
}

fn is_sensitive_key(key: &str) -> bool {
    let normalized = normalize_key_marker(key);
    sensitive_key_markers()
        .iter()
        .any(|needle| normalize_key_marker(needle) == normalized)
}

fn sensitive_key_markers() -> [&'static str; 10] {
    [
        "TOKEN",
        "SECRET",
        "PASSWORD",
        "PRIVATE_KEY",
        "ACCESS_KEY",
        "ACCESSKEY",
        "API_KEY",
        "APIKEY",
        "AUTHORIZATION",
        "AWS_SECRET_ACCESS_KEY",
    ]
}

fn normalize_key_marker(key: &str) -> String {
    key.chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(|character| character.to_uppercase())
        .collect()
}

fn has_url_userinfo(line: &str) -> bool {
    let Some(scheme_index) = line.find("://") else {
        return false;
    };
    let after_scheme = &line[scheme_index + 3..];
    let authority = after_scheme
        .split(['/', '?', '#', ' ', '\t'])
        .next()
        .unwrap_or_default();
    let Some(at_index) = authority.find('@') else {
        return false;
    };
    authority[..at_index].contains(':')
}

fn ensure_stop_allowed(app: &AppHandle, container_id: &str) -> Result<(), CommandFailure> {
    if is_protected_container_id(container_id)
        || inspect_indicates_managed_container(app, container_id)?
    {
        Err(CommandFailure {
            kind: "protected_container_stop_blocked".to_string(),
            message: "managed apple/container resources cannot be stopped from Container UI"
                .to_string(),
            command: format!("container stop {container_id}"),
            exit_code: None,
            stderr: String::new(),
        })
    } else {
        Ok(())
    }
}

fn is_protected_container_id(container_id: &str) -> bool {
    container_id == "buildkit"
}

fn inspect_indicates_managed_container(
    app: &AppHandle,
    container_id: &str,
) -> Result<bool, CommandFailure> {
    let output = run_container(
        app,
        "container_stop_preflight_inspect",
        &["inspect", container_id],
        DEFAULT_TIMEOUT_MS,
    )?;
    let value = parse_json_output(&output)?;
    Ok(json_has_managed_container_marker(&value))
}

fn json_has_managed_container_marker(value: &Value) -> bool {
    match value {
        Value::Array(items) => items.iter().any(json_has_managed_container_marker),
        Value::Object(map) => {
            for (key, item) in map {
                let key_lower = key.to_ascii_lowercase();
                if key_lower == "labels" && labels_indicate_managed(item) {
                    return true;
                }
                if json_has_managed_container_marker(item) {
                    return true;
                }
            }
            false
        }
        _ => false,
    }
}

fn labels_indicate_managed(value: &Value) -> bool {
    let Value::Object(labels) = value else {
        return false;
    };

    labels.keys().any(|key| {
        key == "com.apple.container.plugin" || key == "com.apple.container.resource.role"
    })
}

fn audit_actor(app: &AppHandle) -> String {
    app.path()
        .home_dir()
        .ok()
        .and_then(|path| {
            path.file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .filter(|name| {
            !name.is_empty()
                && name
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        })
        .map(|name| format!("local-user:{name}"))
        .unwrap_or_else(|| "local-user:unknown".to_string())
}

fn audit_approval_id(approval_id: &str) -> String {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in approval_id.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("approval:{hash:016x}")
}

fn activity_id(started_at_ms: u128, action: &str) -> String {
    format!("{started_at_ms}-{action}")
}

fn required_activity_path(app: &AppHandle) -> Result<PathBuf, std::io::Error> {
    let dir = app.path().app_data_dir().map_err(std::io::Error::other)?;
    Ok(dir.join("activity.jsonl"))
}

fn record_activity(
    app: &AppHandle,
    record: ActivityRecord,
    required: bool,
) -> Result<(), CommandFailure> {
    match append_activity(app, record) {
        Ok(()) => Ok(()),
        Err(err) if required => Err(CommandFailure {
            kind: "activity_log_unavailable".to_string(),
            message: format!("failed to write required activity audit record: {err}"),
            command: "activity log".to_string(),
            exit_code: None,
            stderr: String::new(),
        }),
        Err(_) => Ok(()),
    }
}

fn append_activity(app: &AppHandle, record: ActivityRecord) -> Result<(), std::io::Error> {
    let path = required_activity_path(app)?;
    let Some(parent) = path.parent() else {
        return Err(std::io::Error::other(
            "activity path has no parent directory",
        ));
    };
    create_dir_all(parent)?;
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    let line = serde_json::to_string(&record).map_err(std::io::Error::other)?;
    writeln!(file, "{line}")?;
    compact_activity(app)?;
    Ok(())
}

fn read_activity(app: &AppHandle) -> Result<Vec<ActivityRecord>, std::io::Error> {
    let path = required_activity_path(app)?;
    let file = match File::open(path) {
        Ok(file) => file,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err),
    };
    let reader = BufReader::new(file);
    let mut records = VecDeque::with_capacity(ACTIVITY_LIMIT);

    for (line_index, line_result) in reader.lines().enumerate() {
        let line = match line_result {
            Ok(line) => line,
            Err(err) => {
                push_bounded_activity(
                    &mut records,
                    failed_activity_record(
                        "activity_read_line_failed",
                        format!("failed to read activity log line {}: {err}", line_index + 1),
                    ),
                );
                continue;
            }
        };
        let record = match serde_json::from_str::<ActivityRecord>(&line) {
            Ok(record) => sanitize_activity_record(record),
            Err(err) => failed_activity_record(
                "activity_corrupt_line",
                format!(
                    "failed to parse activity log line {}: {err}",
                    line_index + 1
                ),
            ),
        };
        push_bounded_activity(&mut records, record);
    }

    Ok(records.into_iter().rev().collect())
}

fn sanitize_activity_record(mut record: ActivityRecord) -> ActivityRecord {
    record.stderr = mask_sensitive(&record.stderr);
    record.approval_reason = record.approval_reason.as_deref().map(mask_sensitive);
    record.approval_id = record.approval_id.as_deref().map(|approval_id| {
        if approval_id.starts_with("approval:") {
            approval_id.to_string()
        } else {
            audit_approval_id(approval_id)
        }
    });
    record
}

fn push_bounded_activity(records: &mut VecDeque<ActivityRecord>, record: ActivityRecord) {
    if records.len() == ACTIVITY_LIMIT {
        records.pop_front();
    }
    records.push_back(record);
}

fn failed_activity_record(action: &str, message: String) -> ActivityRecord {
    ActivityRecord {
        id: activity_id(now_ms(), action),
        action: action.to_string(),
        command: "activity.jsonl".to_string(),
        started_at_ms: now_ms(),
        duration_ms: 0,
        success: false,
        exit_code: None,
        stderr: mask_sensitive(&message),
        requested_by: None,
        approval_id: None,
        approval_reason: None,
    }
}

fn compact_activity(app: &AppHandle) -> Result<(), std::io::Error> {
    let path = required_activity_path(app)?;
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
        audit_approval_id, expected_stop_phrase, failed_activity_record,
        json_has_managed_container_marker, mask_sensitive, redact_json_value, trim_bytes,
        validate_container_id, validate_stop_reason,
    };
    use serde_json::json;

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
        assert!(validate_container_id("--help").is_err());
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

    #[test]
    fn redacts_broader_credential_shapes() {
        let masked = mask_sensitive(
            "Authorization: Bearer abc\nAPI_KEY=def\nhttps://user:pass@example.com\n-----BEGIN PRIVATE KEY-----\nabc123\n-----END PRIVATE KEY-----",
        );
        assert!(!masked.contains("Bearer abc"));
        assert!(!masked.contains("def"));
        assert!(!masked.contains("user:pass"));
        assert!(!masked.contains("abc123"));
    }

    #[test]
    fn trims_on_byte_boundaries_without_utf8_panic() {
        let text = trim_bytes("あいうえお".as_bytes(), 4);
        assert!(text.contains("[output truncated to 4 bytes]"));
    }

    #[test]
    fn blocks_protected_container_stop_targets() {
        let managed = json!({
            "configuration": {
                "labels": {
                    "com.apple.container.plugin": "builder"
                }
            }
        });
        assert!(json_has_managed_container_marker(&managed));
        assert!(!json_has_managed_container_marker(
            &json!({"configuration": {"labels": {}}})
        ));
    }

    #[test]
    fn redacts_sensitive_json_keys() {
        let mut value = json!({
            "apiKey": "abc",
            "authorization": "Bearer def",
            "nested": {"normal": "visible"}
        });
        redact_json_value(&mut value);
        let text = value.to_string();
        assert!(!text.contains("abc"));
        assert!(!text.contains("Bearer def"));
        assert!(text.contains("visible"));
    }

    #[test]
    fn creates_non_secret_approval_fingerprints() {
        let fingerprint = audit_approval_id("12345678-1234-1234-1234-123456789abc");
        assert!(fingerprint.starts_with("approval:"));
        assert!(!fingerprint.contains("12345678-1234"));
    }

    #[test]
    fn activity_read_failures_surface_as_failed_records() {
        let record = failed_activity_record(
            "activity_read_failed",
            "failed to read activity log: unavailable".to_string(),
        );
        assert_eq!(record.action, "activity_read_failed");
        assert_eq!(record.command, "activity.jsonl");
        assert!(!record.success);
        assert!(record.stderr.contains("unavailable"));
    }
}
