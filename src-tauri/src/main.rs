mod google_drive;
mod onedrive;
mod scan;
mod ssh;
mod updates;
mod window_style;

use serde::Serialize;
use std::process::Command;
use std::sync::Mutex;
use sysinfo::{DiskExt, System, SystemExt};
use tauri::api::process::CommandChild;
use tauri::Manager;
use window_vibrancy::NSVisualEffectMaterial;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DuckDisk<'a> {
    name: &'a str,
    s_mount_point: String,
    total_space: u64,
    available_space: u64,
    is_removable: bool,
}

fn main() {
    tauri::Builder::default()
        .manage(MyState(Default::default()))
        .setup(|app| {
            let window = app.get_window("main").unwrap();
            // window.open_devtools();
            window_vibrancy::apply_vibrancy(&window, NSVisualEffectMaterial::HudWindow, None, None)
                .expect("Error applying blurred bg");

            window_style::set_window_styles(&window).unwrap();

            // app.listen_global("scan_stop", |event| {
            //     let s = app.state::<MyState>();
            //     s.0.lock().unwrap().take().unwrap().kill();
            // });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_disks,
            start_scanning,
            stop_scanning,
            read_scan_result,
            read_scan_error_report,
            read_cached_scan_result,
            has_cached_scan_index,
            clear_cached_scan_result,
            refresh_scan_path,
            get_onedrive_state,
            connect_onedrive_account,
            disconnect_onedrive_account,
            start_onedrive_scan,
            read_onedrive_scan_result,
            refresh_onedrive_item,
            delete_onedrive_items,
            get_google_drive_state,
            connect_google_drive_account,
            disconnect_google_drive_account,
            revoke_google_drive_account,
            start_google_drive_scan,
            read_google_drive_scan_result,
            delete_google_drive_items,
            get_ssh_connections,
            save_ssh_connection,
            remove_ssh_connection,
            start_ssh_scan,
            read_ssh_scan_result,
            check_for_updates,
            open_full_disk_access_settings,
            show_in_folder
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[tauri::command]
fn show_in_folder(path: String) {
    Command::new("open").args(["-R", &path]).spawn().unwrap();
}

#[tauri::command]
fn open_full_disk_access_settings() -> Result<(), String> {
    Command::new("open")
        .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles")
        .spawn()
        .map_err(|err| err.to_string())?;
    Ok(())
}

// Learn more about Tauri commands at https://tauri.app/v1/guides/features/command
#[tauri::command]
fn get_disks() -> String {
    let mut sys = System::new_all();
    sys.refresh_all();

    let mut vec: Vec<DuckDisk> = Vec::new();

    for disk in sys.disks() {
        vec.push(DuckDisk {
            name: disk.name().to_str().unwrap(),
            s_mount_point: disk.mount_point().display().to_string(),
            total_space: disk.total_space(),
            available_space: disk.available_space(),
            is_removable: disk.is_removable(),
        });
    }
    serde_json::to_string(&vec).unwrap().into()
}

pub struct MyState(Mutex<Option<CommandChild>>);

#[tauri::command]
fn start_scanning(
    app_handle: tauri::AppHandle,
    state: tauri::State<'_, MyState>,
    path: String,
    ratio: String,
    use_cache: bool,
) -> Result<(), ()> {
    scan::start(app_handle, state, path, ratio, use_cache)
}

#[tauri::command]
fn stop_scanning(
    _app_handle: tauri::AppHandle,
    state: tauri::State<'_, MyState>,
    _path: String,
) -> Result<(), ()> {
    scan::stop(state);
    Ok(())
}

#[tauri::command]
fn read_scan_result(
    app_handle: tauri::AppHandle,
    path: String,
    scan_path: String,
    ratio: String,
) -> Result<String, String> {
    scan::read_result(app_handle, path, scan_path, ratio)
}

#[tauri::command]
fn read_scan_error_report(path: String) -> Result<String, String> {
    scan::read_error_report(path)
}

#[tauri::command]
fn read_cached_scan_result(
    app_handle: tauri::AppHandle,
    scan_path: String,
    ratio: String,
) -> Result<Option<String>, String> {
    scan::read_cached_result(app_handle, scan_path, ratio)
}

#[tauri::command]
fn has_cached_scan_index(
    app_handle: tauri::AppHandle,
    scan_path: String,
    ratio: String,
) -> Result<bool, String> {
    scan::has_cached_index(&app_handle, &scan_path, &ratio)
}

#[tauri::command]
fn clear_cached_scan_result(
    app_handle: tauri::AppHandle,
    scan_path: String,
    ratio: String,
) -> Result<(), String> {
    scan::clear_cached_result(app_handle, scan_path, ratio)
}

#[tauri::command]
async fn refresh_scan_path(
    app_handle: tauri::AppHandle,
    scan_path: String,
    target_path: String,
    ratio: String,
) -> Result<String, String> {
    scan::refresh_path(&app_handle, &scan_path, &target_path, &ratio).await
}

#[tauri::command]
fn get_onedrive_state(app_handle: tauri::AppHandle) -> Result<onedrive::OneDriveState, String> {
    onedrive::get_state(&app_handle)
}

#[tauri::command]
async fn connect_onedrive_account(
    app_handle: tauri::AppHandle,
) -> Result<onedrive::OneDriveAccount, String> {
    onedrive::connect_account(&app_handle).await
}

#[tauri::command]
fn disconnect_onedrive_account(
    app_handle: tauri::AppHandle,
    account_id: String,
) -> Result<(), String> {
    onedrive::disconnect_account(&app_handle, &account_id)
}

#[tauri::command]
fn start_onedrive_scan(
    app_handle: tauri::AppHandle,
    account_id: String,
    force_full: bool,
) -> Result<(), String> {
    onedrive::start_scan(app_handle, account_id, force_full)
}

#[tauri::command]
fn read_onedrive_scan_result(path: String) -> Result<String, String> {
    onedrive::read_scan_result(&path)
}

#[tauri::command]
async fn refresh_onedrive_item(
    app_handle: tauri::AppHandle,
    account_id: String,
    item_id: String,
) -> Result<String, String> {
    onedrive::refresh_item(&app_handle, &account_id, &item_id).await
}

#[tauri::command]
async fn delete_onedrive_items(
    app_handle: tauri::AppHandle,
    account_id: String,
    item_ids: Vec<String>,
) -> Result<onedrive::OneDriveDeleteResult, String> {
    onedrive::delete_items(&app_handle, &account_id, item_ids).await
}

#[tauri::command]
fn get_google_drive_state(
    app_handle: tauri::AppHandle,
) -> Result<google_drive::GoogleDriveState, String> {
    google_drive::get_state(&app_handle)
}

#[tauri::command]
async fn connect_google_drive_account(
    app_handle: tauri::AppHandle,
) -> Result<google_drive::GoogleDriveAccount, String> {
    google_drive::connect_account(&app_handle).await
}

#[tauri::command]
fn disconnect_google_drive_account(
    app_handle: tauri::AppHandle,
    account_id: String,
) -> Result<(), String> {
    google_drive::disconnect_account(&app_handle, &account_id)
}

#[tauri::command]
async fn revoke_google_drive_account(
    app_handle: tauri::AppHandle,
    account_id: String,
) -> Result<(), String> {
    google_drive::revoke_account(&app_handle, &account_id).await
}

#[tauri::command]
fn start_google_drive_scan(
    app_handle: tauri::AppHandle,
    account_id: String,
    force_full: bool,
) -> Result<(), String> {
    google_drive::start_scan(app_handle, account_id, force_full)
}

#[tauri::command]
fn read_google_drive_scan_result(path: String) -> Result<String, String> {
    google_drive::read_scan_result(&path)
}

#[tauri::command]
async fn delete_google_drive_items(
    app_handle: tauri::AppHandle,
    account_id: String,
    item_ids: Vec<String>,
) -> Result<google_drive::GoogleDriveDeleteResult, String> {
    google_drive::trash_items(&app_handle, &account_id, item_ids).await
}

#[tauri::command]
fn get_ssh_connections(app_handle: tauri::AppHandle) -> Result<Vec<ssh::SshConnection>, String> {
    ssh::get_connections(&app_handle)
}

#[tauri::command]
fn save_ssh_connection(
    app_handle: tauri::AppHandle,
    connection: ssh::SshConnectionInput,
) -> Result<ssh::SshConnection, String> {
    ssh::save_connection(&app_handle, connection)
}

#[tauri::command]
fn remove_ssh_connection(
    app_handle: tauri::AppHandle,
    connection_id: String,
) -> Result<(), String> {
    ssh::remove_connection(&app_handle, &connection_id)
}

#[tauri::command]
fn start_ssh_scan(
    app_handle: tauri::AppHandle,
    connection_id: String,
) -> Result<(), String> {
    ssh::start_scan(app_handle, connection_id)
}

#[tauri::command]
fn read_ssh_scan_result(path: String) -> Result<String, String> {
    ssh::read_scan_result(&path)
}

#[tauri::command]
async fn check_for_updates() -> Result<updates::UpdateCheck, String> {
    updates::check(env!("CARGO_PKG_VERSION")).await
}
