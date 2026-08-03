use std::collections::{hash_map::DefaultHasher, HashMap, HashSet, VecDeque};
use std::fs;
use std::hash::{Hash, Hasher};
use std::io::{ErrorKind, Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use oauth2::basic::BasicClient;
use oauth2::reqwest::async_http_client;
use oauth2::{
    AuthUrl, AuthorizationCode, ClientId, CsrfToken, PkceCodeChallenge, RedirectUrl, RefreshToken,
    Scope, TokenResponse, TokenUrl,
};
use once_cell::sync::Lazy;
use reqwest::{Client, StatusCode};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Mutex,
};
use tauri::Manager;
use url::Url;

const GRAPH_ROOT: &str = "https://graph.microsoft.com/v1.0";
const KEYCHAIN_SERVICE: &str = "com.duckdisk.dev.onedrive";
const CACHE_VERSION: &str = "duckdisk-onedrive-cache-v1";
const CALLBACK_TIMEOUT: Duration = Duration::from_secs(300);
const ACCESS_TOKEN_REFRESH_INTERVAL: Duration = Duration::from_secs(45 * 60);
const CHECKPOINT_ITEM_INTERVAL: u64 = 20_000;
const SCAN_CANCELLED_MESSAGE: &str = "OneDrive scan cancelled.";

#[derive(Clone, Copy)]
enum ActiveScanPhase {
    Full,
    Incremental,
    Finalizing,
}

#[derive(Clone, Default)]
struct ActiveScan {
    items: u64,
    total: u64,
    phase: Option<ActiveScanPhase>,
    cancelled: bool,
}

