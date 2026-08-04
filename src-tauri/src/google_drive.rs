use std::collections::{hash_map::DefaultHasher, HashMap, HashSet};
use std::fs;
use std::hash::{Hash, Hasher};
use std::io::{ErrorKind, Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Mutex,
};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use oauth2::basic::BasicClient;
use oauth2::{
    AuthType, AuthUrl, AuthorizationCode, ClientId, ClientSecret, CsrfToken, EndpointNotSet,
    EndpointSet, PkceCodeChallenge, RedirectUrl, RefreshToken, Scope, TokenResponse, TokenUrl,
};
use once_cell::sync::Lazy;
use reqwest::{Client, StatusCode};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::Manager;
use url::Url;

use crate::oauth_config;

const API_ROOT: &str = "https://www.googleapis.com/drive/v3";
const DRIVE_SCOPE: &str = "https://www.googleapis.com/auth/drive";
const KEYCHAIN_SERVICE: &str = "com.duckdisk.dev.googledrive";
const CACHE_VERSION: &str = "duckdisk-google-drive-cache-v1";
const CALLBACK_TIMEOUT: Duration = Duration::from_secs(120);
const USER_CAP_MESSAGE: &str = "Google Drive sign-in is temporarily unavailable because DuckDisk has reached Google's 100-user limit. Please check for a newer DuckDisk release or try again after the app completes Google verification.";
const SCAN_CANCELLED_MESSAGE: &str = "Google Drive scan cancelled.";

type ConfiguredBasicClient =
    BasicClient<EndpointSet, EndpointNotSet, EndpointNotSet, EndpointNotSet, EndpointSet>;

