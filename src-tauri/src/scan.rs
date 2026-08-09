use std::collections::{hash_map::DefaultHasher, HashSet};
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use once_cell::sync::Lazy;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Mutex;
use tauri::api::process::{Command as TauriCommand, CommandEvent};
use tauri::Manager;

use crate::temp_files::validate_result_file;
use crate::MyState;

static ACTIVE_INCREMENTAL_SCANS: Lazy<Mutex<HashSet<String>>> =
    Lazy::new(|| Mutex::new(HashSet::new()));

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct CompletedPayload {
    path: String,
    errors_path: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Payload {
    items: u64,
    total: u64,
    operation_not_permitted: u64,
    permission_denied: u64,
    interrupted: u64,
    other: u64,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ScanErrorRecord {
    operation: String,
    path: String,
    reason: String,
    kind: String,
}

#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct ScanErrorCounts {
    operation_not_permitted: u64,
    permission_denied: u64,
    interrupted: u64,
    other: u64,
}

#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct ScanErrorReport {
    counts: ScanErrorCounts,
    records: Vec<ScanErrorRecord>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CacheIndex {
    version: String,
    scan_path: String,
    ratio: String,
    children: Vec<CacheIndexEntry>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CacheIndexEntry {
    path: String,
    is_dir: bool,
    modified_ms: u128,
    len: u64,
}

// Start scan
pub fn start(
    app_handle: tauri::AppHandle,
    state: tauri::State<'_, MyState>,
    path: String,
    ratio: String,
    use_cache: bool,
) -> Result<(), ()> {
    println!("Start Scanning {}", path);
    let ratio_arg = ["--min-ratio=", ratio.as_str()].join("");

    if use_cache {
        let scan_key = format!("{path}\n{ratio}");
        let inserted = ACTIVE_INCREMENTAL_SCANS
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(scan_key.clone());
        if !inserted {
            app_handle.emit_all("scan_incremental", ()).ok();
            return Ok(());
        }

        tauri::async_runtime::spawn(async move {
            match incremental_scan(&app_handle, &path, &ratio, &ratio_arg).await {
                Ok((result_path, error_report)) => match write_scan_error_report(&error_report) {
                    Ok(errors_path) => {
                        app_handle
                            .emit_all(
                                "scan_completed",
                                CompletedPayload {
                                    path: result_path.display().to_string(),
                                    errors_path: errors_path.display().to_string(),
                                },
                            )
                            .ok();
                    }
                    Err(err) => {
                        app_handle
                            .emit_all(
                                "scan_failed",
                                format!("Failed to write scan error report: {err}"),
                            )
                            .ok();
                    }
                },
                Err(err) => {
                    app_handle.emit_all("scan_failed", err).ok();
                }
            }
            ACTIVE_INCREMENTAL_SCANS
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .remove(&scan_key);
        });
        return Ok(());
    }

    let mut paths_to_scan = scan_args(&ratio_arg);
    paths_to_scan.extend(scan_targets(&path));

    let progress_regex = Regex::new(
        r"\(scanned ([0-9]+), total ([0-9]+)(?:, linked [0-9]+, shared [0-9]+)?(?:, erred ([0-9]+))?\)",
    )
    .expect("valid progress regex");
    let error_regex = Regex::new(r#"^\[error\] (\S+) "(.+)": (.+)$"#).expect("valid error regex");

    let (mut rx, child) = TauriCommand::new_sidecar("pdu")
        .expect("failed to create `my-sidecar` binary command")
        .args(paths_to_scan)
        .spawn()
        .expect("Failed to spawn sidecar");
    let child_pid = register_child(&state, child);

    // unlisten to the event using the `id` returned on the `listen_global` function
    // an `once_global` API is also exposed on the `App` struct

    tauri::async_runtime::spawn(async move {
        let mut items = 0;
        let mut total = 0;
        let mut error_records: Vec<ScanErrorRecord> = Vec::new();

        while let Some(event) = rx.recv().await {
            match event {
                CommandEvent::Stdout(line) => {
                    app_handle.emit_all("scan_finalizing", ()).ok();
                    let error_report = build_error_report(error_records.clone());
                    let normalized = normalize_pdu_content(&line);
                    match normalized.and_then(|content| {
                        write_scan_result(&content).map_err(|err| err.to_string())
                    }) {
                        Ok(path) => match write_scan_error_report(&error_report) {
                            Ok(errors_path) => {
                                app_handle
                                    .emit_all(
                                        "scan_completed",
                                        CompletedPayload {
                                            path: path.display().to_string(),
                                            errors_path: errors_path.display().to_string(),
                                        },
                                    )
                                    .ok();
                            }
                            Err(err) => {
                                app_handle
                                    .emit_all(
                                        "scan_failed",
                                        format!("Failed to write scan error report: {err}"),
                                    )
                                    .ok();
                            }
                        },
                        Err(err) => {
                            app_handle
                                .emit_all(
                                    "scan_failed",
                                    format!("Failed to write scan result: {err}"),
                                )
                                .ok();
                        }
                    }
                }
                CommandEvent::Stderr(line) => {
                    if let Some(captures) = progress_regex.captures(&line) {
                        items = captures
                            .get(1)
                            .and_then(|matched| matched.as_str().parse::<u64>().ok())
                            .unwrap_or_default();
                        total = captures
                            .get(2)
                            .and_then(|matched| matched.as_str().parse::<u64>().ok())
                            .unwrap_or_default();

                        emit_scan_status(&app_handle, items, total, &error_records);
                    } else if let Some(record) = parse_scan_error(&error_regex, &line) {
                        error_records.push(record);
                        emit_scan_status(&app_handle, items, total, &error_records);
                    }
                }
                CommandEvent::Terminated(t) => {
                    println!("{t:?}");
                    unregister_child(&app_handle.state::<MyState>(), child_pid);
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

fn scan_args(ratio: &str) -> Vec<String> {
    vec![
        "--json-output".to_string(),
        "--quantity=dual-size".to_string(),
        "--progress".to_string(),
        "--deduplicate-hardlinks".to_string(),
        "--omit-json-shared-details".to_string(),
        "--omit-json-shared-summary".to_string(),
        "--threads=max".to_string(),
        ratio.to_string(),
    ]
}

fn emit_scan_status(
    app_handle: &tauri::AppHandle,
    items: u64,
    total: u64,
    records: &[ScanErrorRecord],
) {
    let counts = count_scan_errors(records);
    app_handle
        .emit_all(
            "scan_status",
            Payload {
                items,
                total,
                operation_not_permitted: counts.operation_not_permitted,
                permission_denied: counts.permission_denied,
                interrupted: counts.interrupted,
                other: counts.other,
            },
        )
        .ok();
}

fn parse_scan_error(regex: &Regex, line: &str) -> Option<ScanErrorRecord> {
    let captures = regex.captures(line)?;
    let reason = captures.get(3)?.as_str().to_string();
    Some(ScanErrorRecord {
        operation: captures.get(1)?.as_str().to_string(),
        path: captures.get(2)?.as_str().to_string(),
        kind: classify_scan_error(&reason).to_string(),
        reason,
    })
}

fn classify_scan_error(reason: &str) -> &'static str {
    if reason.contains("Operation not permitted") {
        "operationNotPermitted"
    } else if reason.contains("Permission denied") {
        "permissionDenied"
    } else if reason.contains("Interrupted system call") {
        "interrupted"
    } else {
        "other"
    }
}

fn count_scan_errors(records: &[ScanErrorRecord]) -> ScanErrorCounts {
    let mut counts = ScanErrorCounts::default();
    for record in records {
        match record.kind.as_str() {
            "operationNotPermitted" => counts.operation_not_permitted += 1,
            "permissionDenied" => counts.permission_denied += 1,
            "interrupted" => counts.interrupted += 1,
            _ => counts.other += 1,
        }
    }
    counts
}

fn build_error_report(records: Vec<ScanErrorRecord>) -> ScanErrorReport {
    ScanErrorReport {
        counts: count_scan_errors(&records),
        records,
    }
}

async fn incremental_scan(
    app_handle: &tauri::AppHandle,
    scan_path: &str,
    ratio: &str,
    ratio_arg: &str,
) -> Result<(PathBuf, ScanErrorReport), String> {
    app_handle.emit_all("scan_incremental", ()).ok();

    let cached_path = cache_path(app_handle, scan_path, ratio)?;
    let index_path = cache_index_path(app_handle, scan_path, ratio)?;
    let cached_content = fs::read_to_string(&cached_path).map_err(|err| err.to_string())?;
    let index_content = fs::read_to_string(&index_path).map_err(|err| err.to_string())?;
    let index: CacheIndex = serde_json::from_str(&index_content).map_err(|err| err.to_string())?;
    let mut cached_json: Value =
        serde_json::from_str(&cached_content).map_err(|err| err.to_string())?;

    let current_entries = current_child_entries(scan_path)?;
    let indexed_paths: HashSet<String> = index
        .children
        .iter()
        .map(|entry| entry.path.clone())
        .collect();
    let current_paths: HashSet<String> = current_entries
        .iter()
        .map(|entry| entry.path.clone())
        .collect();

    let mut changed_paths: Vec<String> = current_entries
        .iter()
        .filter(|entry| {
            index
                .children
                .iter()
                .find(|cached| cached.path == entry.path)
                .map(|cached| {
                    cached.modified_ms != entry.modified_ms
                        || cached.len != entry.len
                        || cached.is_dir != entry.is_dir
                })
                .unwrap_or(true)
        })
        .map(|entry| entry.path.clone())
        .collect();

    let removed_paths: HashSet<String> = indexed_paths
        .difference(&current_paths)
        .map(|path| path.to_string())
        .collect();

    let error_report = if changed_paths.is_empty() {
        build_error_report(Vec::new())
    } else {
        let (scan_json, report) = run_pdu_for_paths(app_handle, ratio_arg, &changed_paths).await?;
        merge_changed_children(&mut cached_json, scan_path, &removed_paths, &scan_json)?;
        changed_paths.clear();
        report
    };

    if !removed_paths.is_empty() && changed_paths.is_empty() {
        merge_changed_children(&mut cached_json, scan_path, &removed_paths, &None)?;
    }

    let content = serde_json::to_string(&cached_json).map_err(|err| err.to_string())?;
    write_scan_result(&content)
        .map(|path| (path, error_report))
        .map_err(|err| err.to_string())
}

pub async fn refresh_path(
    app_handle: &tauri::AppHandle,
    scan_path: &str,
    target_path: &str,
    ratio: &str,
) -> Result<String, String> {
    let scan_root = Path::new(scan_path);
    let target = Path::new(target_path);
    if !target.starts_with(scan_root) {
        return Err("The selected item is outside the current scan".to_string());
    }
    if !target.exists() {
        return Err("The selected item no longer exists".to_string());
    }

    let ratio_arg = format!("--min-ratio={ratio}");
    let (scan_json, _) =
        run_pdu_for_paths(app_handle, &ratio_arg, &[target_path.to_string()]).await?;
    let refreshed =
        scan_json.ok_or_else(|| "The selected item could not be scanned".to_string())?;
    let refreshed_content = serde_json::to_string(&refreshed).map_err(|err| err.to_string())?;

    let cached_path = cache_path(app_handle, scan_path, ratio)?;
    if target == scan_root {
        write_cached_result(app_handle, scan_path, ratio, &refreshed_content)?;
        return Ok(refreshed_content);
    }

    if cached_path.exists() {
        let cached_content = fs::read_to_string(&cached_path).map_err(|err| err.to_string())?;
        let mut cached_json: Value =
            serde_json::from_str(&cached_content).map_err(|err| err.to_string())?;
        let refreshed_tree = refreshed
            .get("tree")
            .ok_or_else(|| "Refreshed scan has no tree".to_string())?;
        let cached_tree = cached_json
            .get_mut("tree")
            .ok_or_else(|| "Cached scan has no tree".to_string())?;

        if !replace_cached_subtree(cached_tree, scan_root, target, refreshed_tree) {
            return Err("The selected item is no longer present in the cached scan".to_string());
        }
        recalculate_tree_sizes(cached_tree);
        let merged_content = serde_json::to_string(&cached_json).map_err(|err| err.to_string())?;
        write_cached_result(app_handle, scan_path, ratio, &merged_content)?;
    }

    Ok(refreshed_content)
}

async fn run_pdu_for_paths(
    app_handle: &tauri::AppHandle,
    ratio_arg: &str,
    paths: &[String],
) -> Result<(Option<Value>, ScanErrorReport), String> {
    let mut args = scan_args(ratio_arg);
    args.extend(paths.iter().cloned());

    let progress_regex = Regex::new(
        r"\(scanned ([0-9]+), total ([0-9]+)(?:, linked [0-9]+, shared [0-9]+)?(?:, erred ([0-9]+))?\)",
    )
    .map_err(|err| err.to_string())?;
    let error_regex =
        Regex::new(r#"^\[error\] (\S+) "(.+)": (.+)$"#).map_err(|err| err.to_string())?;

    let (mut rx, child) = TauriCommand::new_sidecar("pdu")
        .map_err(|err| err.to_string())?
        .args(args)
        .spawn()
        .map_err(|err| err.to_string())?;
    let child_pid = register_child(&app_handle.state::<MyState>(), child);

    let mut stdout = None;
    let mut items = 0;
    let mut total = 0;
    let mut error_records = Vec::new();

    while let Some(event) = rx.recv().await {
        match event {
            CommandEvent::Stdout(line) => {
                stdout = Some(line);
            }
            CommandEvent::Stderr(line) => {
                if let Some(captures) = progress_regex.captures(&line) {
                    items = captures
                        .get(1)
                        .and_then(|matched| matched.as_str().parse::<u64>().ok())
                        .unwrap_or_default();
                    total = captures
                        .get(2)
                        .and_then(|matched| matched.as_str().parse::<u64>().ok())
                        .unwrap_or_default();
                    emit_scan_status(app_handle, items, total, &error_records);
                } else if let Some(record) = parse_scan_error(&error_regex, &line) {
                    error_records.push(record);
                    emit_scan_status(app_handle, items, total, &error_records);
                }
            }
            CommandEvent::Terminated(_) => {}
            _ => {}
        }
    }
    unregister_child(&app_handle.state::<MyState>(), child_pid);

    let error_report = build_error_report(error_records);
    let parsed = stdout
        .map(|content| parse_pdu_content(&content))
        .transpose()?;
    Ok((parsed, error_report))
}

fn parse_pdu_content(content: &str) -> Result<Value, String> {
    let mut parsed: Value = serde_json::from_str(content).map_err(|err| err.to_string())?;
    let tree = parsed
        .get_mut("tree")
        .ok_or_else(|| "Scan result has no tree".to_string())?;
    normalize_pdu_tree(tree)?;
    parsed["unit"] = Value::String("bytes".to_string());
    Ok(parsed)
}

fn normalize_pdu_content(content: &str) -> Result<String, String> {
    serde_json::to_string(&parse_pdu_content(content)?).map_err(|err| err.to_string())
}

fn normalize_pdu_tree(node: &mut Value) -> Result<(), String> {
    let size = node
        .get("size")
        .and_then(Value::as_object)
        .ok_or_else(|| "Scan result is missing dual size data".to_string())?;
    let apparent = size
        .get("apparent")
        .and_then(Value::as_u64)
        .ok_or_else(|| "Scan result is missing apparent size data".to_string())?;
    let allocated = size
        .get("allocated")
        .and_then(Value::as_u64)
        .ok_or_else(|| "Scan result is missing allocated size data".to_string())?;

    node["size"] = Value::from(apparent);
    node["allocatedSize"] = Value::from(allocated);

    if let Some(children) = node.get_mut("children").and_then(Value::as_array_mut) {
        for child in children {
            normalize_pdu_tree(child)?;
        }
    }
    Ok(())
}

fn replace_cached_subtree(
    node: &mut Value,
    node_path: &Path,
    target_path: &Path,
    refreshed_tree: &Value,
) -> bool {
    let Some(children) = node.get_mut("children").and_then(Value::as_array_mut) else {
        return false;
    };

    for child in children {
        let Some(name) = child.get("name").and_then(Value::as_str) else {
            continue;
        };
        let child_path = if Path::new(name).is_absolute() {
            PathBuf::from(name)
        } else {
            node_path.join(name)
        };

        if child_path == target_path {
            let preserved_name = child["name"].clone();
            *child = refreshed_tree.clone();
            child["name"] = preserved_name;
            return true;
        }
        if target_path.starts_with(&child_path)
            && replace_cached_subtree(child, &child_path, target_path, refreshed_tree)
        {
            return true;
        }
    }
    false
}

fn recalculate_tree_sizes(node: &mut Value) -> (u64, u64) {
    let child_sizes = node
        .get_mut("children")
        .and_then(Value::as_array_mut)
        .map(|children| {
            children
                .iter_mut()
                .map(recalculate_tree_sizes)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    if child_sizes.is_empty() {
        return (tree_size(node), tree_allocated_size(node));
    }

    let (size, allocated_size) = child_sizes.into_iter().fold(
        (0_u64, 0_u64),
        |(size, allocated), (child_size, child_allocated)| {
            (size + child_size, allocated + child_allocated)
        },
    );
    node["size"] = Value::from(size);
    node["allocatedSize"] = Value::from(allocated_size);
    (size, allocated_size)
}

fn merge_changed_children(
    cached_json: &mut Value,
    scan_path: &str,
    removed_paths: &HashSet<String>,
    changed_json: &Option<Value>,
) -> Result<(), String> {
    let root = cached_json
        .get_mut("tree")
        .ok_or_else(|| "Cached scan has no tree".to_string())?;
    let root_name = root
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let total_root = root_name == "(total)";
    let children = root
        .get_mut("children")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| "Cached scan tree has no children".to_string())?;

    children.retain(|child| {
        child_path(scan_path, total_root, child)
            .map(|path| !removed_paths.contains(&path))
            .unwrap_or(true)
    });

    if let Some(changed_json) = changed_json {
        for mut changed_child in top_level_nodes(changed_json) {
            let path = changed_child
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            if path.is_empty() {
                continue;
            }

            if !total_root {
                if let Some(name) = Path::new(&path).file_name().and_then(|name| name.to_str()) {
                    changed_child["name"] = Value::String(name.to_string());
                }
            }

            if let Some(index) = children.iter().position(|child| {
                child_path(scan_path, total_root, child)
                    .map(|child_path| child_path == path)
                    .unwrap_or(false)
            }) {
                children[index] = changed_child;
            } else {
                children.push(changed_child);
            }
        }
    }

    let size = children.iter().map(tree_size).sum::<u64>();
    let allocated_size = children.iter().map(tree_allocated_size).sum::<u64>();
    root["size"] = Value::from(size);
    root["allocatedSize"] = Value::from(allocated_size);
    Ok(())
}

fn top_level_nodes(scan_json: &Value) -> Vec<Value> {
    let Some(tree) = scan_json.get("tree") else {
        return Vec::new();
    };
    let name = tree.get("name").and_then(Value::as_str).unwrap_or_default();
    if name == "(total)" {
        tree.get("children")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    } else {
        vec![tree.clone()]
    }
}

fn tree_size(node: &Value) -> u64 {
    node.get("size").and_then(Value::as_u64).unwrap_or_default()
}

fn tree_allocated_size(node: &Value) -> u64 {
    node.get("allocatedSize")
        .and_then(Value::as_u64)
        .unwrap_or_else(|| tree_size(node))
}

fn child_path(scan_path: &str, total_root: bool, child: &Value) -> Option<String> {
    let name = child.get("name")?.as_str()?;
    if total_root || name.starts_with('/') {
        Some(name.to_string())
    } else if scan_path == "/" {
        Some(format!("/{name}"))
    } else {
        Some(Path::new(scan_path).join(name).display().to_string())
    }
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

    fs::read_to_string(path)
        .map(Some)
        .map_err(|err| err.to_string())
}

pub fn has_cached_index(
    app_handle: &tauri::AppHandle,
    scan_path: &str,
    ratio: &str,
) -> Result<bool, String> {
    Ok(cache_index_path(app_handle, scan_path, ratio)?.exists())
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
    let prefix = format!("duckdisk-scan-{}-", std::process::id());
    let path = validate_result_file(&path, &prefix)?;

    let content = fs::read_to_string(&path).map_err(|err| err.to_string())?;
    fs::remove_file(path).ok();
    write_cached_result(&app_handle, &scan_path, &ratio, &content)?;
    Ok(content)
}

pub fn read_error_report(path: String) -> Result<String, String> {
    let prefix = format!("duckdisk-scan-errors-{}-", std::process::id());
    let path = validate_result_file(&path, &prefix)?;

    let content = fs::read_to_string(&path).map_err(|err| err.to_string())?;
    fs::remove_file(path).ok();
    Ok(content)
}

fn register_child(state: &MyState, child: tauri::api::process::CommandChild) -> u32 {
    let pid = child.pid();
    state
        .0
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .push(child);
    pid
}

fn unregister_child(state: &MyState, pid: u32) {
    state
        .0
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .retain(|child| child.pid() != pid);
}

pub fn stop_all(state: &MyState) {
    let children = {
        let mut children = state
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        std::mem::take(&mut *children)
    };
    for child in children {
        child.kill().ok();
    }
}

pub fn stop(state: tauri::State<'_, MyState>) {
    stop_all(&state);
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

fn write_scan_error_report(report: &ScanErrorReport) -> std::io::Result<PathBuf> {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let path = std::env::temp_dir().join(format!(
        "duckdisk-scan-errors-{}-{timestamp}.json",
        std::process::id()
    ));
    let content = serde_json::to_string(report)?;
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
    fs::write(path, content).map_err(|err| err.to_string())?;
    write_cache_index(app_handle, scan_path, ratio, content).ok();
    Ok(())
}

fn cache_path(
    app_handle: &tauri::AppHandle,
    scan_path: &str,
    ratio: &str,
) -> Result<PathBuf, String> {
    let mut hasher = DefaultHasher::new();
    "pdu-0.23.0-dual-size-v2-dataless".hash(&mut hasher);
    scan_path.hash(&mut hasher);
    ratio.hash(&mut hasher);
    let key = hasher.finish();
    let cache_dir = app_handle
        .path_resolver()
        .app_cache_dir()
        .ok_or_else(|| "Could not resolve app cache directory".to_string())?;
    Ok(cache_dir.join("scans").join(format!("{key:016x}.json")))
}

fn cache_index_path(
    app_handle: &tauri::AppHandle,
    scan_path: &str,
    ratio: &str,
) -> Result<PathBuf, String> {
    Ok(cache_path(app_handle, scan_path, ratio)?.with_extension("index.json"))
}

fn write_cache_index(
    app_handle: &tauri::AppHandle,
    scan_path: &str,
    ratio: &str,
    content: &str,
) -> Result<(), String> {
    let parsed: Value = serde_json::from_str(content).map_err(|err| err.to_string())?;
    let Some(tree) = parsed.get("tree") else {
        return Ok(());
    };
    let total_root = tree
        .get("name")
        .and_then(Value::as_str)
        .map(|name| name == "(total)")
        .unwrap_or(false);
    let children = tree
        .get("children")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let entries = children
        .iter()
        .filter_map(|child| child_path(scan_path, total_root, child))
        .filter_map(|path| metadata_entry(&path))
        .collect();
    let index = CacheIndex {
        version: "duckdisk-cache-index-v3-dataless".to_string(),
        scan_path: scan_path.to_string(),
        ratio: ratio.to_string(),
        children: entries,
    };
    let path = cache_index_path(app_handle, scan_path, ratio)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| err.to_string())?;
    }
    let content = serde_json::to_string(&index).map_err(|err| err.to_string())?;
    fs::write(path, content).map_err(|err| err.to_string())
}

fn current_child_entries(scan_path: &str) -> Result<Vec<CacheIndexEntry>, String> {
    let paths = if scan_path == "/" {
        scan_targets(scan_path)
    } else {
        fs::read_dir(scan_path)
            .map_err(|err| err.to_string())?
            .filter_map(Result::ok)
            .filter_map(|entry| entry.path().to_str().map(str::to_string))
            .collect()
    };

    Ok(paths
        .iter()
        .filter_map(|path| metadata_entry(path))
        .collect())
}

fn metadata_entry(path: &str) -> Option<CacheIndexEntry> {
    let metadata = fs::metadata(path).ok()?;
    Some(CacheIndexEntry {
        path: path.to_string(),
        is_dir: metadata.is_dir(),
        modified_ms: metadata
            .modified()
            .ok()
            .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
            .map(|duration| duration.as_millis())
            .unwrap_or_default(),
        len: metadata.len(),
    })
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn replaces_nested_cached_subtree_and_recalculates_sizes() {
        let mut root = json!({
            "name": "(total)",
            "size": 15,
            "allocatedSize": 24,
            "children": [{
                "name": "/Users",
                "size": 10,
                "allocatedSize": 16,
                "children": [{
                    "name": "qi",
                    "size": 10,
                    "allocatedSize": 16,
                    "children": []
                }]
            }, {
                "name": "/Applications",
                "size": 5,
                "allocatedSize": 8,
                "children": []
            }]
        });
        let refreshed = json!({
            "name": "/Users/qi",
            "size": 30,
            "allocatedSize": 40,
            "children": []
        });

        assert!(replace_cached_subtree(
            &mut root,
            Path::new("/"),
            Path::new("/Users/qi"),
            &refreshed
        ));
        assert_eq!(root["children"][0]["children"][0]["name"], "qi");
        assert_eq!(recalculate_tree_sizes(&mut root), (35, 48));
        assert_eq!(root["size"], 35);
        assert_eq!(root["allocatedSize"], 48);
    }

    #[test]
    fn normalizes_dual_size_output() {
        let content = json!({
            "unit": "dual-bytes",
            "tree": {
                "name": "/tmp/sample",
                "size": { "allocated": 4096, "apparent": 5 },
                "children": []
            }
        })
        .to_string();

        let parsed = parse_pdu_content(&content).expect("dual size output should normalize");
        assert_eq!(parsed["unit"], "bytes");
        assert_eq!(parsed["tree"]["size"], 5);
        assert_eq!(parsed["tree"]["allocatedSize"], 4096);
    }
}