static ACTIVE_SCANS: Lazy<Mutex<HashMap<String, ActiveScan>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));
static OAUTH_CANCELLED: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OneDriveAccount {
    pub id: String,
    pub name: String,
    pub drive_type: String,
    pub total_space: u64,
    pub used_space: u64,
    pub available_space: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OneDriveState {
    configured: bool,
    accounts: Vec<OneDriveAccount>,
}

#[derive(Serialize, Deserialize)]
struct StoredCredential {
    refresh_token: String,
    #[serde(default)]
    can_delete: bool,
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
struct OneDriveCache {
    version: String,
    account_id: String,
    root_id: String,
    delta_link: String,
    #[serde(default)]
    next_link: String,
    #[serde(default)]
    checkpoint_items: u64,
    #[serde(default)]
    checkpoint_bytes: u64,
    items: HashMap<String, CachedItem>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GraphDrive {
    id: String,
    drive_type: Option<String>,
    owner: Option<GraphOwner>,
    quota: Option<GraphQuota>,
}

#[derive(Deserialize)]
struct GraphOwner {
    user: Option<GraphUser>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GraphUser {
    display_name: Option<String>,
}

#[derive(Default, Deserialize)]
struct GraphQuota {
    total: Option<u64>,
    used: Option<u64>,
    remaining: Option<u64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GraphItem {
    id: String,
    name: Option<String>,
    size: Option<u64>,
    parent_reference: Option<ParentReference>,
    folder: Option<Value>,
    deleted: Option<Value>,
}

#[derive(Deserialize)]
struct ParentReference {
    id: Option<String>,
}

#[derive(Deserialize)]
struct DeltaPage {
    value: Vec<GraphItem>,
    #[serde(rename = "@odata.nextLink")]
    next_link: Option<String>,
    #[serde(rename = "@odata.deltaLink")]
    delta_link: Option<String>,
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
struct AccountPayload {
    account_id: String,
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
pub struct OneDriveDeleteFailure {
    item_id: String,
    message: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OneDriveDeleteResult {
    deleted_ids: Vec<String>,
    failures: Vec<OneDriveDeleteFailure>,
}

pub fn get_state(app_handle: &tauri::AppHandle) -> Result<OneDriveState, String> {
    Ok(OneDriveState {
        configured: !client_id().is_empty(),
        accounts: read_accounts(app_handle)?,
    })
}

pub async fn connect_account(app_handle: &tauri::AppHandle) -> Result<OneDriveAccount, String> {
    OAUTH_CANCELLED.store(false, Ordering::SeqCst);
    let client_id = required_client_id()?;
    let listener = TcpListener::bind("127.0.0.1:0").map_err(|err| err.to_string())?;
    let port = listener.local_addr().map_err(|err| err.to_string())?.port();
    let redirect = format!("http://localhost:{port}");
    let oauth_client = oauth_client(&client_id, Some(&redirect))?;
    let (pkce_challenge, pkce_verifier) = PkceCodeChallenge::new_random_sha256();
    let (authorize_url, csrf_token) = oauth_client
        .authorize_url(CsrfToken::new_random)
        .set_pkce_challenge(pkce_challenge)
        .add_scope(Scope::new("offline_access".to_string()))
        .add_scope(Scope::new("Files.ReadWrite".to_string()))
        .add_extra_param("prompt", "select_account")
        .url();

    Command::new("open")
        .arg(authorize_url.as_str())
        .spawn()
        .map_err(|err| format!("Could not open Microsoft sign-in: {err}"))?;

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
        .map_err(|err| format!("Microsoft sign-in failed: {err}"))?;
    let refresh_token = token
        .refresh_token()
        .ok_or_else(|| "Microsoft did not return a refresh token".to_string())?
        .secret()
        .to_string();

    let http = Client::new();
    let drive: GraphDrive = graph_get(
        &http,
        token.access_token().secret(),
        &format!("{GRAPH_ROOT}/me/drive?$select=id,driveType,owner,quota"),
    )
    .await?;
    let account = account_from_drive(&drive);
    store_credential(&account.id, &refresh_token, true)?;
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

    let cache_path = cache_path(app_handle, account_id)?;
    if cache_path.exists() {
        fs::remove_file(cache_path).map_err(|err| err.to_string())?;
    }
    Ok(())
}

pub fn start_scan(
    app_handle: tauri::AppHandle,
    account_id: String,
    force_full: bool,
) -> Result<(), String> {
    if !read_accounts(&app_handle)?
        .iter()
        .any(|account| account.id == account_id)
    {
        return Err("OneDrive account is not connected".to_string());
    }

    let active_scan = {
        let mut scans = ACTIVE_SCANS
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(scan) = scans.get(&account_id) {
            Some(scan.clone())
        } else {
            scans.insert(account_id.clone(), ActiveScan::default());
            None
        }
    };

    if let Some(scan) = active_scan {
        replay_active_scan(&app_handle, &account_id, &scan);
        return Ok(());
    }

    tauri::async_runtime::spawn(async move {
        match scan_account(&app_handle, &account_id, force_full).await {
            Ok(path) => {
                app_handle
                    .emit_all(
                        "onedrive_scan_completed",
                        CompletedPayload {
                            account_id: account_id.clone(),
                            path: path.display().to_string(),
                            errors_path: String::new(),
                        },
                    )
                    .ok();
            }
            Err(err) if err == SCAN_CANCELLED_MESSAGE => {}
            Err(err) => {
                app_handle
                    .emit_all(
                        "onedrive_scan_failed",
                        FailedPayload {
                            account_id: account_id.clone(),
                            message: err,
                        },
                    )
                    .ok();
            }
        }
        ACTIVE_SCANS
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&account_id);
    });
    Ok(())
}

pub fn stop_scan(account_id: &str) {
    if let Some(scan) = ACTIVE_SCANS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get_mut(account_id)
    {
        scan.cancelled = true;
    }
}

pub fn read_scan_result(path: &str) -> Result<String, String> {
    let prefix = format!("duckdisk-onedrive-scan-{}-", std::process::id());
    let path = crate::temp_files::validate_result_file(path, &prefix)?;
    fs::read_to_string(path).map_err(|err| err.to_string())
}

pub async fn refresh_item(
    app_handle: &tauri::AppHandle,
    account_id: &str,
    item_id: &str,
) -> Result<String, String> {
    if !read_accounts(app_handle)?
        .iter()
        .any(|account| account.id == account_id)
    {
        return Err("OneDrive account is not connected".to_string());
    }

    {
        let mut scans = ACTIVE_SCANS
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if scans.contains_key(account_id) {
            return Err(
                "Wait for the current OneDrive scan to finish before refreshing this item."
                    .to_string(),
            );
        }
        scans.insert(account_id.to_string(), ActiveScan::default());
    }

    let result = refresh_item_inner(app_handle, account_id, item_id).await;
    ACTIVE_SCANS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .remove(account_id);
    result
}

async fn refresh_item_inner(
    app_handle: &tauri::AppHandle,
    account_id: &str,
    item_id: &str,
) -> Result<String, String> {
    let path = cache_path(app_handle, account_id)?;
    let mut cache = read_cache(&path, account_id).ok_or_else(|| {
        "Complete a OneDrive scan before refreshing an individual item.".to_string()
    })?;
    if cache.delta_link.is_empty() {
        return Err(
            "Complete the current OneDrive scan before refreshing an individual item.".to_string(),
        );
    }
    if !cache.items.contains_key(item_id) {
        return Err("The selected OneDrive item is not present in the cache.".to_string());
    }

    let access_token = refresh_access_token(account_id).await?;
    let http = Client::new();
    let selected: GraphItem = graph_get(
        &http,
        &access_token,
        &format!(
            "{}?$select=id,name,size,parentReference,folder,file",
            drive_item_url(account_id, item_id)?
        ),
    )
    .await?;
    let selected_is_folder = selected.folder.is_some();

    remove_cached_subtree(&mut cache, item_id);
    apply_delta_item(&mut cache, selected);

    let mut folders = VecDeque::new();
    if selected_is_folder {
        folders.push_back(item_id.to_string());
    }

    while let Some(folder_id) = folders.pop_front() {
        let mut next_url = format!(
            "{}/children?$select=id,name,size,parentReference,folder,file&$top=200",
            drive_item_url(account_id, &folder_id)?
        );
        loop {
            let page: DeltaPage = graph_get(&http, &access_token, &next_url).await?;
            for item in page.value {
                if item.folder.is_some() {
                    folders.push_back(item.id.clone());
                }
                apply_delta_item(&mut cache, item);
            }
            match page.next_link {
                Some(next) => next_url = next,
                None => break,
            }
        }
    }

    write_cache(&path, &cache)?;
    build_item_json(&cache, item_id)
}

pub async fn delete_items(
    app_handle: &tauri::AppHandle,
    account_id: &str,
    item_ids: Vec<String>,
) -> Result<OneDriveDeleteResult, String> {
    if item_ids.is_empty() {
        return Ok(OneDriveDeleteResult {
            deleted_ids: Vec::new(),
            failures: Vec::new(),
        });
    }
    if !read_accounts(app_handle)?
        .iter()
        .any(|account| account.id == account_id)
    {
        return Err("OneDrive account is not connected".to_string());
    }
    if !read_credential(account_id)?.can_delete {
        return Err(
            "Reconnect OneDrive from All Disks to grant permission to move items to the Recycle Bin."
                .to_string(),
        );
    }

    let mut access_token = refresh_access_token(account_id).await?;
    let http = Client::new();
    let total = item_ids.len() as u64;
    let mut deleted_ids = Vec::new();
    let mut failures = Vec::new();

    for (index, item_id) in item_ids.into_iter().enumerate() {
        let mut result = match drive_item_url(account_id, &item_id) {
            Ok(url) => graph_delete(&http, &access_token, &url).await,
            Err(err) => Err(err),
        };
        if result
            .as_ref()
            .err()
            .map(|message| is_authentication_error(message))
            .unwrap_or(false)
        {
            result = match refresh_access_token(account_id).await {
                Ok(refreshed_token) => {
                    access_token = refreshed_token;
                    match drive_item_url(account_id, &item_id) {
                        Ok(url) => graph_delete(&http, &access_token, &url).await,
                        Err(err) => Err(err),
                    }
                }
                Err(err) => Err(err),
            };
        }
        match result {
            Ok(()) => deleted_ids.push(item_id),
            Err(message) => failures.push(OneDriveDeleteFailure { item_id, message }),
        }
        app_handle
            .emit_all(
                "onedrive_delete_status",
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

        if let Ok(drive) = graph_get::<GraphDrive>(
            &http,
            &access_token,
            &format!("{GRAPH_ROOT}/me/drive?$select=id,driveType,owner,quota"),
        )
        .await
        {
            upsert_account(app_handle, account_from_drive(&drive)).ok();
        }
    }

    Ok(OneDriveDeleteResult {
        deleted_ids,
        failures,
    })
}

async fn scan_account(
    app_handle: &tauri::AppHandle,
    account_id: &str,
    force_full: bool,
) -> Result<PathBuf, String> {
    ensure_scan_active(account_id)?;
    let mut access_token = refresh_access_token(account_id).await?;
    let mut access_token_refreshed_at = Instant::now();
    let http = Client::new();
    let drive: GraphDrive = graph_get(
        &http,
        &access_token,
        &format!("{GRAPH_ROOT}/me/drive?$select=id,driveType,owner,quota"),
    )
    .await?;
    if drive.id != account_id {
        return Err("The saved Microsoft account no longer matches this OneDrive".to_string());
    }
    upsert_account(app_handle, account_from_drive(&drive))?;

    let path = cache_path(app_handle, account_id)?;
    let mut cache = if !force_full {
        if let Some(cache) = read_cache(&path, account_id) {
            if cache.delta_link.is_empty() {
                emit_scan_phase(app_handle, account_id, ActiveScanPhase::Full);
            } else {
                emit_scan_phase(app_handle, account_id, ActiveScanPhase::Incremental);
            }
            cache
        } else {
            emit_scan_phase(app_handle, account_id, ActiveScanPhase::Full);
            create_empty_cache(&http, &access_token, account_id).await?
        }
    } else {
        emit_scan_phase(app_handle, account_id, ActiveScanPhase::Full);
        create_empty_cache(&http, &access_token, account_id).await?
    };

    let mut next_url = if !cache.next_link.is_empty() {
        cache.next_link.clone()
    } else if cache.delta_link.is_empty() {
        initial_delta_url()
    } else {
        cache.delta_link.clone()
    };
    let mut can_restart_full = !cache.delta_link.is_empty();
    let mut changed_items = cache.checkpoint_items;
    let mut changed_bytes = cache.checkpoint_bytes;
    let mut last_checkpoint_items = changed_items;
    let mut authentication_retried = false;

    if cache.next_link.is_empty() {
        cache.next_link = next_url.clone();
        write_cache(&path, &cache)?;
    } else {
        emit_scan_status(app_handle, account_id, changed_items, changed_bytes);
    }

    loop {
        ensure_scan_active(account_id)?;
        if access_token_refreshed_at.elapsed() >= ACCESS_TOKEN_REFRESH_INTERVAL {
            access_token = refresh_access_token(account_id).await?;
            access_token_refreshed_at = Instant::now();
            authentication_retried = false;
        }

        let page: DeltaPage =
            match graph_get(&http, &access_token, &next_url).await {
                Ok(page) => {
                    authentication_retried = false;
                    page
                }
                Err(err) if is_authentication_error(&err) && !authentication_retried => {
                    access_token = refresh_access_token(account_id).await?;
                    access_token_refreshed_at = Instant::now();
                    authentication_retried = true;
                    continue;
                }
                Err(err) if is_authentication_error(&err) => return Err(
                    "Microsoft sign-in expired during the scan. Reconnect OneDrive and try again."
                        .to_string(),
                ),
                Err(err) if can_restart_full && is_delta_resync_error(&err) => {
                    fs::remove_file(&path).ok();
                    cache = create_empty_cache(&http, &access_token, account_id).await?;
                    next_url = initial_delta_url();
                    cache.next_link = next_url.clone();
                    write_cache(&path, &cache)?;
                    can_restart_full = false;
                    changed_items = 0;
                    changed_bytes = 0;
                    last_checkpoint_items = 0;
                    authentication_retried = false;
                    emit_scan_status(app_handle, account_id, 0, 0);
                    emit_scan_phase(app_handle, account_id, ActiveScanPhase::Full);
                    continue;
                }
                Err(err) if is_delta_resync_error(&err) => {
                    return Err(
                        "OneDrive could not restart its change index. Use Clean Cache & Rescan."
                            .to_string(),
                    )
                }
                Err(err) => return Err(err),
            };
        ensure_scan_active(account_id)?;
        for item in page.value {
            changed_items += 1;
            if item.folder.is_none() && item.deleted.is_none() {
                changed_bytes = changed_bytes.saturating_add(item.size.unwrap_or_default());
            }
            apply_delta_item(&mut cache, item);
        }
        emit_scan_status(app_handle, account_id, changed_items, changed_bytes);

        if let Some(next) = page.next_link {
            cache.next_link = next.clone();
            cache.checkpoint_items = changed_items;
            cache.checkpoint_bytes = changed_bytes;
            if changed_items.saturating_sub(last_checkpoint_items) >= CHECKPOINT_ITEM_INTERVAL {
                write_cache(&path, &cache)?;
                last_checkpoint_items = changed_items;
            }
            next_url = next;
            continue;
        }
        cache.delta_link = page
            .delta_link
            .ok_or_else(|| "OneDrive scan ended without a delta token".to_string())?;
        cache.next_link.clear();
        cache.checkpoint_items = 0;
        cache.checkpoint_bytes = 0;
        break;
    }

    ensure_scan_active(account_id)?;

    write_cache(&path, &cache)?;
    emit_scan_phase(app_handle, account_id, ActiveScanPhase::Finalizing);
    let account = read_accounts(app_handle)?
        .into_iter()
        .find(|account| account.id == account_id)
        .ok_or_else(|| "OneDrive account metadata is missing".to_string())?;
    let result = build_scan_json(&cache, &account)?;
    write_scan_result(&result)
}

async fn create_empty_cache(
    http: &Client,
    access_token: &str,
    account_id: &str,
) -> Result<OneDriveCache, String> {
    let root: GraphItem = graph_get(
        http,
        access_token,
        &format!("{GRAPH_ROOT}/me/drive/root?$select=id,name,size,parentReference,folder"),
    )
    .await?;
    let root_id = root.id.clone();
    let mut items = HashMap::new();
    items.insert(
        root_id.clone(),
        CachedItem {
            id: root_id.clone(),
            parent_id: None,
            name: root.name.unwrap_or_else(|| "OneDrive".to_string()),
            size: root.size.unwrap_or_default(),
            is_folder: true,
        },
    );
    Ok(OneDriveCache {
        version: CACHE_VERSION.to_string(),
        account_id: account_id.to_string(),
        root_id,
        delta_link: String::new(),
        next_link: String::new(),
        checkpoint_items: 0,
        checkpoint_bytes: 0,
        items,
    })
}

fn apply_delta_item(cache: &mut OneDriveCache, item: GraphItem) {
    if item.deleted.is_some() {
        cache.items.remove(&item.id);
        return;
    }
    let id = item.id;
    cache.items.insert(
        id.clone(),
        CachedItem {
            id,
            parent_id: item.parent_reference.and_then(|parent| parent.id),
            name: item.name.unwrap_or_else(|| "(unnamed)".to_string()),
            size: item.size.unwrap_or_default(),
            is_folder: item.folder.is_some(),
        },
    );
}

fn build_scan_json(cache: &OneDriveCache, account: &OneDriveAccount) -> Result<String, String> {
    let mut child_ids: HashMap<String, Vec<String>> = HashMap::new();
    for item in cache.items.values() {
        if item.id == cache.root_id {
            continue;
        }
        if let Some(parent_id) = &item.parent_id {
            child_ids
                .entry(parent_id.clone())
                .or_default()
                .push(item.id.clone());
        }
    }

    let mut visiting = HashSet::new();
    let mut children = Vec::new();
    for child_id in child_ids.get(&cache.root_id).cloned().unwrap_or_default() {
        if let Some(node) = build_node(&child_id, cache, &child_ids, &mut visiting) {
            children.push(node);
        }
    }
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
            "displayName": format!("OneDrive - {}", account.name),
            "cloudId": cache.root_id,
            "isDirectory": true,
            "size": size,
            "children": children
        }
    }))
    .map_err(|err| err.to_string())
}

fn build_item_json(cache: &OneDriveCache, item_id: &str) -> Result<String, String> {
    let mut child_ids: HashMap<String, Vec<String>> = HashMap::new();
    for item in cache.items.values() {
        if let Some(parent_id) = &item.parent_id {
            child_ids
                .entry(parent_id.clone())
                .or_default()
                .push(item.id.clone());
        }
    }

    let node = build_node(item_id, cache, &child_ids, &mut HashSet::new())
        .ok_or_else(|| "The refreshed OneDrive item could not be rebuilt.".to_string())?;
    serde_json::to_string(&node).map_err(|err| err.to_string())
}

fn build_node(
    item_id: &str,
    cache: &OneDriveCache,
    child_ids: &HashMap<String, Vec<String>>,
    visiting: &mut HashSet<String>,
) -> Option<Value> {
    if !visiting.insert(item_id.to_string()) {
        return None;
    }
    let item = cache.items.get(item_id)?;
    let mut children = Vec::new();
    if item.is_folder {
        for child_id in child_ids.get(item_id).cloned().unwrap_or_default() {
            if let Some(node) = build_node(&child_id, cache, child_ids, visiting) {
                children.push(node);
            }
        }
        children.sort_by_key(|node| std::cmp::Reverse(node["size"].as_u64().unwrap_or_default()));
    }
    visiting.remove(item_id);
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

async fn refresh_access_token(account_id: &str) -> Result<String, String> {
    let client_id = required_client_id()?;
    let credential = read_credential(account_id)?;
    let token = oauth_client(&client_id, None)?
        .exchange_refresh_token(&RefreshToken::new(credential.refresh_token.clone()))
        .request_async(async_http_client)
        .await
        .map_err(|err| format!("Could not refresh Microsoft sign-in: {err}"))?;

    if let Some(refresh_token) = token.refresh_token() {
        store_credential(account_id, refresh_token.secret(), credential.can_delete)?;
    }
    Ok(token.access_token().secret().to_string())
}

async fn graph_get<T: DeserializeOwned>(
    http: &Client,
    access_token: &str,
    url: &str,
) -> Result<T, String> {
    let mut attempts = 0;
    loop {
        let response = http
            .get(url)
            .bearer_auth(access_token)
            .send()
            .await
            .map_err(|err| err.to_string())?;
        let status = response.status();
        if status == StatusCode::TOO_MANY_REQUESTS || status == StatusCode::SERVICE_UNAVAILABLE {
            attempts += 1;
            if attempts > 5 {
                return Err(format!("Microsoft Graph remained unavailable ({status})"));
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
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(format!("Microsoft Graph returned {status}: {body}"));
        }
        return response.json::<T>().await.map_err(|err| err.to_string());
    }
}

fn is_delta_resync_error(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    message.contains("410 gone")
        || message.contains("resyncrequired")
        || message.contains("resyncchangesapplydifferences")
        || message.contains("resyncchangesuploaddifferences")
}

fn is_authentication_error(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    message.contains("401 unauthorized") || message.contains("invalidauthenticationtoken")
}

async fn graph_delete(http: &Client, access_token: &str, url: &str) -> Result<(), String> {
    let mut attempts = 0;
    loop {
        let response = match http.delete(url).bearer_auth(access_token).send().await {
            Ok(response) => response,
            Err(err) if (err.is_connect() || err.is_timeout()) && attempts < 5 => {
                attempts += 1;
                tokio::time::sleep(Duration::from_secs(2_u64.pow(attempts).min(30))).await;
                continue;
            }
            Err(err) => {
                return Err(format!(
                    "Could not reach Microsoft Graph while moving the item: {err}"
                ))
            }
        };
        let status = response.status();
        if status == StatusCode::NO_CONTENT || status == StatusCode::NOT_FOUND {
            return Ok(());
        }
        if is_retryable_delete_status(status) {
            attempts += 1;
            if attempts > 5 {
                let body = response.text().await.unwrap_or_default();
                return Err(graph_error_message(status, &body));
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
        return Err(graph_error_message(status, &body));
    }
}

fn is_retryable_delete_status(status: StatusCode) -> bool {
    matches!(
        status.as_u16(),
        408 | 409 | 423 | 429 | 500 | 502 | 503 | 504
    )
}

fn graph_error_message(status: StatusCode, body: &str) -> String {
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
        Some(message) => format!("Microsoft Graph returned {status}: {message}"),
        None => format!("Microsoft Graph returned {status}"),
    }
}

fn drive_item_url(account_id: &str, item_id: &str) -> Result<String, String> {
    let mut url = Url::parse(&format!("{GRAPH_ROOT}/")).map_err(|err| err.to_string())?;
    url.path_segments_mut()
        .map_err(|_| "Could not build Microsoft Graph item URL".to_string())?
        .extend(["drives", account_id, "items", item_id]);
    Ok(url.to_string())
}

fn remove_cached_subtree(cache: &mut OneDriveCache, root_id: &str) {
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

fn wait_for_oauth_callback(listener: TcpListener, expected_state: &str) -> Result<String, String> {
    listener
        .set_nonblocking(true)
        .map_err(|err| err.to_string())?;
    let deadline = Instant::now() + CALLBACK_TIMEOUT;
    while Instant::now() < deadline {
        if OAUTH_CANCELLED.load(Ordering::SeqCst) {
            return Err("OneDrive connection cancelled.".to_string());
        }
        match listener.accept() {
            Ok((mut stream, _)) => {
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .map_err(|err| err.to_string())?;
                let mut buffer = [0_u8; 8192];
                let count = stream.read(&mut buffer).map_err(|err| err.to_string())?;
                let request = String::from_utf8_lossy(&buffer[..count]);
                let target = request
                    .lines()
                    .next()
                    .and_then(|line| line.split_whitespace().nth(1))
                    .ok_or_else(|| "Invalid OAuth callback".to_string())?;
                let callback = Url::parse(&format!("http://localhost{target}"))
                    .map_err(|err| err.to_string())?;
                let params: HashMap<String, String> = callback.query_pairs().into_owned().collect();

                if let Some(error) = params.get("error") {
                    let description = params
                        .get("error_description")
                        .cloned()
                        .unwrap_or_else(|| error.clone());
                    write_callback_response(
                        &mut stream,
                        "OneDrive connection cancelled",
                        "You can close this browser tab and return to DuckDisk.",
                    )?;
                    return Err(description);
                }
                if params.get("state").map(String::as_str) != Some(expected_state) {
                    return Err("Microsoft sign-in state did not match".to_string());
                }
                let code = params.get("code").cloned().ok_or_else(|| {
                    "Microsoft sign-in returned no authorization code".to_string()
                })?;
                write_callback_response(
                    &mut stream,
                    "OneDrive connected",
                    "You can close this browser tab and return to DuckDisk.",
                )?;
                return Ok(code);
            }
            Err(err) if err.kind() == ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(err) => return Err(err.to_string()),
        }
    }
    Err("Microsoft sign-in timed out".to_string())
}

fn write_callback_response(
    stream: &mut std::net::TcpStream,
    title: &str,
    message: &str,
) -> Result<(), String> {
    let body = format!(
        "<!doctype html><meta charset=\"utf-8\"><title>{title}</title>\
         <style>body{{font:16px system-ui;background:#0f172a;color:#e2e8f0;\
         display:grid;place-items:center;height:100vh;margin:0}}main{{text-align:center}}\
         h1{{font-size:24px}}</style><main><h1>{title}</h1><p>{message}</p></main>"
    );
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    stream
        .write_all(response.as_bytes())
        .map_err(|err| err.to_string())
}

fn oauth_client(client_id: &str, redirect: Option<&str>) -> Result<BasicClient, String> {
    let client = BasicClient::new(
        ClientId::new(client_id.to_string()),
        None,
        AuthUrl::new("https://login.microsoftonline.com/common/oauth2/v2.0/authorize".to_string())
            .map_err(|err| err.to_string())?,
        Some(
            TokenUrl::new("https://login.microsoftonline.com/common/oauth2/v2.0/token".to_string())
                .map_err(|err| err.to_string())?,
        ),
    );
    match redirect {
        Some(redirect) => Ok(client.set_redirect_uri(
            RedirectUrl::new(redirect.to_string()).map_err(|err| err.to_string())?,
        )),
        None => Ok(client),
    }
}

fn client_id() -> String {
    env!("DUCKDISK_ONEDRIVE_CLIENT_ID").trim().to_string()
}

fn required_client_id() -> Result<String, String> {
    let client_id = client_id();
    if client_id.is_empty() {
        Err(
            "OneDrive is not configured in this build. Set DUCKDISK_ONEDRIVE_CLIENT_ID and rebuild."
                .to_string(),
        )
    } else {
        Ok(client_id)
    }
}

fn store_credential(account_id: &str, refresh_token: &str, can_delete: bool) -> Result<(), String> {
    let entry = keyring::Entry::new(KEYCHAIN_SERVICE, account_id).map_err(|err| err.to_string())?;
    let value = serde_json::to_string(&StoredCredential {
        refresh_token: refresh_token.to_string(),
        can_delete,
    })
    .map_err(|err| err.to_string())?;
    entry.set_password(&value).map_err(|err| err.to_string())
}

fn read_credential(account_id: &str) -> Result<StoredCredential, String> {
    let entry = keyring::Entry::new(KEYCHAIN_SERVICE, account_id).map_err(|err| err.to_string())?;
    let value = entry
        .get_password()
        .map_err(|err| format!("OneDrive sign-in is missing from Keychain: {err}"))?;
    serde_json::from_str(&value).map_err(|err| err.to_string())
}

fn account_from_drive(drive: &GraphDrive) -> OneDriveAccount {
    let quota = drive.quota.as_ref();
    let total = quota.and_then(|quota| quota.total).unwrap_or_default();
    let used = quota.and_then(|quota| quota.used).unwrap_or_else(|| {
        total.saturating_sub(quota.and_then(|quota| quota.remaining).unwrap_or(total))
    });
    OneDriveAccount {
        id: drive.id.clone(),
        name: drive
            .owner
            .as_ref()
            .and_then(|owner| owner.user.as_ref())
            .and_then(|user| user.display_name.clone())
            .unwrap_or_else(|| "Microsoft account".to_string()),
        drive_type: drive
            .drive_type
            .clone()
            .unwrap_or_else(|| "OneDrive".to_string()),
        total_space: total,
        used_space: used,
        available_space: quota
            .and_then(|quota| quota.remaining)
            .unwrap_or_else(|| total.saturating_sub(used)),
    }
}

fn accounts_path(app_handle: &tauri::AppHandle) -> Result<PathBuf, String> {
    app_handle
        .path_resolver()
        .app_config_dir()
        .map(|path| path.join("onedrive-accounts.json"))
        .ok_or_else(|| "Could not resolve DuckDisk configuration directory".to_string())
}

fn read_accounts(app_handle: &tauri::AppHandle) -> Result<Vec<OneDriveAccount>, String> {
    let path = accounts_path(app_handle)?;
    if !path.exists() {
        return Ok(Vec::new());
    }
    let content = fs::read_to_string(path).map_err(|err| err.to_string())?;
    serde_json::from_str(&content).map_err(|err| err.to_string())
}

fn write_accounts(
    app_handle: &tauri::AppHandle,
    accounts: &[OneDriveAccount],
) -> Result<(), String> {
    let path = accounts_path(app_handle)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| err.to_string())?;
    }
    let content = serde_json::to_string_pretty(accounts).map_err(|err| err.to_string())?;
    fs::write(path, content).map_err(|err| err.to_string())
}

fn upsert_account(app_handle: &tauri::AppHandle, account: OneDriveAccount) -> Result<(), String> {
    let mut accounts = read_accounts(app_handle)?;
    if let Some(existing) = accounts
        .iter_mut()
        .find(|existing| existing.id == account.id)
    {
        *existing = account;
    } else {
        accounts.push(account);
    }
    write_accounts(app_handle, &accounts)
}

fn cache_path(app_handle: &tauri::AppHandle, account_id: &str) -> Result<PathBuf, String> {
    let mut hasher = DefaultHasher::new();
    account_id.hash(&mut hasher);
    let key = hasher.finish();
    app_handle
        .path_resolver()
        .app_cache_dir()
        .map(|path| path.join("onedrive").join(format!("{key:016x}.json")))
        .ok_or_else(|| "Could not resolve DuckDisk cache directory".to_string())
}

fn read_cache(path: &Path, account_id: &str) -> Option<OneDriveCache> {
    let content = fs::read_to_string(path).ok()?;
    let cache: OneDriveCache = serde_json::from_str(&content).ok()?;
    (cache.version == CACHE_VERSION
        && cache.account_id == account_id
        && (!cache.delta_link.is_empty() || !cache.next_link.is_empty()))
    .then_some(cache)
}

fn write_cache(path: &Path, cache: &OneDriveCache) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| err.to_string())?;
    }
    let content = serde_json::to_string(cache).map_err(|err| err.to_string())?;
    let temporary_path = path.with_extension("json.tmp");
    fs::write(&temporary_path, content).map_err(|err| err.to_string())?;
    fs::rename(temporary_path, path).map_err(|err| err.to_string())
}

fn initial_delta_url() -> String {
    format!(
        "{GRAPH_ROOT}/me/drive/root/delta?$select=id,name,size,parentReference,folder,file,deleted"
    )
}

fn write_scan_result(content: &str) -> Result<PathBuf, String> {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let path = std::env::temp_dir().join(format!(
        "duckdisk-onedrive-scan-{}-{timestamp}.json",
        std::process::id()
    ));
    fs::write(&path, content).map_err(|err| err.to_string())?;
    Ok(path)
}

fn emit_scan_status(app_handle: &tauri::AppHandle, account_id: &str, items: u64, total: u64) {
    if let Some(scan) = ACTIVE_SCANS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get_mut(account_id)
    {
        scan.items = items;
        scan.total = total;
    }
    app_handle
        .emit_all(
            "onedrive_scan_status",
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

fn emit_scan_phase(app_handle: &tauri::AppHandle, account_id: &str, phase: ActiveScanPhase) {
    if let Some(scan) = ACTIVE_SCANS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get_mut(account_id)
    {
        scan.phase = Some(phase);
    }
    app_handle
        .emit_all(
            scan_phase_event(phase),
            AccountPayload {
                account_id: account_id.to_string(),
            },
        )
        .ok();
}

fn replay_active_scan(app_handle: &tauri::AppHandle, account_id: &str, scan: &ActiveScan) {
    if let Some(phase) = scan.phase {
        app_handle
            .emit_all(
                scan_phase_event(phase),
                AccountPayload {
                    account_id: account_id.to_string(),
                },
            )
            .ok();
    }
    emit_scan_status(app_handle, account_id, scan.items, scan.total);
}

fn ensure_scan_active(account_id: &str) -> Result<(), String> {
    let cancelled = ACTIVE_SCANS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(account_id)
        .map(|scan| scan.cancelled)
        .unwrap_or(true);
    if cancelled {
        Err(SCAN_CANCELLED_MESSAGE.to_string())
    } else {
        Ok(())
    }
}

fn scan_phase_event(phase: ActiveScanPhase) -> &'static str {
    match phase {
        ActiveScanPhase::Full => "onedrive_scan_full",
        ActiveScanPhase::Incremental => "onedrive_scan_incremental",
        ActiveScanPhase::Finalizing => "onedrive_scan_finalizing",
    }
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
    fn builds_cloud_tree_and_sums_leaf_sizes() {
        let mut items = HashMap::new();
        items.insert("root".to_string(), item("root", None, "root", 999, true));
        items.insert(
            "folder".to_string(),
            item("folder", Some("root"), "Photos", 999, true),
        );
        items.insert(
            "file".to_string(),
            item("file", Some("folder"), "duck.png", 42, false),
        );
        let cache = OneDriveCache {
            version: CACHE_VERSION.to_string(),
            account_id: "drive".to_string(),
            root_id: "root".to_string(),
            delta_link: "delta".to_string(),
            next_link: String::new(),
            checkpoint_items: 0,
            checkpoint_bytes: 0,
            items,
        };
        let account = OneDriveAccount {
            id: "drive".to_string(),
            name: "Qi".to_string(),
            drive_type: "personal".to_string(),
            total_space: 100,
            used_space: 42,
            available_space: 58,
        };

        let result: Value = serde_json::from_str(&build_scan_json(&cache, &account).unwrap())
            .expect("valid result");
        assert_eq!(result["tree"]["size"], 42);
        assert_eq!(result["tree"]["children"][0]["size"], 42);
        assert_eq!(
            result["tree"]["children"][0]["children"][0]["cloudId"],
            "file"
        );
        let folder: Value =
            serde_json::from_str(&build_item_json(&cache, "folder").unwrap()).unwrap();
        assert_eq!(folder["cloudId"], "folder");
        assert_eq!(folder["size"], 42);
        assert_eq!(folder["children"][0]["cloudId"], "file");
    }

    #[test]
    fn delta_delete_removes_cached_item() {
        let mut cache = OneDriveCache {
            version: CACHE_VERSION.to_string(),
            account_id: "drive".to_string(),
            root_id: "root".to_string(),
            delta_link: "delta".to_string(),
            next_link: String::new(),
            checkpoint_items: 0,
            checkpoint_bytes: 0,
            items: HashMap::from([(
                "file".to_string(),
                item("file", Some("root"), "old.txt", 12, false),
            )]),
        };
        apply_delta_item(
            &mut cache,
            GraphItem {
                id: "file".to_string(),
                name: None,
                size: None,
                parent_reference: None,
                folder: None,
                deleted: Some(json!({})),
            },
        );
        assert!(!cache.items.contains_key("file"));
    }

    #[test]
    fn cache_delete_removes_folder_descendants() {
        let mut cache = OneDriveCache {
            version: CACHE_VERSION.to_string(),
            account_id: "drive".to_string(),
            root_id: "root".to_string(),
            delta_link: "delta".to_string(),
            next_link: String::new(),
            checkpoint_items: 0,
            checkpoint_bytes: 0,
            items: HashMap::from([
                ("root".to_string(), item("root", None, "root", 0, true)),
                (
                    "folder".to_string(),
                    item("folder", Some("root"), "folder", 0, true),
                ),
                (
                    "child".to_string(),
                    item("child", Some("folder"), "child.txt", 12, false),
                ),
                (
                    "keep".to_string(),
                    item("keep", Some("root"), "keep.txt", 8, false),
                ),
            ]),
        };

        remove_cached_subtree(&mut cache, "folder");

        assert!(!cache.items.contains_key("folder"));
        assert!(!cache.items.contains_key("child"));
        assert!(cache.items.contains_key("root"));
        assert!(cache.items.contains_key("keep"));
    }

    #[test]
    fn recognizes_delta_resync_errors() {
        assert!(is_delta_resync_error(
            r#"Microsoft Graph returned 410 Gone: {"error":{"code":"resyncRequired"}}"#
        ));
        assert!(is_delta_resync_error(
            r#"{"error":{"code":"resyncChangesApplyDifferences"}}"#
        ));
        assert!(!is_delta_resync_error(
            "Microsoft Graph remained unavailable (429 Too Many Requests)"
        ));
    }

    #[test]
    fn recognizes_authentication_errors() {
        assert!(is_authentication_error(
            r#"Microsoft Graph returned 401 Unauthorized: {"error":{"code":"InvalidAuthenticationToken"}}"#
        ));
        assert!(!is_authentication_error(
            "Microsoft Graph returned 403 Forbidden"
        ));
    }

    #[test]
    fn retries_only_transient_delete_statuses() {
        for status in [408, 409, 423, 429, 500, 502, 503, 504] {
            assert!(is_retryable_delete_status(
                StatusCode::from_u16(status).unwrap()
            ));
        }
        for status in [400, 401, 403, 404] {
            assert!(!is_retryable_delete_status(
                StatusCode::from_u16(status).unwrap()
            ));
        }
    }

    #[test]
    fn extracts_graph_delete_error_message() {
        assert_eq!(
            graph_error_message(
                StatusCode::LOCKED,
                r#"{"error":{"message":"The resource is temporarily locked."}}"#
            ),
            "Microsoft Graph returned 423 Locked: The resource is temporarily locked."
        );
    }

    #[test]
    fn reads_legacy_cache_without_checkpoint_fields() {
        let cache: OneDriveCache = serde_json::from_str(
            r#"{
                "version":"duckdisk-onedrive-cache-v1",
                "accountId":"drive",
                "rootId":"root",
                "deltaLink":"delta",
                "items":{}
            }"#,
        )
        .expect("legacy cache");

        assert!(cache.next_link.is_empty());
        assert_eq!(cache.checkpoint_items, 0);
        assert_eq!(cache.checkpoint_bytes, 0);
    }
}
