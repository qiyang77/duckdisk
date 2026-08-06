use std::collections::HashMap;
use std::fs;
use std::io::{ErrorKind, Read};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::{engine::general_purpose::STANDARD_NO_PAD, Engine as _};
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use ssh2::{HashType, Session};
use tauri::Manager;

static ACTIVE_SCANS: Lazy<Mutex<HashMap<String, Arc<AtomicBool>>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

const CONNECTIONS_FILE: &str = "ssh-connections-mas.json";
const PASSWORD_SERVICE: &str = "com.duckdisk.app.ssh.password";
const PRIVATE_KEY_SERVICE: &str = "com.duckdisk.app.ssh.private-key";
const KEY_PASSPHRASE_SERVICE: &str = "com.duckdisk.app.ssh.key-passphrase";
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
    #[serde(default)]
    pub trusted_host_key: String,
    #[serde(default)]
    pub has_private_key: bool,
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
    #[serde(default)]
    private_key_path: String,
    #[serde(default)]
    key_passphrase: String,
    #[serde(default)]
    trusted_host_key: String,
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

pub fn run_askpass_helper() -> Result<bool, String> {
    Ok(false)
}

pub fn get_connections(app_handle: &tauri::AppHandle) -> Result<Vec<SshConnection>, String> {
    read_connections(app_handle)
}

pub fn inspect_host_key(host: &str, port: u16) -> Result<String, String> {
    let (_, hostname) = split_user_and_host(host)?;
    let session = connect_session(hostname, port)?;
    host_fingerprint(&session)
}

pub fn get_storage_usage(
    app_handle: &tauri::AppHandle,
    connection_id: &str,
) -> Result<SshStorageUsage, String> {
    let connection = find_connection(app_handle, connection_id)?;
    let remote_script = r#"import json, os, sys
stats = os.statvfs(sys.argv[1])
block_size = stats.f_frsize or stats.f_bsize
total = max(0, int(stats.f_blocks) * int(block_size))
available = max(0, int(stats.f_bavail) * int(block_size))
print(json.dumps({"totalSpace":total,"usedSpace":max(0,total-available),"availableSpace":available}, separators=(",",":")))"#;
    let command = format!(
        "python3 -c {} {}",
        shell_quote(remote_script),
        shell_quote(&connection.path)
    );
    let output = execute_remote(&connection, &command)?;
    let usage: SshStorageUsage = serde_json::from_str(&output)
        .map_err(|err| format!("SSH storage query returned invalid JSON: {err}"))?;
    let mut connections = read_connections(app_handle)?;
    if let Some(item) = connections.iter_mut().find(|item| item.id == connection_id) {
        item.storage_usage = Some(usage.clone());
        write_connections(app_handle, &connections)?;
    }
    Ok(usage)
}

pub fn save_connection(
    app_handle: &tauri::AppHandle,
    input: SshConnectionInput,
) -> Result<SshConnection, String> {
    let host = input.host.trim();
    let path = input.path.trim();
    validate_connection_fields(host, input.port, path)?;
    let inspected_key = inspect_host_key(host, input.port)?;
    if input.trusted_host_key != inspected_key {
        return Err(format!(
            "The saved SSH host fingerprint does not match {host}:{}. Expected {inspected_key}",
            input.port
        ));
    }

    let mut connections = read_connections(app_handle)?;
    let existing_index = input
        .id
        .as_ref()
        .map(|id| {
            connections
                .iter()
                .position(|item| item.id == *id)
                .ok_or_else(|| "SSH connection was not found".to_string())
        })
        .transpose()?;
    let existing = existing_index.map(|index| connections[index].clone());
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let id = existing
        .as_ref()
        .map(|item| item.id.clone())
        .unwrap_or_else(|| format!("ssh-{stamp:x}"));

    let has_existing_password = existing
        .as_ref()
        .map(|item| item.auth_method == SshAuthMethod::Password)
        .unwrap_or(false);
    let has_existing_key = existing
        .as_ref()
        .map(|item| item.has_private_key)
        .unwrap_or(false);

    match input.auth_method {
        SshAuthMethod::Password => {
            if input.password.is_empty() && !has_existing_password {
                return Err("Enter the SSH password".to_string());
            }
            if !input.password.is_empty() {
                set_secret(PASSWORD_SERVICE, &id, &input.password)?;
            }
            delete_secret(PRIVATE_KEY_SERVICE, &id)?;
            delete_secret(KEY_PASSPHRASE_SERVICE, &id)?;
        }
        SshAuthMethod::Key => {
            if input.private_key_path.is_empty() && !has_existing_key {
                return Err("Choose an SSH private key".to_string());
            }
            if !input.private_key_path.is_empty() {
                let private_key = fs::read_to_string(&input.private_key_path)
                    .map_err(|err| format!("Could not read the selected private key: {err}"))?;
                if !private_key.contains("PRIVATE KEY") {
                    return Err("The selected file is not an SSH private key".to_string());
                }
                set_secret(PRIVATE_KEY_SERVICE, &id, &private_key)?;
                if input.key_passphrase.is_empty() {
                    delete_secret(KEY_PASSPHRASE_SERVICE, &id)?;
                } else {
                    set_secret(KEY_PASSPHRASE_SERVICE, &id, &input.key_passphrase)?;
                }
            } else if !input.key_passphrase.is_empty() {
                set_secret(KEY_PASSPHRASE_SERVICE, &id, &input.key_passphrase)?;
            }
            delete_secret(PASSWORD_SERVICE, &id)?;
        }
    }

    let connection = SshConnection {
        id: id.clone(),
        name: if input.name.trim().is_empty() {
            host.to_string()
        } else {
            input.name.trim().to_string()
        },
        host: host.to_string(),
        port: input.port,
        path: path.to_string(),
        auth_method: input.auth_method,
        trusted_host_key: inspected_key,
        has_private_key: input.auth_method == SshAuthMethod::Key,
        storage_usage: None,
    };
    if let Some(index) = existing_index {
        connections[index] = connection.clone();
    } else {
        connections.push(connection.clone());
    }
    write_connections(app_handle, &connections)?;
    clear_cached_result(app_handle, &id).ok();
    Ok(connection)
}

