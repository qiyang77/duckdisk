use std::collections::{hash_map::DefaultHasher, HashMap, HashSet};
use std::fs;
use std::hash::{Hash, Hasher};
use std::io::{ErrorKind, Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use oauth2::basic::BasicClient;
use oauth2::reqwest::async_http_client;
use oauth2::{
    AuthType, AuthUrl, AuthorizationCode, ClientId, CsrfToken, PkceCodeChallenge, RedirectUrl,
    RefreshToken, Scope, TokenResponse, TokenUrl,
};
use once_cell::sync::Lazy;
use reqwest::Client;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::Manager;
use url::Url;

const API_ROOT: &str = "https://www.googleapis.com/drive/v3";
const KEYCHAIN_SERVICE: &str = "com.duckdisk.dev.googledrive";
const CACHE_VERSION: &str = "duckdisk-google-drive-cache-v1";
const CALLBACK_TIMEOUT: Duration = Duration::from_secs(300);

static ACTIVE_SCANS: Lazy<Mutex<HashSet<String>>> = Lazy::new(|| Mutex::new(HashSet::new()));

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GoogleDriveAccount {
    pub id: String,
    pub name: String,
    pub email: String,
    pub total_space: u64,
    pub used_space: u64,
    pub available_space: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GoogleDriveState {
    configured: bool,
    accounts: Vec<GoogleDriveAccount>,
}

#[derive(Serialize, Deserialize)]
struct StoredCredential {
    refresh_token: String,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CachedItem {
    id: String,
    parent_id: Option<String>,
    name: String,
    size: u64,
    is_folder: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GoogleDriveCache {
    version: String,
    account_id: String,
    root_id: String,
    page_token: String,
    items: HashMap<String, CachedItem>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AboutResponse {
    user: GoogleUser,
    storage_quota: StorageQuota,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GoogleUser {
    display_name: String,
    email_address: String,
    permission_id: String,
}

#[derive(Default, Deserialize)]
struct StorageQuota {
    limit: Option<String>,
    #[serde(rename = "usageInDrive")]
    usage_in_drive: Option<String>,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DriveFile {
    id: String,
    name: Option<String>,
    mime_type: Option<String>,
    size: Option<String>,
    quota_bytes_used: Option<String>,
    parents: Option<Vec<String>>,
    trashed: Option<bool>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FilePage {
    next_page_token: Option<String>,
    files: Vec<DriveFile>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct StartPageToken {
    start_page_token: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChangePage {
    next_page_token: Option<String>,
    new_start_page_token: Option<String>,
    changes: Vec<DriveChange>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DriveChange {
    file_id: String,
    removed: Option<bool>,
    file: Option<DriveFile>,
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

pub fn get_state(app_handle: &tauri::AppHandle) -> Result<GoogleDriveState, String> {
    Ok(GoogleDriveState {
        configured: !client_id().is_empty(),
        accounts: read_accounts(app_handle)?,
    })
}

pub async fn connect_account(app_handle: &tauri::AppHandle) -> Result<GoogleDriveAccount, String> {
    let client_id = required_client_id()?;
    let listener = TcpListener::bind("127.0.0.1:0").map_err(|err| err.to_string())?;
    let port = listener.local_addr().map_err(|err| err.to_string())?.port();
    let redirect = format!("http://127.0.0.1:{port}");
    let oauth_client = oauth_client(&client_id, Some(&redirect))?;
    let (pkce_challenge, pkce_verifier) = PkceCodeChallenge::new_random_sha256();
    let (authorize_url, csrf_token) = oauth_client
        .authorize_url(CsrfToken::new_random)
        .set_pkce_challenge(pkce_challenge)
        .add_scope(Scope::new(
            "https://www.googleapis.com/auth/drive.metadata.readonly".to_string(),
        ))
        .add_extra_param("access_type", "offline")
        .add_extra_param("prompt", "consent select_account")
        .url();

    Command::new("open")
        .arg(authorize_url.as_str())
        .spawn()
        .map_err(|err| format!("Could not open Google sign-in: {err}"))?;

    let expected_state = csrf_token.secret().to_string();
    let code = tauri::async_runtime::spawn_blocking(move || {
        wait_for_oauth_callback(listener, &expected_state)
    })
    .await
    .map_err(|err| err.to_string())??;

    let token = oauth_client
        .exchange_code(AuthorizationCode::new(code))
        .set_pkce_verifier(pkce_verifier)
        .request_async(async_http_client)
        .await
        .map_err(|err| format!("Google sign-in failed: {err}"))?;
    let refresh_token = token
        .refresh_token()
        .ok_or_else(|| "Google did not return a refresh token".to_string())?
        .secret()
        .to_string();

    let http = Client::new();
    let about = fetch_about(&http, token.access_token().secret()).await?;
    let account = account_from_about(&about);
    store_credential(&account.id, &refresh_token)?;
    upsert_account(app_handle, account.clone())?;
    Ok(account)
}

pub fn disconnect_account(app_handle: &tauri::AppHandle, account_id: &str) -> Result<(), String> {
    let entry = keyring::Entry::new(KEYCHAIN_SERVICE, account_id).map_err(|err| err.to_string())?;
    match entry.delete_password() {
        Ok(()) | Err(keyring::Error::NoEntry) => {}
        Err(err) => return Err(err.to_string()),
    }
    let mut accounts = read_accounts(app_handle)?;
    accounts.retain(|account| account.id != account_id);
    write_accounts(app_handle, &accounts)?;
    let path = cache_path(app_handle, account_id)?;
    if path.exists() {
        fs::remove_file(path).map_err(|err| err.to_string())?;
    }
    Ok(())
}

pub fn start_scan(
    app_handle: tauri::AppHandle,
    account_id: String,
    force_full: bool,
) -> Result<(), String> {
    if !read_accounts(&app_handle)?.iter().any(|item| item.id == account_id) {
        return Err("Google Drive account is not connected".to_string());
    }
    {
        let mut active = ACTIVE_SCANS.lock().unwrap_or_else(|item| item.into_inner());
        if !active.insert(account_id.clone()) {
            return Ok(());
        }
    }
    tauri::async_runtime::spawn(async move {
        match scan_account(&app_handle, &account_id, force_full).await {
            Ok(path) => {
                app_handle.emit_all(
                    "googledrive_scan_completed",
                    CompletedPayload {
                        account_id: account_id.clone(),
                        path: path.display().to_string(),
                        errors_path: String::new(),
                    },
                ).ok();
            }
            Err(message) => {
                app_handle.emit_all(
                    "googledrive_scan_failed",
                    FailedPayload { account_id: account_id.clone(), message },
                ).ok();
            }
        }
        ACTIVE_SCANS.lock().unwrap_or_else(|item| item.into_inner()).remove(&account_id);
    });
    Ok(())
}

pub fn read_scan_result(path: &str) -> Result<String, String> {
    let path = PathBuf::from(path);
    if !path.starts_with(std::env::temp_dir())
        || !path.file_name().and_then(|name| name.to_str())
            .map(|name| name.starts_with("duckdisk-google-drive-scan-"))
            .unwrap_or(false)
    {
        return Err("Refusing to read Google Drive result outside the temporary directory".to_string());
    }
    fs::read_to_string(path).map_err(|err| err.to_string())
}

async fn scan_account(
    app_handle: &tauri::AppHandle,
    account_id: &str,
    force_full: bool,
) -> Result<PathBuf, String> {
    let access_token = refresh_access_token(account_id).await?;
    let http = Client::new();
    let about = fetch_about(&http, &access_token).await?;
    upsert_account(app_handle, account_from_about(&about))?;
    let path = cache_path(app_handle, account_id)?;
    let existing = (!force_full).then(|| read_cache(&path, account_id)).flatten();
    let cache = match existing {
        Some(cache) => match incremental_scan(app_handle, &http, &access_token, cache).await {
            Ok(cache) => cache,
            Err(message) if message.contains("410 Gone") => {
                full_scan(app_handle, &http, &access_token, account_id).await?
            }
            Err(message) => return Err(message),
        },
        None => full_scan(app_handle, &http, &access_token, account_id).await?,
    };
    write_cache(&path, &cache)?;
    app_handle.emit_all(
        "googledrive_scan_finalizing",
        AccountPayload { account_id: account_id.to_string() },
    ).ok();
    let account = read_accounts(app_handle)?.into_iter()
        .find(|account| account.id == account_id)
        .ok_or_else(|| "Google Drive account metadata is missing".to_string())?;
    write_scan_result(&build_scan_json(&cache, &account)?)
}

async fn full_scan(
    app_handle: &tauri::AppHandle,
    http: &Client,
    access_token: &str,
    account_id: &str,
) -> Result<GoogleDriveCache, String> {
    app_handle.emit_all(
        "googledrive_scan_full",
        AccountPayload { account_id: account_id.to_string() },
    ).ok();
    let root: DriveFile = google_get(
        http,
        access_token,
        &format!("{API_ROOT}/files/root?fields=id,name,mimeType"),
    ).await?;
    let root_id = root.id.clone();
    let mut items = HashMap::new();
    items.insert(root_id.clone(), cached_item(root));
    let mut page_token: Option<String> = None;
    let mut count = 0_u64;
    loop {
        let mut request = http.get(&format!("{API_ROOT}/files"))
            .bearer_auth(access_token)
            .query(&[
                ("spaces", "drive"),
                ("pageSize", "1000"),
                ("q", "trashed = false"),
                ("fields", "nextPageToken,files(id,name,mimeType,size,quotaBytesUsed,parents,trashed)"),
            ]);
        if let Some(token) = &page_token {
            request = request.query(&[("pageToken", token)]);
        }
        let page: FilePage = google_response(request.send().await).await?;
        for file in page.files {
            if file.id != root_id {
                items.insert(file.id.clone(), cached_item(file));
                count += 1;
            }
        }
        emit_status(app_handle, account_id, count);
        page_token = page.next_page_token;
        if page_token.is_none() { break; }
    }
    let token: StartPageToken = google_get(
        http,
        access_token,
        &format!("{API_ROOT}/changes/startPageToken"),
    ).await?;
    Ok(GoogleDriveCache {
        version: CACHE_VERSION.to_string(),
        account_id: account_id.to_string(),
        root_id,
        page_token: token.start_page_token,
        items,
    })
}

async fn incremental_scan(
    app_handle: &tauri::AppHandle,
    http: &Client,
    access_token: &str,
    mut cache: GoogleDriveCache,
) -> Result<GoogleDriveCache, String> {
    app_handle.emit_all(
        "googledrive_scan_incremental",
        AccountPayload { account_id: cache.account_id.clone() },
    ).ok();
    let mut token = cache.page_token.clone();
    let mut count = 0_u64;
    loop {
        let request = http.get(&format!("{API_ROOT}/changes"))
            .bearer_auth(access_token)
            .query(&[
                ("pageToken", token.as_str()),
                ("pageSize", "1000"),
                ("spaces", "drive"),
                ("fields", "nextPageToken,newStartPageToken,changes(fileId,removed,file(id,name,mimeType,size,quotaBytesUsed,parents,trashed))"),
            ]);
        let page: ChangePage = google_response(request.send().await).await?;
        for change in page.changes {
            if change.removed.unwrap_or(false) {
                cache.items.remove(&change.file_id);
            } else if let Some(file) = change.file {
                if file.trashed.unwrap_or(false) {
                    cache.items.remove(&file.id);
                } else {
                    cache.items.insert(file.id.clone(), cached_item(file));
                }
            }
            count += 1;
        }
        emit_status(app_handle, &cache.account_id, count);
        if let Some(next) = page.next_page_token {
            token = next;
            continue;
        }
        if let Some(start) = page.new_start_page_token {
            cache.page_token = start;
        }
        break;
    }
    Ok(cache)
}

fn cached_item(file: DriveFile) -> CachedItem {
    CachedItem {
        id: file.id,
        parent_id: file.parents.and_then(|items| items.into_iter().next()),
        name: file.name.unwrap_or_else(|| "(unnamed)".to_string()),
        size: file.size.or(file.quota_bytes_used)
            .and_then(|size| size.parse().ok()).unwrap_or_default(),
        is_folder: file.mime_type.as_deref() == Some("application/vnd.google-apps.folder"),
    }
}

fn build_scan_json(cache: &GoogleDriveCache, account: &GoogleDriveAccount) -> Result<String, String> {
    let mut child_ids: HashMap<String, Vec<String>> = HashMap::new();
    for item in cache.items.values() {
        if item.id == cache.root_id { continue; }
        let parent = item.parent_id.as_ref()
            .filter(|parent| cache.items.contains_key(*parent))
            .cloned()
            .unwrap_or_else(|| cache.root_id.clone());
        child_ids.entry(parent).or_default().push(item.id.clone());
    }
    let mut visiting = HashSet::new();
    let mut children = child_ids.get(&cache.root_id).cloned().unwrap_or_default()
        .into_iter()
        .filter_map(|id| build_node(&id, cache, &child_ids, &mut visiting))
        .collect::<Vec<_>>();
    children.sort_by_key(|node| std::cmp::Reverse(node["size"].as_u64().unwrap_or_default()));
    let size = children.iter().map(|node| node["size"].as_u64().unwrap_or_default()).sum::<u64>();
    serde_json::to_string(&json!({
        "schema-version": "duckdisk-cloud-v1",
        "unit": "bytes",
        "tree": {
            "name": "(total)",
            "displayName": format!("Google Drive - {}", account.name),
            "cloudId": cache.root_id,
            "isDirectory": true,
            "size": size,
            "children": children
        }
    })).map_err(|err| err.to_string())
}

fn build_node(
    id: &str,
    cache: &GoogleDriveCache,
    child_ids: &HashMap<String, Vec<String>>,
    visiting: &mut HashSet<String>,
) -> Option<Value> {
    if !visiting.insert(id.to_string()) { return None; }
    let item = cache.items.get(id)?;
    let mut children = if item.is_folder {
        child_ids.get(id).cloned().unwrap_or_default().into_iter()
            .filter_map(|child| build_node(&child, cache, child_ids, visiting))
            .collect::<Vec<_>>()
    } else { Vec::new() };
    children.sort_by_key(|node| std::cmp::Reverse(node["size"].as_u64().unwrap_or_default()));
    visiting.remove(id);
    let size = if item.is_folder {
        children.iter().map(|node| node["size"].as_u64().unwrap_or_default()).sum()
    } else { item.size };
    Some(json!({
        "name": item.name,
        "cloudId": item.id,
        "isDirectory": item.is_folder,
        "size": size,
        "children": children
    }))
}

async fn fetch_about(http: &Client, access_token: &str) -> Result<AboutResponse, String> {
    google_get(
        http,
        access_token,
        &format!("{API_ROOT}/about?fields=user(displayName,emailAddress,permissionId),storageQuota(limit,usageInDrive)"),
    ).await
}

async fn google_get<T: DeserializeOwned>(http: &Client, token: &str, url: &str) -> Result<T, String> {
    google_response(http.get(url).bearer_auth(token).send().await).await
}

async fn google_response<T: DeserializeOwned>(response: Result<reqwest::Response, reqwest::Error>) -> Result<T, String> {
    let response = response.map_err(|err| format!("Could not reach Google Drive: {err}"))?;
    let status = response.status();
    let body = response.text().await.map_err(|err| err.to_string())?;
    if !status.is_success() {
        return Err(format!("Google Drive returned {status}: {body}"));
    }
    serde_json::from_str(&body).map_err(|err| format!("Invalid Google Drive response: {err}"))
}

async fn refresh_access_token(account_id: &str) -> Result<String, String> {
    let credential = read_credential(account_id)?;
    let token = oauth_client(&required_client_id()?, None)?
        .exchange_refresh_token(&RefreshToken::new(credential.refresh_token))
        .request_async(async_http_client)
        .await
        .map_err(|err| format!("Could not refresh Google sign-in: {err}"))?;
    Ok(token.access_token().secret().to_string())
}

fn oauth_client(client_id: &str, redirect: Option<&str>) -> Result<BasicClient, String> {
    let client = BasicClient::new(
        ClientId::new(client_id.to_string()),
        None,
        AuthUrl::new("https://accounts.google.com/o/oauth2/v2/auth".to_string()).map_err(|err| err.to_string())?,
        Some(TokenUrl::new("https://oauth2.googleapis.com/token".to_string()).map_err(|err| err.to_string())?),
    ).set_auth_type(AuthType::RequestBody);
    match redirect {
        Some(uri) => Ok(client.set_redirect_uri(RedirectUrl::new(uri.to_string()).map_err(|err| err.to_string())?)),
        None => Ok(client),
    }
}

fn client_id() -> String { env!("DUCKDISK_GOOGLE_CLIENT_ID").trim().to_string() }

fn required_client_id() -> Result<String, String> {
    let value = client_id();
    if value.is_empty() {
        Err("Google Drive is not configured in this build. Set DUCKDISK_GOOGLE_CLIENT_ID and rebuild.".to_string())
    } else { Ok(value) }
}

fn account_from_about(about: &AboutResponse) -> GoogleDriveAccount {
    let total = about.storage_quota.limit.as_ref().and_then(|value| value.parse().ok()).unwrap_or_default();
    let used = about.storage_quota.usage_in_drive.as_ref().and_then(|value| value.parse().ok()).unwrap_or_default();
    GoogleDriveAccount {
        id: about.user.permission_id.clone(),
        name: about.user.display_name.clone(),
        email: about.user.email_address.clone(),
        total_space: total,
        used_space: used,
        available_space: total.saturating_sub(used),
    }
}

fn store_credential(account_id: &str, refresh_token: &str) -> Result<(), String> {
    let entry = keyring::Entry::new(KEYCHAIN_SERVICE, account_id).map_err(|err| err.to_string())?;
    let value = serde_json::to_string(&StoredCredential { refresh_token: refresh_token.to_string() }).map_err(|err| err.to_string())?;
    entry.set_password(&value).map_err(|err| err.to_string())
}

fn read_credential(account_id: &str) -> Result<StoredCredential, String> {
    let entry = keyring::Entry::new(KEYCHAIN_SERVICE, account_id).map_err(|err| err.to_string())?;
    let value = entry.get_password().map_err(|err| format!("Google sign-in is missing from Keychain: {err}"))?;
    serde_json::from_str(&value).map_err(|err| err.to_string())
}

fn accounts_path(app_handle: &tauri::AppHandle) -> Result<PathBuf, String> {
    app_handle.path_resolver().app_config_dir()
        .map(|path| path.join("google-drive-accounts.json"))
        .ok_or_else(|| "Could not resolve DuckDisk configuration directory".to_string())
}

fn read_accounts(app_handle: &tauri::AppHandle) -> Result<Vec<GoogleDriveAccount>, String> {
    let path = accounts_path(app_handle)?;
    if !path.exists() { return Ok(Vec::new()); }
    serde_json::from_str(&fs::read_to_string(path).map_err(|err| err.to_string())?).map_err(|err| err.to_string())
}

fn write_accounts(app_handle: &tauri::AppHandle, accounts: &[GoogleDriveAccount]) -> Result<(), String> {
    let path = accounts_path(app_handle)?;
    if let Some(parent) = path.parent() { fs::create_dir_all(parent).map_err(|err| err.to_string())?; }
    fs::write(path, serde_json::to_string_pretty(accounts).map_err(|err| err.to_string())?).map_err(|err| err.to_string())
}

fn upsert_account(app_handle: &tauri::AppHandle, account: GoogleDriveAccount) -> Result<(), String> {
    let mut accounts = read_accounts(app_handle)?;
    if let Some(existing) = accounts.iter_mut().find(|item| item.id == account.id) {
        *existing = account;
    } else { accounts.push(account); }
    write_accounts(app_handle, &accounts)
}

fn cache_path(app_handle: &tauri::AppHandle, account_id: &str) -> Result<PathBuf, String> {
    let mut hasher = DefaultHasher::new();
    account_id.hash(&mut hasher);
    app_handle.path_resolver().app_cache_dir()
        .map(|path| path.join("google-drive").join(format!("{:016x}.json", hasher.finish())))
        .ok_or_else(|| "Could not resolve DuckDisk cache directory".to_string())
}

fn read_cache(path: &Path, account_id: &str) -> Option<GoogleDriveCache> {
    let cache: GoogleDriveCache = serde_json::from_str(&fs::read_to_string(path).ok()?).ok()?;
    (cache.version == CACHE_VERSION && cache.account_id == account_id && !cache.page_token.is_empty()).then_some(cache)
}

fn write_cache(path: &Path, cache: &GoogleDriveCache) -> Result<(), String> {
    if let Some(parent) = path.parent() { fs::create_dir_all(parent).map_err(|err| err.to_string())?; }
    let temporary = path.with_extension("json.tmp");
    fs::write(&temporary, serde_json::to_string(cache).map_err(|err| err.to_string())?).map_err(|err| err.to_string())?;
    fs::rename(temporary, path).map_err(|err| err.to_string())
}

fn write_scan_result(content: &str) -> Result<PathBuf, String> {
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis();
    let path = std::env::temp_dir().join(format!("duckdisk-google-drive-scan-{}-{stamp}.json", std::process::id()));
    fs::write(&path, content).map_err(|err| err.to_string())?;
    Ok(path)
}

fn emit_status(app_handle: &tauri::AppHandle, account_id: &str, items: u64) {
    app_handle.emit_all("googledrive_scan_status", ScanStatusPayload {
        account_id: account_id.to_string(), items, total: 0,
        operation_not_permitted: 0, permission_denied: 0, interrupted: 0, other: 0,
    }).ok();
}

fn wait_for_oauth_callback(listener: TcpListener, expected_state: &str) -> Result<String, String> {
    listener.set_nonblocking(true).map_err(|err| err.to_string())?;
    let started = Instant::now();
    while started.elapsed() < CALLBACK_TIMEOUT {
        match listener.accept() {
            Ok((mut stream, _)) => {
                let mut buffer = [0_u8; 8192];
                let count = stream.read(&mut buffer).map_err(|err| err.to_string())?;
                let request = String::from_utf8_lossy(&buffer[..count]);
                let target = request.split_whitespace().nth(1).ok_or_else(|| "Invalid Google callback".to_string())?;
                let url = Url::parse(&format!("http://127.0.0.1{target}")).map_err(|err| err.to_string())?;
                let params = url.query_pairs().into_owned().collect::<HashMap<_, _>>();
                if params.get("state").map(String::as_str) != Some(expected_state) {
                    return Err("Google sign-in state did not match".to_string());
                }
                if let Some(error) = params.get("error") {
                    write_callback_response(&mut stream, "Google Drive connection cancelled")?;
                    return Err(format!("Google sign-in failed: {error}"));
                }
                let code = params.get("code").cloned().ok_or_else(|| "Google callback did not include a code".to_string())?;
                write_callback_response(&mut stream, "Google Drive connected")?;
                return Ok(code);
            }
            Err(err) if err.kind() == ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(100)),
            Err(err) => return Err(err.to_string()),
        }
    }
    Err("Google sign-in timed out".to_string())
}

fn write_callback_response(stream: &mut std::net::TcpStream, title: &str) -> Result<(), String> {
    let body = format!("<!doctype html><meta charset=\"utf-8\"><title>{title}</title><style>body{{font:16px system-ui;background:#15181c;color:#eef1f3;display:grid;place-items:center;height:100vh;margin:0}}</style><h1>{title}</h1>");
    let response = format!("HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body);
    stream.write_all(response.as_bytes()).map_err(|err| err.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: &str, parent_id: Option<&str>, name: &str, size: u64, folder: bool) -> CachedItem {
        CachedItem {
            id: id.to_string(),
            parent_id: parent_id.map(str::to_string),
            name: name.to_string(),
            size,
            is_folder: folder,
        }
    }

    #[test]
    fn builds_google_drive_tree_and_attaches_orphans() {
        let mut items = HashMap::new();
        items.insert("root".to_string(), item("root", None, "My Drive", 0, true));
        items.insert("folder".to_string(), item("folder", Some("root"), "Work", 0, true));
        items.insert("file".to_string(), item("file", Some("folder"), "notes.txt", 42, false));
        items.insert("orphan".to_string(), item("orphan", Some("missing"), "shared.pdf", 8, false));
        let cache = GoogleDriveCache {
            version: CACHE_VERSION.to_string(),
            account_id: "account".to_string(),
            root_id: "root".to_string(),
            page_token: "token".to_string(),
            items,
        };
        let account = GoogleDriveAccount {
            id: "account".to_string(),
            name: "Qi Yang".to_string(),
            email: "qi@example.com".to_string(),
            total_space: 100,
            used_space: 50,
            available_space: 50,
        };
        let result: Value = serde_json::from_str(&build_scan_json(&cache, &account).unwrap()).unwrap();
        assert_eq!(result["tree"]["size"], 50);
        assert_eq!(result["tree"]["children"].as_array().unwrap().len(), 2);
    }
}