static ACTIVE_SCANS: Lazy<Mutex<HashSet<String>>> = Lazy::new(|| Mutex::new(HashSet::new()));
static CANCELLED_SCANS: Lazy<Mutex<HashSet<String>>> = Lazy::new(|| Mutex::new(HashSet::new()));
static OAUTH_CANCELLED: AtomicBool = AtomicBool::new(false);

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

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct DeleteStatusPayload {
    current: u64,
    total: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GoogleDriveDeleteFailure {
    item_id: String,
    message: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GoogleDriveDeleteResult {
    deleted_ids: Vec<String>,
    failures: Vec<GoogleDriveDeleteFailure>,
}

pub fn get_state(app_handle: &tauri::AppHandle) -> Result<GoogleDriveState, String> {
    Ok(GoogleDriveState {
        configured: !client_id().is_empty() && !client_secret().is_empty(),
        accounts: read_accounts(app_handle)?,
    })
}

pub async fn connect_account(app_handle: &tauri::AppHandle) -> Result<GoogleDriveAccount, String> {
    OAUTH_CANCELLED.store(false, Ordering::SeqCst);
    let client_id = required_client_id()?;
    let client_secret = required_client_secret()?;
    let listener = TcpListener::bind("127.0.0.1:0").map_err(|err| err.to_string())?;
    let port = listener.local_addr().map_err(|err| err.to_string())?.port();
    let redirect = format!("http://127.0.0.1:{port}");
    let oauth_client = oauth_client(&client_id, &client_secret, Some(&redirect))?;
    let (pkce_challenge, pkce_verifier) = PkceCodeChallenge::new_random_sha256();
    let (authorize_url, csrf_token) = oauth_client
        .authorize_url(CsrfToken::new_random)
        .set_pkce_challenge(pkce_challenge)
        .add_scope(Scope::new(DRIVE_SCOPE.to_string()))
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

    let oauth_http = oauth_http_client()?;
    let token = oauth_client
        .exchange_code(AuthorizationCode::new(code))
        .set_pkce_verifier(pkce_verifier)
        .request_async(&oauth_http)
        .await
        .map_err(|err| google_token_error_message(&format!("{err:?}")))?;
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

pub fn cancel_connection() {
    OAUTH_CANCELLED.store(true, Ordering::SeqCst);
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

pub async fn revoke_account(app_handle: &tauri::AppHandle, account_id: &str) -> Result<(), String> {
    let credential = read_credential(account_id)?;
    let response = Client::new()
        .post("https://oauth2.googleapis.com/revoke")
        .form(&[("token", credential.refresh_token.as_str())])
        .send()
        .await
        .map_err(|err| format!("Could not reach Google while revoking access: {err}"))?;

    if !response.status().is_success() && response.status() != StatusCode::BAD_REQUEST {
        return Err(format!(
            "Google could not revoke DuckDisk access: {}",
            response.status()
        ));
    }

    disconnect_account(app_handle, account_id)
}

pub async fn trash_items(
    app_handle: &tauri::AppHandle,
    account_id: &str,
    item_ids: Vec<String>,
) -> Result<GoogleDriveDeleteResult, String> {
    if item_ids.is_empty() {
        return Ok(GoogleDriveDeleteResult {
            deleted_ids: Vec::new(),
            failures: Vec::new(),
        });
    }
    if !read_accounts(app_handle)?
        .iter()
        .any(|account| account.id == account_id)
    {
        return Err("Google Drive account is not connected".to_string());
    }

    let mut access_token = refresh_access_token(account_id).await?;
    let http = Client::new();
    let total = item_ids.len() as u64;
    let mut deleted_ids = Vec::new();
    let mut failures = Vec::new();

    for (index, item_id) in item_ids.into_iter().enumerate() {
        let mut result = google_trash_item(&http, &access_token, &item_id).await;
        if result
            .as_ref()
            .err()
            .map(|message| is_authentication_error(message))
            .unwrap_or(false)
        {
            result = match refresh_access_token(account_id).await {
                Ok(refreshed_token) => {
                    access_token = refreshed_token;
                    google_trash_item(&http, &access_token, &item_id).await
                }
                Err(err) => Err(err),
            };
        }

        match result {
            Ok(()) => deleted_ids.push(item_id),
            Err(message) => failures.push(GoogleDriveDeleteFailure { item_id, message }),
        }
        app_handle
            .emit_all(
                "googledrive_delete_status",
                DeleteStatusPayload {
                    current: index as u64 + 1,
                    total,
                },
            )
            .ok();
    }

    if !deleted_ids.is_empty() {
        let path = cache_path(app_handle, account_id)?;
        if let Some(mut cache) = read_cache(&path, account_id) {
            for item_id in &deleted_ids {
                remove_cached_subtree(&mut cache, item_id);
            }
            if write_cache(&path, &cache).is_err() {
                fs::remove_file(path).ok();
            }
        }

        if let Ok(about) = fetch_about(&http, &access_token).await {
            upsert_account(app_handle, account_from_about(&about)).ok();
        }
    }

    Ok(GoogleDriveDeleteResult {
        deleted_ids,
        failures,
    })
}

pub fn start_scan(
    app_handle: tauri::AppHandle,
    account_id: String,
    force_full: bool,
) -> Result<(), String> {
    if !read_accounts(&app_handle)?
        .iter()
        .any(|item| item.id == account_id)
    {
        return Err("Google Drive account is not connected".to_string());
    }
    {
        let mut active = ACTIVE_SCANS.lock().unwrap_or_else(|item| item.into_inner());
        if !active.insert(account_id.clone()) {
            return Ok(());
        }
    }
    CANCELLED_SCANS
        .lock()
        .unwrap_or_else(|item| item.into_inner())
        .remove(&account_id);
    tauri::async_runtime::spawn(async move {
        match scan_account(&app_handle, &account_id, force_full).await {
            Ok(path) => {
                app_handle
                    .emit_all(
                        "googledrive_scan_completed",
                        CompletedPayload {
                            account_id: account_id.clone(),
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
                        "googledrive_scan_failed",
                        FailedPayload {
                            account_id: account_id.clone(),
                            message,
                        },
                    )
                    .ok();
            }
        }
        ACTIVE_SCANS
            .lock()
            .unwrap_or_else(|item| item.into_inner())
            .remove(&account_id);
        CANCELLED_SCANS
            .lock()
            .unwrap_or_else(|item| item.into_inner())
            .remove(&account_id);
    });
    Ok(())
}

pub fn stop_scan(account_id: &str) {
    if ACTIVE_SCANS
        .lock()
        .unwrap_or_else(|item| item.into_inner())
        .contains(account_id)
    {
        CANCELLED_SCANS
            .lock()
            .unwrap_or_else(|item| item.into_inner())
            .insert(account_id.to_string());
    }
}

pub fn read_scan_result(path: &str) -> Result<String, String> {
    let prefix = format!("duckdisk-google-drive-scan-{}-", std::process::id());
    let path = crate::temp_files::validate_result_file(path, &prefix)?;
    fs::read_to_string(path).map_err(|err| err.to_string())
}

async fn scan_account(
    app_handle: &tauri::AppHandle,
    account_id: &str,
    force_full: bool,
) -> Result<PathBuf, String> {
    ensure_scan_active(account_id)?;
    let access_token = refresh_access_token(account_id).await?;
    let http = Client::new();
    let about = fetch_about(&http, &access_token).await?;
    upsert_account(app_handle, account_from_about(&about))?;
    let path = cache_path(app_handle, account_id)?;
    let existing = (!force_full)
        .then(|| read_cache(&path, account_id))
        .flatten();
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
    ensure_scan_active(account_id)?;
    write_cache(&path, &cache)?;
    app_handle
        .emit_all(
            "googledrive_scan_finalizing",
            AccountPayload {
                account_id: account_id.to_string(),
            },
        )
        .ok();
    let account = read_accounts(app_handle)?
        .into_iter()
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
    app_handle
        .emit_all(
            "googledrive_scan_full",
            AccountPayload {
                account_id: account_id.to_string(),
            },
        )
        .ok();
    let root: DriveFile = google_get(
        http,
        access_token,
        &format!("{API_ROOT}/files/root?fields=id,name,mimeType"),
    )
    .await?;
    let root_id = root.id.clone();
    let mut items = HashMap::new();
    items.insert(root_id.clone(), cached_item(root));
    let mut page_token: Option<String> = None;
    let mut count = 0_u64;
    let mut total = 0_u64;
    loop {
        ensure_scan_active(account_id)?;
        let mut request = http
            .get(&format!("{API_ROOT}/files"))
            .bearer_auth(access_token)
            .query(&[
                ("spaces", "drive"),
                ("pageSize", "1000"),
                ("q", "trashed = false"),
                (
                    "fields",
                    "nextPageToken,files(id,name,mimeType,size,quotaBytesUsed,parents,trashed)",
                ),
            ]);
        if let Some(token) = &page_token {
            request = request.query(&[("pageToken", token)]);
        }
        let page: FilePage = google_response(request.send().await).await?;
        ensure_scan_active(account_id)?;
        for file in page.files {
            if file.id != root_id {
                let item = cached_item(file);
                if !item.is_folder {
                    total = total.saturating_add(item.size);
                }
                items.insert(item.id.clone(), item);
                count += 1;
            }
        }
        emit_status(app_handle, account_id, count, total);
        page_token = page.next_page_token;
        if page_token.is_none() {
            break;
        }
    }
    let token: StartPageToken = google_get(
        http,
        access_token,
        &format!("{API_ROOT}/changes/startPageToken"),
    )
    .await?;
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
    app_handle
        .emit_all(
            "googledrive_scan_incremental",
            AccountPayload {
                account_id: cache.account_id.clone(),
            },
        )
        .ok();
    let mut token = cache.page_token.clone();
    let mut count = 0_u64;
    let mut total = 0_u64;
    loop {
        ensure_scan_active(&cache.account_id)?;
        let request = http.get(&format!("{API_ROOT}/changes"))
            .bearer_auth(access_token)
            .query(&[
                ("pageToken", token.as_str()),
                ("pageSize", "1000"),
                ("spaces", "drive"),
                ("fields", "nextPageToken,newStartPageToken,changes(fileId,removed,file(id,name,mimeType,size,quotaBytesUsed,parents,trashed))"),
            ]);
        let page: ChangePage = google_response(request.send().await).await?;
        ensure_scan_active(&cache.account_id)?;
        for change in page.changes {
            if change.removed.unwrap_or(false) {
                cache.items.remove(&change.file_id);
            } else if let Some(file) = change.file {
                if file.trashed.unwrap_or(false) {
                    cache.items.remove(&file.id);
                } else {
                    let item = cached_item(file);
                    if !item.is_folder {
                        total = total.saturating_add(item.size);
                    }
                    cache.items.insert(item.id.clone(), item);
                }
            }
            count += 1;
        }
        emit_status(app_handle, &cache.account_id, count, total);
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
        size: file
            .size
            .or(file.quota_bytes_used)
            .and_then(|size| size.parse().ok())
            .unwrap_or_default(),
        is_folder: file.mime_type.as_deref() == Some("application/vnd.google-apps.folder"),
    }
}

fn build_scan_json(
    cache: &GoogleDriveCache,
    account: &GoogleDriveAccount,
) -> Result<String, String> {
    let mut child_ids: HashMap<String, Vec<String>> = HashMap::new();
    for item in cache.items.values() {
        if item.id == cache.root_id {
            continue;
        }
        let parent = item
            .parent_id
            .as_ref()
            .filter(|parent| cache.items.contains_key(*parent))
            .cloned()
            .unwrap_or_else(|| cache.root_id.clone());
        child_ids.entry(parent).or_default().push(item.id.clone());
    }
    let mut visiting = HashSet::new();
    let mut children = child_ids
        .get(&cache.root_id)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|id| build_node(&id, cache, &child_ids, &mut visiting))
        .collect::<Vec<_>>();
    children.sort_by_key(|node| std::cmp::Reverse(node["size"].as_u64().unwrap_or_default()));
    let size = children
        .iter()
        .map(|node| node["size"].as_u64().unwrap_or_default())
        .sum::<u64>();
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
    }))
    .map_err(|err| err.to_string())
}

fn build_node(
    id: &str,
    cache: &GoogleDriveCache,
    child_ids: &HashMap<String, Vec<String>>,
    visiting: &mut HashSet<String>,
) -> Option<Value> {
    if !visiting.insert(id.to_string()) {
        return None;
    }
    let item = cache.items.get(id)?;
    let mut children = if item.is_folder {
        child_ids
            .get(id)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|child| build_node(&child, cache, child_ids, visiting))
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    children.sort_by_key(|node| std::cmp::Reverse(node["size"].as_u64().unwrap_or_default()));
    visiting.remove(id);
    let size = if item.is_folder {
        children
            .iter()
            .map(|node| node["size"].as_u64().unwrap_or_default())
            .sum()
    } else {
        item.size
    };
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

async fn google_get<T: DeserializeOwned>(
    http: &Client,
    token: &str,
    url: &str,
) -> Result<T, String> {
    google_response(http.get(url).bearer_auth(token).send().await).await
}

async fn google_response<T: DeserializeOwned>(
    response: Result<reqwest::Response, reqwest::Error>,
) -> Result<T, String> {
    let response = response.map_err(|err| format!("Could not reach Google Drive: {err}"))?;
    let status = response.status();
    let body = response.text().await.map_err(|err| err.to_string())?;
    if !status.is_success() {
        return Err(format!("Google Drive returned {status}: {body}"));
    }
    serde_json::from_str(&body).map_err(|err| format!("Invalid Google Drive response: {err}"))
}

async fn google_trash_item(http: &Client, access_token: &str, item_id: &str) -> Result<(), String> {
    let mut url = google_drive_item_url(item_id)?;
    url.query_pairs_mut()
        .append_pair("supportsAllDrives", "true")
        .append_pair("fields", "id,trashed");

    let mut attempts = 0_u32;
    loop {
        let response = match http
            .patch(url.as_str())
            .bearer_auth(access_token)
            .json(&json!({ "trashed": true }))
            .send()
            .await
        {
            Ok(response) => response,
            Err(err) if (err.is_connect() || err.is_timeout()) && attempts < 5 => {
                attempts += 1;
                tokio::time::sleep(Duration::from_secs(2_u64.pow(attempts).min(30))).await;
                continue;
            }
            Err(err) => {
                return Err(format!(
                    "Could not reach Google Drive while moving the item to Trash: {err}"
                ))
            }
        };
        let status = response.status();
        if status.is_success() {
            let body = response.text().await.unwrap_or_default();
            let updated_file: Value = serde_json::from_str(&body)
                .map_err(|err| format!("Invalid Google Drive update response: {err}"))?;
            if updated_file
                .get("trashed")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                return Ok(());
            }
            return Err(
                "Google Drive accepted the update but did not move the item to Trash".to_string(),
            );
        }
        if is_retryable_delete_status(status) {
            attempts += 1;
            if attempts > 5 {
                let body = response.text().await.unwrap_or_default();
                return Err(google_error_message(status, &body));
            }
            let delay = response
                .headers()
                .get("retry-after")
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or(2_u64.pow(attempts));
            tokio::time::sleep(Duration::from_secs(delay.min(60))).await;
            continue;
        }
        let body = response.text().await.unwrap_or_default();
        let message = google_error_message(status, &body);
        if status == StatusCode::FORBIDDEN && message.to_ascii_lowercase().contains("insufficient")
        {
            return Err(
                "Reconnect Google Drive from All Disks to grant permission to move items to Trash."
                    .to_string(),
            );
        }
        return Err(message);
    }
}

fn google_drive_item_url(item_id: &str) -> Result<Url, String> {
    let mut url = Url::parse(API_ROOT).map_err(|err| err.to_string())?;
    url.path_segments_mut()
        .map_err(|_| "Could not build Google Drive item URL".to_string())?
        .push("files")
        .push(item_id);
    Ok(url)
}

fn is_retryable_delete_status(status: StatusCode) -> bool {
    matches!(status.as_u16(), 408 | 409 | 429 | 500 | 502 | 503 | 504)
}

fn is_authentication_error(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    message.contains("401 unauthorized")
        || message.contains("invalid credentials")
        || message.contains("invalid_grant")
}

fn google_error_message(status: StatusCode, body: &str) -> String {
    let message = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|value| {
            value
                .get("error")
                .and_then(|error| error.get("message"))
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .filter(|message| !message.trim().is_empty());

    match message {
        Some(message) => format!("Google Drive returned {status}: {message}"),
        None => format!("Google Drive returned {status}"),
    }
}

async fn refresh_access_token(account_id: &str) -> Result<String, String> {
    let credential = read_credential(account_id)?;
    let oauth_http = oauth_http_client()?;
    let token = oauth_client(&required_client_id()?, &required_client_secret()?, None)?
        .exchange_refresh_token(&RefreshToken::new(credential.refresh_token))
        .request_async(&oauth_http)
        .await
        .map_err(|err| format!("Could not refresh Google sign-in: {err}"))?;
    Ok(token.access_token().secret().to_string())
}

fn oauth_client(
    client_id: &str,
    client_secret: &str,
    redirect: Option<&str>,
) -> Result<ConfiguredBasicClient, String> {
    let client = BasicClient::new(ClientId::new(client_id.to_string()))
        .set_client_secret(ClientSecret::new(client_secret.to_string()))
        .set_auth_uri(
            AuthUrl::new("https://accounts.google.com/o/oauth2/v2/auth".to_string())
                .map_err(|err| err.to_string())?,
        )
        .set_token_uri(
            TokenUrl::new("https://oauth2.googleapis.com/token".to_string())
                .map_err(|err| err.to_string())?,
        )
        .set_auth_type(AuthType::RequestBody);
    match redirect {
        Some(uri) => Ok(client
            .set_redirect_uri(RedirectUrl::new(uri.to_string()).map_err(|err| err.to_string())?)),
        None => Ok(client),
    }
}

fn oauth_http_client() -> Result<Client, String> {
    Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|err| format!("Could not configure OAuth client: {err}"))
}

fn client_id() -> String {
    oauth_config::google_drive_client_id().trim().to_string()
}

fn client_secret() -> String {
    env!("DUCKDISK_GOOGLE_CLIENT_SECRET").trim().to_string()
}

fn required_client_id() -> Result<String, String> {
    let value = client_id();
    if value.is_empty() {
        Err("Google Drive is not configured in this build.".to_string())
    } else {
        Ok(value)
    }
}

fn required_client_secret() -> Result<String, String> {
    let value = client_secret();
    if value.is_empty() {
        Err("Google Drive is not configured in this build.".to_string())
    } else {
        Ok(value)
    }
}

fn account_from_about(about: &AboutResponse) -> GoogleDriveAccount {
    let total = about
        .storage_quota
        .limit
        .as_ref()
        .and_then(|value| value.parse().ok())
        .unwrap_or_default();
    let used = about
        .storage_quota
        .usage_in_drive
        .as_ref()
        .and_then(|value| value.parse().ok())
        .unwrap_or_default();
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
    let value = serde_json::to_string(&StoredCredential {
        refresh_token: refresh_token.to_string(),
    })
    .map_err(|err| err.to_string())?;
    entry.set_password(&value).map_err(|err| err.to_string())
}

fn read_credential(account_id: &str) -> Result<StoredCredential, String> {
    let entry = keyring::Entry::new(KEYCHAIN_SERVICE, account_id).map_err(|err| err.to_string())?;
    let value = entry
        .get_password()
        .map_err(|err| format!("Google sign-in is missing from Keychain: {err}"))?;
    serde_json::from_str(&value).map_err(|err| err.to_string())
}

fn accounts_path(app_handle: &tauri::AppHandle) -> Result<PathBuf, String> {
    app_handle
        .path_resolver()
        .app_config_dir()
        .map(|path| path.join("google-drive-accounts.json"))
        .ok_or_else(|| "Could not resolve DuckDisk configuration directory".to_string())
}

fn read_accounts(app_handle: &tauri::AppHandle) -> Result<Vec<GoogleDriveAccount>, String> {
    let path = accounts_path(app_handle)?;
    if !path.exists() {
        return Ok(Vec::new());
    }
    serde_json::from_str(&fs::read_to_string(path).map_err(|err| err.to_string())?)
        .map_err(|err| err.to_string())
}

fn write_accounts(
    app_handle: &tauri::AppHandle,
    accounts: &[GoogleDriveAccount],
) -> Result<(), String> {
    let path = accounts_path(app_handle)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| err.to_string())?;
    }
    fs::write(
        path,
        serde_json::to_string_pretty(accounts).map_err(|err| err.to_string())?,
    )
    .map_err(|err| err.to_string())
}

