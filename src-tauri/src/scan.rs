use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use regex::Regex;
use tauri::api::process::{Command as TauriCommand, CommandEvent};
use tauri::Manager;

use crate::MyState;

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct CompletedPayload {
    path: String,
}

#[derive(Clone, serde::Serialize)]
struct Payload {
    items: u64,
    total: u64,
    errors: u64,
}

// Start scan
pub fn start(
    app_handle: tauri::AppHandle,
    state: tauri::State<'_, MyState>,
    path: String,
    ratio: String,
) -> Result<(), ()> {
    println!("Start Scanning {}", path);
    let ratio = ["--min-ratio=", ratio.as_str()].join("");

    let mut paths_to_scan: Vec<String> = vec![
        "--json-output".to_string(),
        "--progress".to_string(),
        "--deduplicate-hardlinks".to_string(),
        "--omit-json-shared-details".to_string(),
        "--omit-json-shared-summary".to_string(),
        "--silent-errors".to_string(),
        "--threads=max".to_string(),
        ratio,
    ];
    paths_to_scan.extend(scan_targets(&path));

    let progress_regex = Regex::new(
        r"\(scanned ([0-9]+), total ([0-9]+)(?:, linked [0-9]+, shared [0-9]+)?(?:, erred ([0-9]+))?\)",
    )
    .expect("valid progress regex");

    let (mut rx, child) = TauriCommand::new_sidecar("pdu")
        .expect("failed to create `my-sidecar` binary command")
        .args(paths_to_scan)
        .spawn()
        .expect("Failed to spawn sidecar");
    
    *state.0.lock().unwrap() = Some(child);

    // unlisten to the event using the `id` returned on the `listen_global` function
    // an `once_global` API is also exposed on the `App` struct

    tauri::async_runtime::spawn(async move {
        while let Some(event) = rx.recv().await {
            match event {
                CommandEvent::Stdout(line) => {
                    app_handle.emit_all("scan_finalizing", ()).ok();
                    match write_scan_result(&line) {
                        Ok(path) => {
                            app_handle
                                .emit_all(
                                    "scan_completed",
                                    CompletedPayload {
                                        path: path.display().to_string(),
                                    },
                                )
                                .ok();
                        }
                        Err(err) => {
                            app_handle
                                .emit_all("scan_failed", format!("Failed to write scan result: {err}"))
                                .ok();
                        }
                    }
                }
                CommandEvent::Stderr(line) => {
                    if let Some(captures) = progress_regex.captures(&line) {
                        let items = captures
                            .get(1)
                            .and_then(|matched| matched.as_str().parse::<u64>().ok())
                            .unwrap_or_default();
                        let total = captures
                            .get(2)
                            .and_then(|matched| matched.as_str().parse::<u64>().ok())
                            .unwrap_or_default();
                        let errors = captures
                            .get(3)
                            .and_then(|matched| matched.as_str().parse::<u64>().ok())
                            .unwrap_or_default();

                        app_handle
                            .emit_all(
                                "scan_status",
                                Payload {
                                    items,
                                    total,
                                    errors,
                                },
                            )
                            .ok();
                    }
                }
                CommandEvent::Terminated(t) => {
                    println!("{t:?}");
                    // app_handle.unlisten(id);
                    // child.kill();
                }
                _ => unimplemented!(),
            };
            // if let CommandEvent::Stdout(line) = event {
            //     println!("StdErr: {}", line);
            // } else {
            //     println!("Terminated {}", event);
            // }
            // if let CommandEvent::Stderr(line) = event {
            //     println!("StdErr: {}", line);
            // }
            // if let CommandEvent::Terminated(line) = event {
            //     println!("Terminated");
            // }
        }
        Result::<(), ()>::Ok(())
    });

    Ok(())
    // thread::spawn(move || {
    //     let path = PathBuf::from(path);
    //     let mut vec: Vec<PathBuf> = Vec::new();
    //     vec.push(path);

    //     fn progress_and_error_reporter<Data>(
    //         app_handle: tauri::AppHandle,
    //     ) -> ProgressAndErrorReporter<Data, fn(ErrorReport)>
    //     where
    //         Data: Size + Into<u64> + Send + Sync,
    //         ProgressReport<Data>: Default + 'static,
    //         u64: Into<Data>,
    //     {
    //         let progress_reporter = move |report: ProgressReport<Data>| {
    //             let ProgressReport {
    //                 items,
    //                 total,
    //                 errors,
    //             } = report;
    //             let mut text = String::new();
    //             write!(
    //                 text,
    //                 "\r(scanned {items}, total {total}",
    //                 items = items,
    //                 total = total.into(),
    //             )
    //             .unwrap();
    //             if errors != 0 {
    //                 write!(text, ", erred {}", errors).unwrap();
    //             }
    //             write!(text, ")").unwrap();
    //             println!("{}", text);
    //             app_handle
    //                 .emit_all(
    //                     "scan_status",
    //                     Payload {
    //                         items: items,
    //                         total: total.into(),
    //                         errors: errors,
    //                     },
    //                 )
    //                 .unwrap();
    //         };

    //         struct TextReport<'a>(ErrorReport<'a>);

    //         impl<'a> Display for TextReport<'a> {
    //             fn fmt(&self, formatter: &mut Formatter<'_>) -> Result<(), Error> {
    //                 write!(
    //                     formatter,
    //                     "[error] {operation} {path:?}: {error}",
    //                     operation = self.0.operation.name(),
    //                     path = self.0.path,
    //                     error = self.0.error,
    //                 )
    //             }
    //         }

    //         let error_reporter: fn(ErrorReport) = |report| {
    //             let message = TextReport(report).to_string();
    //             println!("{}", message);
    //         };

    //         ProgressAndErrorReporter::new(
    //             progress_reporter,
    //             Duration::from_millis(100),
    //             error_reporter,
    //         )
    //     }
    //     // pub struct MyReporter {}
    //     // impl parallel_disk_usage::reporter::progress_and_error_reporter
    //     let pdu = parallel_disk_usage::app::Sub {
    //         json_output: true,
    //         direction: Direction::BottomUp,
    //         bar_alignment: BarAlignment::Right,
    //         get_data: GET_APPARENT_SIZE,
    //         files: vec,
    //         no_sort: true,
    //         min_ratio: 0.01.try_into().unwrap(),
    //         max_depth: 10.try_into().unwrap(),
    //         reporter: progress_and_error_reporter(app_handle),
    //         bytes_format: BytesFormat::MetricUnits,
    //         column_width_distribution: ColumnWidthDistribution::total(100),
    //     }
    //     .run();
    // });
}

