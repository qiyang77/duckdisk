mod scan;
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