fn upsert_account(
    app_handle: &tauri::AppHandle,
    account: GoogleDriveAccount,
) -> Result<(), String> {
    let mut accounts = read_accounts(app_handle)?;
    if let Some(existing) = accounts.iter_mut().find(|item| item.id == account.id) {
        *existing = account;
    } else {
        accounts.push(account);
    }
    write_accounts(app_handle, &accounts)
}

fn cache_path(app_handle: &tauri::AppHandle, account_id: &str) -> Result<PathBuf, String> {
    let mut hasher = DefaultHasher::new();
    account_id.hash(&mut hasher);
    app_handle
        .path_resolver()
        .app_cache_dir()
        .map(|path| {
            path.join("google-drive")
                .join(format!("{:016x}.json", hasher.finish()))
        })
        .ok_or_else(|| "Could not resolve DuckDisk cache directory".to_string())
}

fn read_cache(path: &Path, account_id: &str) -> Option<GoogleDriveCache> {
    let cache: GoogleDriveCache = serde_json::from_str(&fs::read_to_string(path).ok()?).ok()?;
    (cache.version == CACHE_VERSION
        && cache.account_id == account_id
        && !cache.page_token.is_empty())
    .then_some(cache)
}

fn write_cache(path: &Path, cache: &GoogleDriveCache) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| err.to_string())?;
    }
    let temporary = path.with_extension("json.tmp");
    fs::write(
        &temporary,
        serde_json::to_string(cache).map_err(|err| err.to_string())?,
    )
    .map_err(|err| err.to_string())?;
    fs::rename(temporary, path).map_err(|err| err.to_string())
}

