use std::collections::{hash_map::DefaultHasher, HashMap};
use std::env;
use std::fs;
use std::hash::{Hash, Hasher};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use tauri::Manager;

static ACTIVE_SCANS: Lazy<Mutex<HashMap<String, Arc<AtomicBool>>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));
const KEYCHAIN_SERVICE: &str = "com.duckdisk.dev.ssh";
const SCAN_CANCELLED_MESSAGE: &str = "SSH scan cancelled.";
const PROGRESS_PREFIX: &str = "DUCKDISK_PROGRESS\t";

#[derive(Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum SshAuthMethod {
    Key,
    Password,
}

impl Default for SshAuthMethod {
    fn default() -> Self {
        Self::Key
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SshConnection {
    pub id: String,
    pub name: String,
    pub host: String,
    pub port: u16,
    pub path: String,
    #[serde(default)]
    pub auth_method: SshAuthMethod,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub storage_usage: Option<SshStorageUsage>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SshConnectionInput {
    #[serde(default)]
    id: Option<String>,
    name: String,
    host: String,
    port: u16,
    path: String,
    #[serde(default)]
    auth_method: SshAuthMethod,
    #[serde(default)]
    password: String,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SshStorageUsage {
    total_space: u64,
    used_space: u64,
    available_space: u64,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct AccountPayload {
    account_id: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ScanStatusPayload {
    account_id: String,
    items: u64,
    total: u64,
    operation_not_permitted: u64,
    permission_denied: u64,
    interrupted: u64,
    other: u64,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct CompletedPayload {
    account_id: String,
    path: String,
    errors_path: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct FailedPayload {
    account_id: String,
    message: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct DeleteStatusPayload {
    current: u64,
    total: u64,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SshDeleteFailure {
    item_id: String,
    message: String,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SshDeleteResult {
    deleted_ids: Vec<String>,
    failures: Vec<SshDeleteFailure>,
}

pub fn get_connections(app_handle: &tauri::AppHandle) -> Result<Vec<SshConnection>, String> {
    read_connections(app_handle)
}

pub fn get_storage_usage(
    app_handle: &tauri::AppHandle,
    connection_id: &str,
) -> Result<SshStorageUsage, String> {
    let connection = read_connections(app_handle)?
        .into_iter()
        .find(|item| item.id == connection_id)
        .ok_or_else(|| "SSH connection was not found".to_string())?;
    let remote_script = r#"import json, os, sys
stats = os.statvfs(sys.argv[1])
block_size = stats.f_frsize or stats.f_bsize
total = max(0, int(stats.f_blocks) * int(block_size))
available = max(0, int(stats.f_bavail) * int(block_size))
print(json.dumps({"totalSpace":total,"usedSpace":max(0,total-available),"availableSpace":available}, separators=(",",":")))"#;
    let remote_command = format!(
        "python3 -c {} {}",
        shell_quote(remote_script),
        shell_quote(&connection.path),
    );
    let output = ssh_command(&connection)?
        .arg(&connection.host)
        .arg(remote_command)
        .stdin(Stdio::null())
        .output()
        .map_err(|err| format!("Could not query SSH storage usage: {err}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if stderr.is_empty() {
            format!("SSH storage query exited with {}", output.status)
        } else {
            format!("SSH storage query failed: {stderr}")
        });
    }
    let usage: SshStorageUsage = serde_json::from_slice(&output.stdout)
        .map_err(|err| format!("SSH storage query returned invalid JSON: {err}"))?;
    if let Err(error) = store_storage_usage(app_handle, connection_id, &usage) {
        eprintln!("Could not cache SSH storage usage: {error}");
    }
    Ok(usage)
}

fn store_storage_usage(
    app_handle: &tauri::AppHandle,
    connection_id: &str,
    usage: &SshStorageUsage,
) -> Result<(), String> {
    let mut connections = read_connections(app_handle)?;
    let connection = connections
        .iter_mut()
        .find(|item| item.id == connection_id)
        .ok_or_else(|| "SSH connection was not found".to_string())?;
    connection.storage_usage = Some(usage.clone());
    write_connections(app_handle, &connections)
}

pub fn run_askpass_helper() -> Result<bool, String> {
    if env::var_os("DUCKDISK_SSH_ASKPASS").is_none() {
        return Ok(false);
    }
    let connection_id = env::var("DUCKDISK_SSH_CONNECTION_ID")
        .map_err(|_| "SSH password helper did not receive a connection ID".to_string())?;
    let password = read_password(&connection_id)?;
    io::stdout()
        .write_all(password.as_bytes())
        .map_err(|err| err.to_string())?;
    Ok(true)
}

pub fn save_connection(
    app_handle: &tauri::AppHandle,
    input: SshConnectionInput,
) -> Result<SshConnection, String> {
    let host = input.host.trim();
    let path = input.path.trim();
    if host.is_empty() {
        return Err("Enter an SSH host or alias".to_string());
    }
    validate_ssh_host(host)?;
    if path.is_empty() || !path.starts_with('/') {
        return Err("Remote path must be an absolute path".to_string());
    }
    if path.contains(['\0', '\n', '\r']) {
        return Err("Remote path contains unsupported control characters".to_string());
    }
    if input.port == 0 {
        return Err("SSH port must be between 1 and 65535".to_string());
    }
    let mut connections = read_connections(app_handle)?;
    let existing_index = input
        .id
        .as_ref()
        .map(|id| {
            connections
                .iter()
                .position(|connection| connection.id == *id)
                .ok_or_else(|| "SSH connection was not found".to_string())
        })
        .transpose()?;
    let existing = existing_index.map(|index| connections[index].clone());
    let keeps_existing_password = existing
        .as_ref()
        .map(|connection| connection.auth_method == SshAuthMethod::Password)
        .unwrap_or(false);
    if input.auth_method == SshAuthMethod::Password
        && input.password.is_empty()
        && !keeps_existing_password
    {
        return Err("Enter the SSH password".to_string());
    }
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let connection = SshConnection {
        id: existing
            .as_ref()
            .map(|connection| connection.id.clone())
            .unwrap_or_else(|| format!("ssh-{stamp:x}")),
        name: if input.name.trim().is_empty() {
            host.to_string()
        } else {
            input.name.trim().to_string()
        },
        host: host.to_string(),
        port: input.port,
        path: path.to_string(),
        auth_method: input.auth_method,
        storage_usage: None,
    };
    let previous_connections = connections.clone();
    if let Some(index) = existing_index {
        connections[index] = connection.clone();
    } else {
        connections.push(connection.clone());
    }
    if let Err(error) = write_connections(app_handle, &connections) {
        return Err(error);
    }

    let keychain_result = if connection.auth_method == SshAuthMethod::Password {
        if input.password.is_empty() {
            Ok(())
        } else {
            store_password(&connection.id, &input.password)
        }
    } else if keeps_existing_password {
        delete_password(&connection.id)
    } else {
        Ok(())
    };
    if let Err(error) = keychain_result {
        return match write_connections(app_handle, &previous_connections) {
            Ok(()) => Err(error),
            Err(rollback_error) => Err(format!(
                "{error}. DuckDisk also could not restore the previous SSH settings: {rollback_error}"
            )),
        };
    }
    if existing.is_some() {
        clear_cached_result(app_handle, &connection.id).ok();
    }
    Ok(connection)
}

pub fn remove_connection(app_handle: &tauri::AppHandle, connection_id: &str) -> Result<(), String> {
    let mut connections = read_connections(app_handle)?;
    let removed_password = connections
        .iter()
        .any(|item| item.id == connection_id && item.auth_method == SshAuthMethod::Password);
    connections.retain(|item| item.id != connection_id);
    write_connections(app_handle, &connections)?;
    if removed_password {
        delete_password(connection_id)?;
    }
    clear_cached_result(app_handle, connection_id)?;
    Ok(())
}

pub fn start_scan(
    app_handle: tauri::AppHandle,
    connection_id: String,
    force_full: bool,
) -> Result<(), String> {
    let connection = read_connections(&app_handle)?
        .into_iter()
        .find(|item| item.id == connection_id)
        .ok_or_else(|| "SSH connection was not found".to_string())?;
    if !force_full {
        if let Some(path) = copy_cached_result_to_temp(&app_handle, &connection_id)? {
            app_handle
                .emit_all(
                    "ssh_scan_incremental",
                    AccountPayload {
                        account_id: connection_id.clone(),
                    },
                )
                .ok();
            app_handle
                .emit_all(
                    "ssh_scan_completed",
                    CompletedPayload {
                        account_id: connection_id,
                        path: path.display().to_string(),
                        errors_path: String::new(),
                    },
                )
                .ok();
            return Ok(());
        }
    }
    let cancelled = Arc::new(AtomicBool::new(false));
    {
        let mut active = ACTIVE_SCANS.lock().unwrap_or_else(|item| item.into_inner());
        if active.contains_key(&connection_id) {
            return Ok(());
        }
        active.insert(connection_id.clone(), cancelled.clone());
    }
    app_handle
        .emit_all(
            "ssh_scan_full",
            AccountPayload {
                account_id: connection_id.clone(),
            },
        )
        .ok();
    app_handle
        .emit_all(
            "ssh_scan_status",
            ScanStatusPayload {
                account_id: connection_id.clone(),
                items: 0,
                total: 0,
                operation_not_permitted: 0,
                permission_denied: 0,
                interrupted: 0,
                other: 0,
            },
        )
        .ok();

    tauri::async_runtime::spawn_blocking(move || {
        match scan_connection(&app_handle, &connection, &cancelled) {
            Ok((path, content)) => {
                if let Err(message) = write_cached_result(&app_handle, &connection_id, &content) {
                    app_handle
                        .emit_all(
                            "ssh_scan_failed",
                            FailedPayload {
                                account_id: connection_id.clone(),
                                message,
                            },
                        )
                        .ok();
                    ACTIVE_SCANS
                        .lock()
                        .unwrap_or_else(|item| item.into_inner())
                        .remove(&connection_id);
                    return;
                }
                app_handle
                    .emit_all(
                        "ssh_scan_finalizing",
                        AccountPayload {
                            account_id: connection_id.clone(),
                        },
                    )
                    .ok();
                app_handle
                    .emit_all(
                        "ssh_scan_completed",
                        CompletedPayload {
                            account_id: connection_id.clone(),
                            path: path.display().to_string(),
                            errors_path: String::new(),
                        },
                    )
                    .ok();
            }
            Err(message) if message == SCAN_CANCELLED_MESSAGE => {}
            Err(message) => {
                app_handle
                    .emit_all(
                        "ssh_scan_failed",
                        FailedPayload {
                            account_id: connection_id.clone(),
                            message,
                        },
                    )
                    .ok();
            }
        }
        ACTIVE_SCANS
            .lock()
            .unwrap_or_else(|item| item.into_inner())
            .remove(&connection_id);
    });
    Ok(())
}

pub fn stop_scan(connection_id: &str) {
    if let Some(cancelled) = ACTIVE_SCANS
        .lock()
        .unwrap_or_else(|item| item.into_inner())
        .get(connection_id)
    {
        cancelled.store(true, Ordering::SeqCst);
    }
}

pub fn clear_cached_result(
    app_handle: &tauri::AppHandle,
    connection_id: &str,
) -> Result<(), String> {
    let path = cache_path(app_handle, connection_id)?;
    if path.exists() {
        fs::remove_file(path).map_err(|err| err.to_string())?;
    }
    Ok(())
}

pub fn delete_items(
    app_handle: &tauri::AppHandle,
    connection_id: &str,
    item_ids: Vec<String>,
) -> Result<SshDeleteResult, String> {
    if item_ids.is_empty() {
        return Ok(SshDeleteResult {
            deleted_ids: Vec::new(),
            failures: Vec::new(),
        });
    }
    let connection = read_connections(app_handle)?
        .into_iter()
        .find(|item| item.id == connection_id)
        .ok_or_else(|| "SSH connection was not found".to_string())?;
    let remote_script = r#"import json, os, stat, sys
root = os.path.realpath(sys.argv[1])
requested = json.loads(sys.argv[2])
deleted = []
failures = []
root_device = os.lstat(root).st_dev
def remove_tree(path):
    with os.scandir(path) as entries:
        for entry in entries:
            info = entry.stat(follow_symlinks=False)
            if info.st_dev != root_device:
                continue
            if stat.S_ISDIR(info.st_mode) and not stat.S_ISLNK(info.st_mode):
                remove_tree(entry.path)
            else:
                os.unlink(entry.path)
    os.rmdir(path)
for raw in requested:
    absolute = os.path.abspath(raw)
    path = os.path.join(os.path.realpath(os.path.dirname(absolute)), os.path.basename(absolute))
    try:
        inside = os.path.commonpath([root, path]) == root
    except ValueError:
        inside = False
    if not inside or path == root:
        failures.append({"itemId": raw, "message": "Refusing to delete outside the configured remote path or delete its root"})
        continue
    try:
        info = os.lstat(path)
        if info.st_dev != root_device:
            raise OSError("Item is on a different filesystem")
        if stat.S_ISDIR(info.st_mode) and not stat.S_ISLNK(info.st_mode):
            remove_tree(path)
        elif stat.S_ISREG(info.st_mode) or stat.S_ISLNK(info.st_mode):
            os.unlink(path)
        else:
            raise OSError("Refusing to delete a device or other special file")
        deleted.append(raw)
    except OSError as error:
        failures.append({"itemId": raw, "message": str(error)})
print(json.dumps({"deletedIds": deleted, "failures": failures}, separators=(",", ":")))"#;
    let remote_command = format!(
        "python3 -c {} {} {}",
        shell_quote(remote_script),
        shell_quote(&connection.path),
        shell_quote(&serde_json::to_string(&item_ids).map_err(|err| err.to_string())?),
    );
    let output = ssh_command(&connection)?
        .arg(&connection.host)
        .arg(&remote_command)
        .stdin(Stdio::null())
        .output()
        .map_err(|err| format!("Could not start ssh: {err}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if stderr.is_empty() {
            format!("SSH deletion exited with {}", output.status)
        } else {
            format!("SSH deletion failed: {stderr}")
        });
    }
    let result: SshDeleteResult = serde_json::from_slice(&output.stdout)
        .map_err(|err| format!("Remote deletion returned invalid JSON: {err}"))?;
    app_handle
        .emit_all(
            "ssh_delete_status",
            DeleteStatusPayload {
                current: item_ids.len() as u64,
                total: item_ids.len() as u64,
            },
        )
        .ok();
    if !result.deleted_ids.is_empty() {
        clear_cached_result(app_handle, connection_id)?;
    }
    Ok(result)
}

pub fn read_scan_result(path: &str) -> Result<String, String> {
    let prefix = format!("duckdisk-ssh-scan-{}-", std::process::id());
    let path = crate::temp_files::validate_result_file(path, &prefix)?;
    fs::read_to_string(path).map_err(|err| err.to_string())
}

fn scan_connection(
    app_handle: &tauri::AppHandle,
    connection: &SshConnection,
    cancelled: &Arc<AtomicBool>,
) -> Result<(PathBuf, String), String> {
    let remote_script = r#"import json, os, stat, sys
root = os.path.abspath(sys.argv[1])
errors = 0
items = 0
total = 0
try:
    root_device = os.lstat(root).st_dev
except OSError:
    raise SystemExit("Remote path could not be read")
def progress():
    sys.stderr.write("DUCKDISK_PROGRESS\t{}\t{}\n".format(items, total))
    sys.stderr.flush()
def walk(path):
    global errors, items, total
    try:
        info = os.lstat(path)
    except OSError:
        errors += 1
        return None
    folder = stat.S_ISDIR(info.st_mode) and not stat.S_ISLNK(info.st_mode)
    regular = stat.S_ISREG(info.st_mode) and not stat.S_ISLNK(info.st_mode)
    if path != root and info.st_dev != root_device:
        return None
    if not folder and not regular:
        return None
    items += 1
    node = {"name": os.path.basename(path) or path, "cloudId": path, "isDirectory": folder, "size": 0, "children": []}
    if not folder:
        blocks = getattr(info, "st_blocks", None)
        node["size"] = max(0, int(blocks) * 512 if blocks is not None else int(info.st_size))
        total += node["size"]
        if items % 512 == 0:
            progress()
        return node
    try:
        with os.scandir(path) as entries:
            for entry in entries:
                child = walk(entry.path)
                if child is not None:
                    node["children"].append(child)
    except OSError:
        errors += 1
    node["children"].sort(key=lambda child: child["size"], reverse=True)
    node["size"] = sum(child["size"] for child in node["children"])
    if items % 512 == 0:
        progress()
    return node
tree = walk(root)
if tree is None:
    raise SystemExit("Remote path could not be read")
tree["displayName"] = sys.argv[2]
progress()
print(json.dumps({"schema-version":"duckdisk-cloud-v1","unit":"bytes","tree":tree,"remoteErrors":errors,"remoteItems":items}, separators=(",",":")))"#;
    let remote_command = format!(
        "python3 -c {} {} {}",
        shell_quote(remote_script),
        shell_quote(&connection.path),
        shell_quote(&format!("{} ({})", connection.name, connection.host)),
    );
    let mut child = ssh_command(connection)?
        .arg(&connection.host)
        .arg(&remote_command)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| format!("Could not start ssh: {err}"))?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "Could not read SSH output".to_string())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "Could not read SSH errors".to_string())?;
    let stdout_reader = std::thread::spawn(move || {
        let mut content = String::new();
        BufReader::new(stdout)
            .read_to_string(&mut content)
            .map(|_| content)
            .map_err(|err| err.to_string())
    });
    let progress_app = app_handle.clone();
    let progress_account = connection.id.clone();
    let stderr_reader = std::thread::spawn(move || {
        let mut errors = Vec::new();
        for line in BufReader::new(stderr).lines() {
            let line = line.map_err(|err| err.to_string())?;
            if let Some((items, total)) = parse_progress(&line) {
                emit_scan_status(&progress_app, &progress_account, items, total);
            } else if !line.trim().is_empty() {
                errors.push(line);
            }
        }
        Ok::<String, String>(errors.join("\n"))
    });

    let status = loop {
        if cancelled.load(Ordering::SeqCst) {
            child.kill().ok();
            child.wait().ok();
            stdout_reader.join().ok();
            stderr_reader.join().ok();
            return Err(SCAN_CANCELLED_MESSAGE.to_string());
        }
        match child.try_wait().map_err(|err| err.to_string())? {
            Some(status) => break status,
            None => std::thread::sleep(Duration::from_millis(100)),
        }
    };
    let content = stdout_reader
        .join()
        .map_err(|_| "SSH output reader stopped unexpectedly".to_string())??;
    let stderr = stderr_reader
        .join()
        .map_err(|_| "SSH error reader stopped unexpectedly".to_string())??;
    if !status.success() {
        return Err(if stderr.is_empty() {
            format!("SSH scan exited with {status}")
        } else {
            format!("SSH scan failed: {stderr}")
        });
    }
    serde_json::from_str::<serde_json::Value>(&content)
        .map_err(|err| format!("Remote scan returned invalid JSON: {err}"))?;
    let path = temporary_result_path();
    fs::write(&path, content).map_err(|err| err.to_string())?;
    let content = fs::read_to_string(&path).map_err(|err| err.to_string())?;
    Ok((path, content))
}

fn ssh_command(connection: &SshConnection) -> Result<Command, String> {
    let port = connection.port.to_string();
    let mut command = Command::new("ssh");
    command.args(["-o", "ConnectTimeout=12", "-p", &port]);
    match connection.auth_method {
        SshAuthMethod::Key => {
            command.args(["-o", "BatchMode=yes"]);
        }
        SshAuthMethod::Password => {
            let askpass = env::current_exe()
                .map_err(|err| format!("Could not locate DuckDisk password helper: {err}"))?;
            command.args([
                "-o",
                "BatchMode=no",
                "-o",
                "NumberOfPasswordPrompts=1",
                "-o",
                "PreferredAuthentications=password,keyboard-interactive",
                "-o",
                "PubkeyAuthentication=no",
            ]);
            command
                .env("SSH_ASKPASS", askpass)
                .env("SSH_ASKPASS_REQUIRE", "force")
                .env("DISPLAY", "duckdisk")
                .env("DUCKDISK_SSH_ASKPASS", "1")
                .env("DUCKDISK_SSH_CONNECTION_ID", &connection.id);
        }
    }
    Ok(command)
}

fn validate_ssh_host(host: &str) -> Result<(), String> {
    if host.starts_with('-') || host.chars().any(char::is_whitespace) || host.contains('\0') {
        return Err("SSH host or alias cannot start with '-' or contain whitespace".to_string());
    }
    Ok(())
}

fn parse_progress(line: &str) -> Option<(u64, u64)> {
    let values = line.strip_prefix(PROGRESS_PREFIX)?;
    let mut parts = values.split('\t');
    let items = parts.next()?.parse().ok()?;
    let total = parts.next()?.parse().ok()?;
    Some((items, total))
}

fn emit_scan_status(app_handle: &tauri::AppHandle, account_id: &str, items: u64, total: u64) {
    app_handle
        .emit_all(
            "ssh_scan_status",
            ScanStatusPayload {
                account_id: account_id.to_string(),
                items,
                total,
                operation_not_permitted: 0,
                permission_denied: 0,
                interrupted: 0,
                other: 0,
            },
        )
        .ok();
}

fn cache_path(app_handle: &tauri::AppHandle, connection_id: &str) -> Result<PathBuf, String> {
    let mut hasher = DefaultHasher::new();
    connection_id.hash(&mut hasher);
    app_handle
        .path_resolver()
        .app_cache_dir()
        .map(|path| {
            path.join("ssh")
                .join(format!("{:016x}.json", hasher.finish()))
        })
        .ok_or_else(|| "Could not resolve DuckDisk cache directory".to_string())
}

fn temporary_result_path() -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    std::env::temp_dir().join(format!(
        "duckdisk-ssh-scan-{}-{stamp}.json",
        std::process::id()
    ))
}

fn write_cached_result(
    app_handle: &tauri::AppHandle,
    connection_id: &str,
    content: &str,
) -> Result<(), String> {
    let path = cache_path(app_handle, connection_id)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| err.to_string())?;
    }
    let temporary = path.with_extension("json.tmp");
    fs::write(&temporary, content).map_err(|err| err.to_string())?;
    fs::rename(temporary, path).map_err(|err| err.to_string())
}

fn copy_cached_result_to_temp(
    app_handle: &tauri::AppHandle,
    connection_id: &str,
) -> Result<Option<PathBuf>, String> {
    let cache = cache_path(app_handle, connection_id)?;
    if !cache.exists() {
        return Ok(None);
    }
    let content = fs::read_to_string(cache).map_err(|err| err.to_string())?;
    if serde_json::from_str::<serde_json::Value>(&content).is_err() {
        clear_cached_result(app_handle, connection_id)?;
        return Ok(None);
    }
    let result = temporary_result_path();
    fs::write(&result, content).map_err(|err| err.to_string())?;
    Ok(Some(result))
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn store_password(connection_id: &str, password: &str) -> Result<(), String> {
    keyring::Entry::new(KEYCHAIN_SERVICE, connection_id)
        .map_err(|err| err.to_string())?
        .set_password(password)
        .map_err(|err| format!("Could not save SSH password in macOS Keychain: {err}"))
}

fn read_password(connection_id: &str) -> Result<String, String> {
    keyring::Entry::new(KEYCHAIN_SERVICE, connection_id)
        .map_err(|err| err.to_string())?
        .get_password()
        .map_err(|err| format!("Could not read SSH password from macOS Keychain: {err}"))
}

fn delete_password(connection_id: &str) -> Result<(), String> {
    let entry =
        keyring::Entry::new(KEYCHAIN_SERVICE, connection_id).map_err(|err| err.to_string())?;
    match entry.delete_password() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(err) => Err(format!(
            "Could not remove SSH password from macOS Keychain: {err}"
        )),
    }
}

fn connections_path(app_handle: &tauri::AppHandle) -> Result<PathBuf, String> {
    app_handle
        .path_resolver()
        .app_config_dir()
        .map(|path| path.join("ssh-connections.json"))
        .ok_or_else(|| "Could not resolve DuckDisk configuration directory".to_string())
}

fn read_connections(app_handle: &tauri::AppHandle) -> Result<Vec<SshConnection>, String> {
    let path = connections_path(app_handle)?;
    if !path.exists() {
        return Ok(Vec::new());
    }
    serde_json::from_str(&fs::read_to_string(path).map_err(|err| err.to_string())?)
        .map_err(|err| err.to_string())
}

fn write_connections(
    app_handle: &tauri::AppHandle,
    connections: &[SshConnection],
) -> Result<(), String> {
    let path = connections_path(app_handle)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| err.to_string())?;
    }
    fs::write(
        path,
        serde_json::to_string_pretty(connections).map_err(|err| err.to_string())?,
    )
    .map_err(|err| err.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_quotes_remote_values() {
        assert_eq!(shell_quote("/tmp/a b"), "'/tmp/a b'");
        assert_eq!(shell_quote("a'b"), "'a'\"'\"'b'");
    }

    #[test]
    fn rejects_ssh_option_injection_as_a_host() {
        assert!(validate_ssh_host("-oProxyCommand=bad").is_err());
        assert!(validate_ssh_host("host name").is_err());
        assert!(validate_ssh_host("server.example.com").is_ok());
    }

    #[test]
    fn legacy_connections_default_to_ssh_key_authentication() {
        let connection: SshConnection = serde_json::from_str(
            r#"{"id":"legacy","name":"Legacy","host":"server","port":22,"path":"/"}"#,
        )
        .unwrap();
        assert!(connection.auth_method == SshAuthMethod::Key);
    }

    #[test]
    fn update_input_accepts_an_existing_connection_id() {
        let input: SshConnectionInput = serde_json::from_str(
            r#"{"id":"ssh-existing","name":"Server","host":"host","port":22,"path":"/","authMethod":"password","password":""}"#,
        ).unwrap();
        assert_eq!(input.id.as_deref(), Some("ssh-existing"));
        assert!(input.auth_method == SshAuthMethod::Password);
    }

    #[test]
    fn parses_remote_scan_progress() {
        assert_eq!(
            parse_progress("DUCKDISK_PROGRESS\t1250\t1048576"),
            Some((1250, 1_048_576))
        );
        assert_eq!(parse_progress("ssh warning"), None);
    }

    #[test]
    fn parses_remote_storage_usage() {
        let usage: SshStorageUsage =
            serde_json::from_str(r#"{"totalSpace":1000,"usedSpace":750,"availableSpace":250}"#)
                .unwrap();
        assert_eq!(usage.total_space, 1000);
        assert_eq!(usage.used_space, 750);
        assert_eq!(usage.available_space, 250);
    }
}
