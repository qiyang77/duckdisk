use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use tauri::Manager;

static ACTIVE_SCANS: Lazy<Mutex<HashSet<String>>> = Lazy::new(|| Mutex::new(HashSet::new()));

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SshConnection {
    pub id: String,
    pub name: String,
    pub host: String,
    pub port: u16,
    pub path: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SshConnectionInput {
    name: String,
    host: String,
    port: u16,
    path: String,
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

pub fn get_connections(app_handle: &tauri::AppHandle) -> Result<Vec<SshConnection>, String> {
    read_connections(app_handle)
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
    if path.is_empty() || !path.starts_with('/') {
        return Err("Remote path must be an absolute path".to_string());
    }
    if input.port == 0 {
        return Err("SSH port must be between 1 and 65535".to_string());
    }
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos();
    let connection = SshConnection {
        id: format!("ssh-{stamp:x}"),
        name: if input.name.trim().is_empty() { host.to_string() } else { input.name.trim().to_string() },
        host: host.to_string(),
        port: input.port,
        path: path.to_string(),
    };
    let mut connections = read_connections(app_handle)?;
    connections.push(connection.clone());
    write_connections(app_handle, &connections)?;
    Ok(connection)
}

pub fn remove_connection(app_handle: &tauri::AppHandle, connection_id: &str) -> Result<(), String> {
    let mut connections = read_connections(app_handle)?;
    connections.retain(|item| item.id != connection_id);
    write_connections(app_handle, &connections)
}

pub fn start_scan(app_handle: tauri::AppHandle, connection_id: String) -> Result<(), String> {
    let connection = read_connections(&app_handle)?.into_iter()
        .find(|item| item.id == connection_id)
        .ok_or_else(|| "SSH connection was not found".to_string())?;
    {
        let mut active = ACTIVE_SCANS.lock().unwrap_or_else(|item| item.into_inner());
        if !active.insert(connection_id.clone()) {
            return Ok(());
        }
    }
    app_handle.emit_all("ssh_scan_full", AccountPayload { account_id: connection_id.clone() }).ok();
    app_handle.emit_all("ssh_scan_status", ScanStatusPayload {
        account_id: connection_id.clone(), items: 0, total: 0,
        operation_not_permitted: 0, permission_denied: 0, interrupted: 0, other: 0,
    }).ok();

    tauri::async_runtime::spawn_blocking(move || {
        match scan_connection(&connection) {
            Ok(path) => {
                app_handle.emit_all(
                    "ssh_scan_finalizing",
                    AccountPayload { account_id: connection_id.clone() },
                ).ok();
                app_handle.emit_all(
                    "ssh_scan_completed",
                    CompletedPayload {
                        account_id: connection_id.clone(),
                        path: path.display().to_string(),
                        errors_path: String::new(),
                    },
                ).ok();
            }
            Err(message) => {
                app_handle.emit_all(
                    "ssh_scan_failed",
                    FailedPayload { account_id: connection_id.clone(), message },
                ).ok();
            }
        }
        ACTIVE_SCANS.lock().unwrap_or_else(|item| item.into_inner()).remove(&connection_id);
    });
    Ok(())
}

pub fn read_scan_result(path: &str) -> Result<String, String> {
    let path = PathBuf::from(path);
    if !path.starts_with(std::env::temp_dir())
        || !path.file_name().and_then(|name| name.to_str())
            .map(|name| name.starts_with("duckdisk-ssh-scan-"))
            .unwrap_or(false)
    {
        return Err("Refusing to read SSH result outside the temporary directory".to_string());
    }
    fs::read_to_string(path).map_err(|err| err.to_string())
}

fn scan_connection(connection: &SshConnection) -> Result<PathBuf, String> {
    let remote_script = r#"import json, os, stat, sys
root = os.path.abspath(sys.argv[1])
errors = 0
items = 0
def walk(path):
    global errors, items
    try:
        info = os.lstat(path)
    except OSError:
        errors += 1
        return None
    items += 1
    folder = stat.S_ISDIR(info.st_mode) and not stat.S_ISLNK(info.st_mode)
    node = {"name": os.path.basename(path) or path, "cloudId": path, "isDirectory": folder, "size": 0, "children": []}
    if not folder:
        node["size"] = int(info.st_size)
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
    return node
tree = walk(root)
if tree is None:
    raise SystemExit("Remote path could not be read")
tree["displayName"] = sys.argv[2]
print(json.dumps({"schema-version":"duckdisk-cloud-v1","unit":"bytes","tree":tree,"remoteErrors":errors,"remoteItems":items}, separators=(",",":")))"#;
    let remote_command = format!(
        "python3 -c {} {} {}",
        shell_quote(remote_script),
        shell_quote(&connection.path),
        shell_quote(&format!("{} ({})", connection.name, connection.host)),
    );
    let port = connection.port.to_string();
    let output = Command::new("ssh")
        .args([
            "-o", "BatchMode=yes",
            "-o", "ConnectTimeout=12",
            "-p", &port,
            &connection.host,
            &remote_command,
        ])
        .output()
        .map_err(|err| format!("Could not start ssh: {err}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if stderr.is_empty() {
            format!("SSH scan exited with {}", output.status)
        } else {
            format!("SSH scan failed: {stderr}")
        });
    }
    let content = String::from_utf8(output.stdout).map_err(|err| format!("Remote scan returned invalid text: {err}"))?;
    serde_json::from_str::<serde_json::Value>(&content)
        .map_err(|err| format!("Remote scan returned invalid JSON: {err}"))?;
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis();
    let path = std::env::temp_dir().join(format!("duckdisk-ssh-scan-{}-{stamp}.json", std::process::id()));
    fs::write(&path, content).map_err(|err| err.to_string())?;
    Ok(path)
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn connections_path(app_handle: &tauri::AppHandle) -> Result<PathBuf, String> {
    app_handle.path_resolver().app_config_dir()
        .map(|path| path.join("ssh-connections.json"))
        .ok_or_else(|| "Could not resolve DuckDisk configuration directory".to_string())
}

fn read_connections(app_handle: &tauri::AppHandle) -> Result<Vec<SshConnection>, String> {
    let path = connections_path(app_handle)?;
    if !path.exists() { return Ok(Vec::new()); }
    serde_json::from_str(&fs::read_to_string(path).map_err(|err| err.to_string())?).map_err(|err| err.to_string())
}

fn write_connections(app_handle: &tauri::AppHandle, connections: &[SshConnection]) -> Result<(), String> {
    let path = connections_path(app_handle)?;
    if let Some(parent) = path.parent() { fs::create_dir_all(parent).map_err(|err| err.to_string())?; }
    fs::write(path, serde_json::to_string_pretty(connections).map_err(|err| err.to_string())?).map_err(|err| err.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_quotes_remote_values() {
        assert_eq!(shell_quote("/tmp/a b"), "'/tmp/a b'");
        assert_eq!(shell_quote("a'b"), "'a'\"'\"'b'");
    }
}