fn remove_cached_subtree(cache: &mut GoogleDriveCache, root_id: &str) {
    let mut removed = HashSet::from([root_id.to_string()]);
    loop {
        let before = removed.len();
        for item in cache.items.values() {
            if item
                .parent_id
                .as_ref()
                .map(|parent_id| removed.contains(parent_id))
                .unwrap_or(false)
            {
                removed.insert(item.id.clone());
            }
        }
        if removed.len() == before {
            break;
        }
    }
    cache.items.retain(|item_id, _| !removed.contains(item_id));
}

fn write_scan_result(content: &str) -> Result<PathBuf, String> {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let path = std::env::temp_dir().join(format!(
        "duckdisk-google-drive-scan-{}-{stamp}.json",
        std::process::id()
    ));
    fs::write(&path, content).map_err(|err| err.to_string())?;
    Ok(path)
}

fn emit_status(app_handle: &tauri::AppHandle, account_id: &str, items: u64, total: u64) {
    app_handle
        .emit_all(
            "googledrive_scan_status",
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

fn ensure_scan_active(account_id: &str) -> Result<(), String> {
    if CANCELLED_SCANS
        .lock()
        .unwrap_or_else(|item| item.into_inner())
        .contains(account_id)
    {
        Err(SCAN_CANCELLED_MESSAGE.to_string())
    } else {
        Ok(())
    }
}

fn wait_for_oauth_callback(listener: TcpListener, expected_state: &str) -> Result<String, String> {
    listener
        .set_nonblocking(true)
        .map_err(|err| err.to_string())?;
    let started = Instant::now();
    while started.elapsed() < CALLBACK_TIMEOUT {
        if OAUTH_CANCELLED.load(Ordering::SeqCst) {
            return Err("Google Drive connection cancelled.".to_string());
        }
        match listener.accept() {
            Ok((mut stream, _)) => {
                let mut buffer = [0_u8; 8192];
                let count = stream.read(&mut buffer).map_err(|err| err.to_string())?;
                let request = String::from_utf8_lossy(&buffer[..count]);
                let target = request
                    .split_whitespace()
                    .nth(1)
                    .ok_or_else(|| "Invalid Google callback".to_string())?;
                let url = Url::parse(&format!("http://127.0.0.1{target}"))
                    .map_err(|err| err.to_string())?;
                let params = url.query_pairs().into_owned().collect::<HashMap<_, _>>();
                if params.get("state").map(String::as_str) != Some(expected_state) {
                    return Err("Google sign-in state did not match".to_string());
                }
                if let Some(error) = params.get("error") {
                    let message = google_oauth_error_message(
                        error,
                        params.get("error_description").map(String::as_str),
                    );
                    write_callback_response(&mut stream, "Google Drive connection failed")?;
                    return Err(message);
                }
                let code = params
                    .get("code")
                    .cloned()
                    .ok_or_else(|| "Google callback did not include a code".to_string())?;
                write_callback_response(&mut stream, "Google Drive connected")?;
                return Ok(code);
            }
            Err(err) if err.kind() == ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(100))
            }
            Err(err) => return Err(err.to_string()),
        }
    }
    Err("Google sign-in did not return to DuckDisk. If Google showed 'Sign in with Google temporarily disabled', DuckDisk has reached Google's 100-user limit. Otherwise, try again and finish the browser sign-in within two minutes.".to_string())
}