pub fn read_cached_result(
    app_handle: tauri::AppHandle,
    scan_path: String,
    ratio: String,
) -> Result<Option<String>, String> {
    let path = cache_path(&app_handle, &scan_path, &ratio)?;
    if !path.exists() {
        return Ok(None);
    }

    fs::read_to_string(path).map(Some).map_err(|err| err.to_string())
}

pub fn clear_cached_result(
    app_handle: tauri::AppHandle,
    scan_path: String,
    ratio: String,
) -> Result<(), String> {
    let path = cache_path(&app_handle, &scan_path, &ratio)?;
    if path.exists() {
        fs::remove_file(path).map_err(|err| err.to_string())?;
    }
    Ok(())
}

pub fn read_result(
    app_handle: tauri::AppHandle,
    path: String,
    scan_path: String,
    ratio: String,
) -> Result<String, String> {
    let path = PathBuf::from(path);
    let temp_dir = std::env::temp_dir();

    if !path.starts_with(&temp_dir) {
        return Err("Refusing to read scan result outside the temporary directory".to_string());
    }

    let content = fs::read_to_string(&path).map_err(|err| err.to_string())?;
    fs::remove_file(path).ok();
    write_cached_result(&app_handle, &scan_path, &ratio, &content)?;
    Ok(content)
}

pub fn stop(state: tauri::State<'_, MyState>) {
    if let Some(child) = state.0.lock().unwrap().take() {
        child.kill().ok();
    }
}

fn write_scan_result(content: &str) -> std::io::Result<PathBuf> {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let path = std::env::temp_dir().join(format!(
        "duckdisk-scan-{}-{timestamp}.json",
        std::process::id()
    ));
    fs::write(&path, content)?;
    Ok(path)
}

fn write_cached_result(
    app_handle: &tauri::AppHandle,
    scan_path: &str,
    ratio: &str,
    content: &str,
) -> Result<(), String> {
    let path = cache_path(app_handle, scan_path, ratio)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| err.to_string())?;
    }
    fs::write(path, content).map_err(|err| err.to_string())
}

fn cache_path(
    app_handle: &tauri::AppHandle,
    scan_path: &str,
    ratio: &str,
) -> Result<PathBuf, String> {
    let mut hasher = DefaultHasher::new();
    "pdu-0.23.0".hash(&mut hasher);
    scan_path.hash(&mut hasher);
    ratio.hash(&mut hasher);
    let key = hasher.finish();
    let cache_dir = app_handle
        .path_resolver()
        .app_cache_dir()
        .ok_or_else(|| "Could not resolve app cache directory".to_string())?;
    Ok(cache_dir.join("scans").join(format!("{key:016x}.json")))
}

fn scan_targets(path: &str) -> Vec<String> {
    if path != "/" {
        return vec![path.to_string()];
    }

    let skipped = [
        "/.fseventsd",
        "/.Spotlight-V100",
        "/.Trashes",
        "/.vol",
        "/dev",
        "/home",
        "/net",
        "/Network",
        "/System",
        "/Volumes",
    ];

    fs::read_dir("/")
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .filter_map(|entry| entry.path().to_str().map(str::to_string))
                .filter(|candidate| !skipped.contains(&candidate.as_str()))
                .collect()
        })
        .unwrap_or_else(|_| vec![path.to_string()])
}