pub fn remove_connection(app_handle: &tauri::AppHandle, connection_id: &str) -> Result<(), String> {
    let mut connections = read_connections(app_handle)?;
    connections.retain(|item| item.id != connection_id);
    write_connections(app_handle, &connections)?;
    delete_secret(PASSWORD_SERVICE, connection_id)?;
    delete_secret(PRIVATE_KEY_SERVICE, connection_id)?;
    delete_secret(KEY_PASSPHRASE_SERVICE, connection_id)?;
    clear_cached_result(app_handle, connection_id)
}

pub fn start_scan(
    app_handle: tauri::AppHandle,
    connection_id: String,
    force_full: bool,
) -> Result<(), String> {
    let connection = find_connection(&app_handle, &connection_id)?;
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
    emit_scan_status(&app_handle, &connection_id, 0, 0);

    tauri::async_runtime::spawn_blocking(move || {
        let result = scan_connection(&app_handle, &connection, &cancelled);
        match result {
            Ok((path, content)) => {
                if let Err(message) = write_cached_result(&app_handle, &connection_id, &content) {
                    emit_scan_failure(&app_handle, &connection_id, message);
                } else {
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
            }
            Err(message) if message == SCAN_CANCELLED_MESSAGE => {}
            Err(message) => emit_scan_failure(&app_handle, &connection_id, message),
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
    let connection = find_connection(app_handle, connection_id)?;
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
        failures.append({"itemId":raw,"message":"Refusing to delete outside the configured remote path or delete its root"})
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
        failures.append({"itemId":raw,"message":str(error)})
print(json.dumps({"deletedIds":deleted,"failures":failures}, separators=(",",":")))"#;
    let command = format!(
        "python3 -c {} {} {}",
        shell_quote(remote_script),
        shell_quote(&connection.path),
        shell_quote(&serde_json::to_string(&item_ids).map_err(|err| err.to_string())?)
    );
    let output = execute_remote(&connection, &command)?;
    let result: SshDeleteResult = serde_json::from_str(&output)
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
    node = {"name":os.path.basename(path) or path,"cloudId":path,"isDirectory":folder,"size":0,"children":[]}
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
    let command = format!(
        "python3 -c {} {} {}",
        shell_quote(remote_script),
        shell_quote(&connection.path),
        shell_quote(&format!("{} ({})", connection.name, connection.host))
    );
    let content = execute_remote_streaming(app_handle, connection, &command, cancelled)?;
    serde_json::from_str::<serde_json::Value>(&content)
        .map_err(|err| format!("Remote scan returned invalid JSON: {err}"))?;
    let path = temporary_result_path();
    fs::write(&path, &content).map_err(|err| err.to_string())?;
    Ok((path, content))
}

fn connect_authenticated(connection: &SshConnection) -> Result<Session, String> {
    let (username, hostname) = split_user_and_host(&connection.host)?;
    let session = connect_session(hostname, connection.port)?;
    let actual_fingerprint = host_fingerprint(&session)?;
    if connection.trusted_host_key != actual_fingerprint {
        return Err(format!(
            "SSH host identity changed for {}. Expected {}, received {}. Edit the connection and verify the server before reconnecting.",
            connection.host, connection.trusted_host_key, actual_fingerprint
        ));
    }
    match connection.auth_method {
        SshAuthMethod::Password => {
            let password = read_required_secret(PASSWORD_SERVICE, &connection.id, "password")?;
            session
                .userauth_password(username, &password)
                .map_err(|err| format!("SSH password authentication failed: {err}"))?;
        }
        SshAuthMethod::Key => {
            let private_key =
                read_required_secret(PRIVATE_KEY_SERVICE, &connection.id, "private key")?;
            let passphrase = read_optional_secret(KEY_PASSPHRASE_SERVICE, &connection.id)?;
            session
                .userauth_pubkey_memory(username, None, &private_key, passphrase.as_deref())
                .map_err(|err| format!("SSH private-key authentication failed: {err}"))?;
        }
    }
    if !session.authenticated() {
        return Err("SSH authentication did not complete".to_string());
    }
    Ok(session)
}

fn connect_session(hostname: &str, port: u16) -> Result<Session, String> {
    let hostname = hostname
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
        .unwrap_or(hostname);
    let addresses = (hostname, port)
        .to_socket_addrs()
        .map_err(|err| format!("Could not resolve SSH host {hostname}: {err}"))?;
    let mut last_error = None;
    let mut tcp = None;
    for address in addresses {
        match TcpStream::connect_timeout(&address, Duration::from_secs(12)) {
            Ok(stream) => {
                tcp = Some(stream);
                break;
            }
            Err(error) => last_error = Some(error),
        }
    }
    let tcp = tcp.ok_or_else(|| {
        format!(
            "Could not connect to {hostname}:{port}: {}",
            last_error
                .map(|error| error.to_string())
                .unwrap_or_else(|| "no network address was available".to_string())
        )
    })?;
    tcp.set_read_timeout(Some(Duration::from_secs(30))).ok();
    tcp.set_write_timeout(Some(Duration::from_secs(30))).ok();
    let mut session =
        Session::new().map_err(|err| format!("Could not create SSH session: {err}"))?;
    session.set_tcp_stream(tcp);
    session
        .handshake()
        .map_err(|err| format!("SSH handshake failed: {err}"))?;
    Ok(session)
}

fn execute_remote(connection: &SshConnection, command: &str) -> Result<String, String> {
    let session = connect_authenticated(connection)?;
    let mut channel = session
        .channel_session()
        .map_err(|err| format!("Could not open SSH command channel: {err}"))?;
    channel
        .exec(command)
        .map_err(|err| format!("Could not start remote command: {err}"))?;
    let mut stdout = String::new();
    channel
        .read_to_string(&mut stdout)
        .map_err(|err| format!("Could not read SSH output: {err}"))?;
    let mut stderr = String::new();
    channel
        .stderr()
        .read_to_string(&mut stderr)
        .map_err(|err| format!("Could not read SSH errors: {err}"))?;
    channel
        .wait_close()
        .map_err(|err| format!("Could not finish SSH command: {err}"))?;
    let status = channel
        .exit_status()
        .map_err(|err| format!("Could not read SSH command status: {err}"))?;
    if status != 0 {
        return Err(if stderr.trim().is_empty() {
            format!("SSH command exited with status {status}")
        } else {
            format!("SSH command failed: {}", stderr.trim())
        });
    }
    Ok(stdout)
}

fn execute_remote_streaming(
    app_handle: &tauri::AppHandle,
    connection: &SshConnection,
    command: &str,
    cancelled: &Arc<AtomicBool>,
) -> Result<String, String> {
    let session = connect_authenticated(connection)?;
    let mut channel = session
        .channel_session()
        .map_err(|err| format!("Could not open SSH scan channel: {err}"))?;
    channel
        .exec(command)
        .map_err(|err| format!("Could not start remote scan: {err}"))?;
    session.set_blocking(false);
    let mut stderr_stream = channel.stderr();
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let mut stdout_buffer = [0_u8; 64 * 1024];
    let mut stderr_buffer = [0_u8; 8 * 1024];
    let mut progress_tail = String::new();

    loop {
        if cancelled.load(Ordering::SeqCst) {
            channel.close().ok();
            return Err(SCAN_CANCELLED_MESSAGE.to_string());
        }
        let mut made_progress = false;
        match channel.read(&mut stdout_buffer) {
            Ok(0) => {}
            Ok(count) => {
                stdout.extend_from_slice(&stdout_buffer[..count]);
                made_progress = true;
            }
            Err(error) if error.kind() == ErrorKind::WouldBlock => {}
            Err(error) => return Err(format!("Could not read SSH scan output: {error}")),
        }
        match stderr_stream.read(&mut stderr_buffer) {
            Ok(0) => {}
            Ok(count) => {
                let chunk = String::from_utf8_lossy(&stderr_buffer[..count]);
                progress_tail.push_str(&chunk);
                while let Some(newline) = progress_tail.find('\n') {
                    let line = progress_tail[..newline].to_string();
                    progress_tail.drain(..=newline);
                    if let Some((items, total)) = parse_progress(&line) {
                        emit_scan_status(app_handle, &connection.id, items, total);
                    } else if !line.trim().is_empty() {
                        stderr.extend_from_slice(line.as_bytes());
                        stderr.push(b'\n');
                    }
                }
                made_progress = true;
            }
            Err(error) if error.kind() == ErrorKind::WouldBlock => {}
            Err(error) => return Err(format!("Could not read SSH scan errors: {error}")),
        }
        if channel.eof() {
            break;
        }
        if !made_progress {
            std::thread::sleep(Duration::from_millis(25));
        }
    }
    if !progress_tail.trim().is_empty() {
        if let Some((items, total)) = parse_progress(progress_tail.trim()) {
            emit_scan_status(app_handle, &connection.id, items, total);
        } else {
            stderr.extend_from_slice(progress_tail.as_bytes());
        }
    }
    session.set_blocking(true);
    channel
        .wait_close()
        .map_err(|err| format!("Could not finish SSH scan: {err}"))?;
    let status = channel
        .exit_status()
        .map_err(|err| format!("Could not read SSH scan status: {err}"))?;
    let stderr = String::from_utf8_lossy(&stderr).trim().to_string();
    if status != 0 {
        return Err(if stderr.is_empty() {
            format!("SSH scan exited with status {status}")
        } else {
            format!("SSH scan failed: {stderr}")
        });
    }
    String::from_utf8(stdout).map_err(|err| format!("SSH scan returned invalid UTF-8: {err}"))
}

fn host_fingerprint(session: &Session) -> Result<String, String> {
    let hash = session
        .host_key_hash(HashType::Sha256)
        .ok_or_else(|| "SSH server did not provide a host-key fingerprint".to_string())?;
    Ok(format!("SHA256:{}", STANDARD_NO_PAD.encode(hash)))
}

fn split_user_and_host(value: &str) -> Result<(&str, &str), String> {
    let value = value.trim();
    if value.is_empty() || value.starts_with('-') || value.chars().any(char::is_whitespace) {
        return Err("Enter an SSH host as user@example.com".to_string());
    }
    match value.rsplit_once('@') {
        Some((username, hostname)) if !username.is_empty() && !hostname.is_empty() => {
            Ok((username, hostname))
        }
        _ => Err(
            "The Mac App Store SSH client requires a host in user@example.com format".to_string(),
        ),
    }
}

fn validate_connection_fields(host: &str, port: u16, path: &str) -> Result<(), String> {
    split_user_and_host(host)?;
    if port == 0 {
        return Err("SSH port must be between 1 and 65535".to_string());
    }
    if path.is_empty() || !path.starts_with('/') {
        return Err("Remote path must be an absolute path".to_string());
    }
    if path.contains(['\0', '\n', '\r']) {
        return Err("Remote path contains unsupported control characters".to_string());
    }
    Ok(())
}

fn parse_progress(line: &str) -> Option<(u64, u64)> {
    let values = line.strip_prefix(PROGRESS_PREFIX)?;
    let mut parts = values.split('\t');
    Some((parts.next()?.parse().ok()?, parts.next()?.parse().ok()?))
}

fn emit_scan_status(app_handle: &tauri::AppHandle, connection_id: &str, items: u64, total: u64) {
    app_handle
        .emit_all(
            "ssh_scan_status",
            ScanStatusPayload {
                account_id: connection_id.to_string(),
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

fn emit_scan_failure(app_handle: &tauri::AppHandle, connection_id: &str, message: String) {
    app_handle
        .emit_all(
            "ssh_scan_failed",
            FailedPayload {
                account_id: connection_id.to_string(),
                message,
            },
        )
        .ok();
}

fn find_connection(
    app_handle: &tauri::AppHandle,
    connection_id: &str,
) -> Result<SshConnection, String> {
    read_connections(app_handle)?
        .into_iter()
        .find(|item| item.id == connection_id)
        .ok_or_else(|| "SSH connection was not found".to_string())
}

fn connections_path(app_handle: &tauri::AppHandle) -> Result<PathBuf, String> {
    app_handle
        .path_resolver()
        .app_config_dir()
        .map(|path| path.join(CONNECTIONS_FILE))
        .ok_or_else(|| "Could not resolve DuckDisk configuration directory".to_string())
}

fn read_connections(app_handle: &tauri::AppHandle) -> Result<Vec<SshConnection>, String> {
    let path = connections_path(app_handle)?;
    if !path.exists() {
        return Ok(Vec::new());
    }
    let content = fs::read_to_string(path).map_err(|err| err.to_string())?;
    serde_json::from_str(&content).map_err(|err| format!("Invalid SSH connection settings: {err}"))
}

fn write_connections(
    app_handle: &tauri::AppHandle,
    connections: &[SshConnection],
) -> Result<(), String> {
    let path = connections_path(app_handle)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| err.to_string())?;
    }
    let content = serde_json::to_vec_pretty(connections).map_err(|err| err.to_string())?;
    fs::write(path, content).map_err(|err| err.to_string())
}

fn cache_path(app_handle: &tauri::AppHandle, connection_id: &str) -> Result<PathBuf, String> {
    let directory = app_handle
        .path_resolver()
        .app_cache_dir()
        .ok_or_else(|| "Could not resolve DuckDisk cache directory".to_string())?
        .join("ssh");
    fs::create_dir_all(&directory).map_err(|err| err.to_string())?;
    Ok(directory.join(format!("{connection_id}.json")))
}

fn write_cached_result(
    app_handle: &tauri::AppHandle,
    connection_id: &str,
    content: &str,
) -> Result<(), String> {
    fs::write(cache_path(app_handle, connection_id)?, content).map_err(|err| err.to_string())
}

fn copy_cached_result_to_temp(
    app_handle: &tauri::AppHandle,
    connection_id: &str,
) -> Result<Option<PathBuf>, String> {
    let path = cache_path(app_handle, connection_id)?;
    if !path.exists() {
        return Ok(None);
    }
    let content = fs::read_to_string(path).map_err(|err| err.to_string())?;
    if serde_json::from_str::<serde_json::Value>(&content).is_err() {
        clear_cached_result(app_handle, connection_id)?;
        return Ok(None);
    }
    let result = temporary_result_path();
    fs::write(&result, content).map_err(|err| err.to_string())?;
    Ok(Some(result))
}

fn temporary_result_path() -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "duckdisk-ssh-scan-{}-{stamp}.json",
        std::process::id()
    ))
}

fn set_secret(service: &str, account: &str, secret: &str) -> Result<(), String> {
    keyring::Entry::new(service, account)
        .map_err(|err| err.to_string())?
        .set_password(secret)
        .map_err(|err| format!("Could not save SSH credential in macOS Keychain: {err}"))
}

fn read_required_secret(service: &str, account: &str, label: &str) -> Result<String, String> {
    keyring::Entry::new(service, account)
        .map_err(|err| err.to_string())?
        .get_password()
        .map_err(|err| format!("Could not read SSH {label} from macOS Keychain: {err}"))
}

fn read_optional_secret(service: &str, account: &str) -> Result<Option<String>, String> {
    let entry = keyring::Entry::new(service, account).map_err(|err| err.to_string())?;
    match entry.get_password() {
        Ok(value) => Ok(Some(value)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(error) => Err(format!(
            "Could not read SSH credential from macOS Keychain: {error}"
        )),
    }
}

fn delete_secret(service: &str, account: &str) -> Result<(), String> {
    let entry = keyring::Entry::new(service, account).map_err(|err| err.to_string())?;
    match entry.delete_password() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(error) => Err(format!(
            "Could not remove SSH credential from macOS Keychain: {error}"
        )),
    }
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_required_user_and_host() {
        assert_eq!(
            split_user_and_host("alice@example.com").unwrap(),
            ("alice", "example.com")
        );
        assert_eq!(
            split_user_and_host("root@[::1]").unwrap(),
            ("root", "[::1]")
        );
        assert!(split_user_and_host("example.com").is_err());
    }

    #[test]
    fn quotes_remote_values() {
        assert_eq!(shell_quote("/tmp/a b"), "'/tmp/a b'");
        assert_eq!(shell_quote("a'b"), "'a'\"'\"'b'");
    }
}