fn google_oauth_error_message(error: &str, description: Option<&str>) -> String {
    let details = description.unwrap_or(error);
    if is_user_cap_error(details) || is_user_cap_error(error) {
        return USER_CAP_MESSAGE.to_string();
    }
    if error.eq_ignore_ascii_case("access_denied") {
        return "Google Drive access was not granted.".to_string();
    }
    format!("Google sign-in failed: {details}")
}

fn google_token_error_message(details: &str) -> String {
    if is_user_cap_error(details) {
        USER_CAP_MESSAGE.to_string()
    } else {
        format!("Google sign-in failed while exchanging the authorization code: {details}")
    }
}

fn is_user_cap_error(details: &str) -> bool {
    let normalized = details.to_ascii_lowercase();
    normalized.contains("rate_limit_exceeded")
        || normalized.contains("temporarily disabled")
        || normalized.contains("user cap")
        || normalized.contains("100-user")
        || normalized.contains("user limit")
}

fn write_callback_response(stream: &mut std::net::TcpStream, title: &str) -> Result<(), String> {
    let body = format!("<!doctype html><meta charset=\"utf-8\"><title>{title}</title><style>body{{font:16px system-ui;background:#15181c;color:#eef1f3;display:grid;place-items:center;height:100vh;margin:0}}</style><h1>{title}</h1>");
    let response = format!("HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body);
    stream
        .write_all(response.as_bytes())
        .map_err(|err| err.to_string())
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
        items.insert(
            "folder".to_string(),
            item("folder", Some("root"), "Work", 0, true),
        );
        items.insert(
            "file".to_string(),
            item("file", Some("folder"), "notes.txt", 42, false),
        );
        items.insert(
            "orphan".to_string(),
            item("orphan", Some("missing"), "shared.pdf", 8, false),
        );
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
        let result: Value =
            serde_json::from_str(&build_scan_json(&cache, &account).unwrap()).unwrap();
        assert_eq!(result["tree"]["size"], 50);
        assert_eq!(result["tree"]["children"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn removes_google_drive_cached_subtree() {
        let mut items = HashMap::new();
        items.insert("root".to_string(), item("root", None, "My Drive", 0, true));
        items.insert(
            "folder".to_string(),
            item("folder", Some("root"), "Work", 0, true),
        );
        items.insert(
            "nested".to_string(),
            item("nested", Some("folder"), "Drafts", 0, true),
        );
        items.insert(
            "file".to_string(),
            item("file", Some("nested"), "notes.txt", 42, false),
        );
        items.insert(
            "keep".to_string(),
            item("keep", Some("root"), "keep.pdf", 8, false),
        );
        let mut cache = GoogleDriveCache {
            version: CACHE_VERSION.to_string(),
            account_id: "account".to_string(),
            root_id: "root".to_string(),
            page_token: "token".to_string(),
            items,
        };

        remove_cached_subtree(&mut cache, "folder");

        assert!(cache.items.contains_key("root"));
        assert!(cache.items.contains_key("keep"));
        assert!(!cache.items.contains_key("folder"));
        assert!(!cache.items.contains_key("nested"));
        assert!(!cache.items.contains_key("file"));
    }

    #[test]
    fn explains_google_oauth_user_cap() {
        let message = google_oauth_error_message(
            "rate_limit_exceeded",
            Some("Sign in with Google temporarily disabled for this app"),
        );
        assert_eq!(message, USER_CAP_MESSAGE);
    }

    #[test]
    fn distinguishes_cancelled_google_oauth() {
        let message = google_oauth_error_message("access_denied", None);
        assert_eq!(message, "Google Drive access was not granted.");
    }

    #[test]
    fn builds_google_drive_item_url_without_an_empty_path_segment() {
        let url = google_drive_item_url("file-id").unwrap();

        assert_eq!(
            url.as_str(),
            "https://www.googleapis.com/drive/v3/files/file-id"
        );
    }
}
