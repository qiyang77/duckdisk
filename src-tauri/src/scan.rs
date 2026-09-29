use std::collections::{hash_map::DefaultHasher, HashMap, HashSet};
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use once_cell::sync::Lazy;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Mutex;
use tauri::{Emitter, Manager};
use tauri_plugin_shell::{process::CommandEvent, ShellExt};

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
    cloud_skipped: u64,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ScanErrorRecord {
    operation: String,
    path: String,
    reason: String,
    kind: String,
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ScanErrorCounts {
    operation_not_permitted: u64,
    permission_denied: u64,
    interrupted: u64,
    other: u64,
    #[serde(default)]
    cloud_skipped: u64,
}

#[derive(Clone, Default, Serialize, Deserialize)]
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

const CACHE_INDEX_VERSION: &str = "duckdisk-cache-index-v6-no-cloud-materialization";
const SCAN_CANCELLED_MESSAGE: &str = "Local scan cancelled.";

impl CacheIndex {
    fn matches_scan(&self, scan_path: &str, ratio: &str) -> bool {
        self.version == CACHE_INDEX_VERSION && self.scan_path == scan_path && self.ratio == ratio
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CacheIndexEntry {
    path: String,
    is_dir: bool,
    modified_ms: u128,
    len: u64,
    #[serde(default)]
    cloud_placeholder: bool,
}

// Start scan
pub fn start(
    app_handle: tauri::AppHandle,
    _state: tauri::State<'_, MyState>,
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
            app_handle.emit("scan_incremental", ()).ok();
            return Ok(());
        }

        tauri::async_runtime::spawn(async move {
            match incremental_scan(&app_handle, &path, &ratio, &ratio_arg).await {
                Ok((result_path, error_report)) => match write_scan_error_report(&error_report) {
                    Ok(errors_path) => {
                        app_handle
                            .emit(
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
                            .emit(
                                "scan_failed",
                                format!("Failed to write scan error report: {err}"),
                            )
                            .ok();
                    }
                },
                Err(err) if err == SCAN_CANCELLED_MESSAGE => {}
                Err(err) => {
                    app_handle.emit("scan_failed", err).ok();
                }
            }
            ACTIVE_INCREMENTAL_SCANS
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .remove(&scan_key);
        });
        return Ok(());
    }

    let targets = scan_targets(&path);
    tauri::async_runtime::spawn(async move {
        let result = async {
            let (parsed, report) = run_pdu_for_paths(&app_handle, &ratio_arg, &targets).await?;
            app_handle.emit("scan_finalizing", ()).ok();
            let parsed = parsed.ok_or_else(|| "Local scanner produced no result".to_string())?;
            let content = serde_json::to_string(&parsed).map_err(|error| error.to_string())?;
            let result_path = write_scan_result(&content).map_err(|error| error.to_string())?;
            let errors_path =
                write_scan_error_report(&report).map_err(|error| error.to_string())?;
            Ok::<_, String>(CompletedPayload {
                path: result_path.display().to_string(),
                errors_path: errors_path.display().to_string(),
            })
        }
        .await;
        match result {
            Ok(payload) => {
                app_handle.emit("scan_completed", payload).ok();
            }
            Err(message) if message == SCAN_CANCELLED_MESSAGE => {}
            Err(message) => {
                app_handle.emit("scan_failed", message).ok();
            }
        }
    });
    Ok(())
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
    counts: &ScanErrorCounts,
) {
    app_handle
        .emit(
            "scan_status",
            Payload {
                items,
                total,
                operation_not_permitted: counts.operation_not_permitted,
                permission_denied: counts.permission_denied,
                interrupted: counts.interrupted,
                other: counts.other,
                cloud_skipped: counts.cloud_skipped,
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

fn collect_scan_stderr(
    chunk: &str,
    progress_regex: &Regex,
    error_regex: &Regex,
    items: &mut u64,
    total: &mut u64,
    error_records: &mut Vec<ScanErrorRecord>,
    counts: &mut ScanErrorCounts,
) -> bool {
    let mut changed = false;
    // pdu updates progress with carriage returns. A shell stderr event may contain
    // several progress updates followed by an error before its newline.
    for segment in chunk
        .split(['\r', '\n'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        if let Some(captures) = progress_regex.captures(segment) {
            *items = captures
                .get(1)
                .and_then(|matched| matched.as_str().parse::<u64>().ok())
                .unwrap_or_default();
            *total = captures
                .get(2)
                .and_then(|matched| matched.as_str().parse::<u64>().ok())
                .unwrap_or_default();
            changed = true;
        }
        if let Some(record) = parse_scan_error(error_regex, segment) {
            increment_scan_count(counts, &record.kind);
            error_records.push(record);
            changed = true;
        }
    }
    changed
}

fn classify_scan_error(reason: &str) -> &'static str {
    if reason.contains("DuckDisk cloud placeholder skipped")
        || (reason.to_ascii_lowercase().contains("deadlock") && reason.contains("(os error 11)"))
    {
        "cloudPlaceholder"
    } else if reason.contains("Operation not permitted") {
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
        increment_scan_count(&mut counts, &record.kind);
    }
    counts
}

fn increment_scan_count(counts: &mut ScanErrorCounts, kind: &str) {
    match kind {
        "operationNotPermitted" => counts.operation_not_permitted += 1,
        "permissionDenied" => counts.permission_denied += 1,
        "interrupted" => counts.interrupted += 1,
        "cloudPlaceholder" => counts.cloud_skipped += 1,
        _ => counts.other += 1,
    }
}

fn build_error_report(records: Vec<ScanErrorRecord>) -> ScanErrorReport {
    ScanErrorReport {
        counts: count_scan_errors(&records),
        records,
    }
}

fn merge_scan_error_reports(
    cached: ScanErrorReport,
    refreshed: ScanErrorReport,
    changed_paths: &[String],
    removed_paths: &HashSet<String>,
) -> ScanErrorReport {
    let mut records = cached.records;
    records.retain(|record| {
        let path = Path::new(&record.path);
        !changed_paths
            .iter()
            .any(|changed| path.starts_with(Path::new(changed)))
            && !removed_paths
                .iter()
                .any(|removed| path.starts_with(Path::new(removed)))
    });
    records.extend(refreshed.records);
    build_error_report(records)
}

async fn incremental_scan(
    app_handle: &tauri::AppHandle,
    scan_path: &str,
    ratio: &str,
    ratio_arg: &str,
) -> Result<(PathBuf, ScanErrorReport), String> {
    app_handle.emit("scan_incremental", ()).ok();

    let cached_path = cache_path(app_handle, scan_path, ratio)?;
    let index_path = cache_index_path(app_handle, scan_path, ratio)?;
    let cached_content = fs::read_to_string(&cached_path).map_err(|err| err.to_string())?;
    let index_content = fs::read_to_string(&index_path).map_err(|err| err.to_string())?;
    let index: CacheIndex = serde_json::from_str(&index_content).map_err(|err| err.to_string())?;
    if !index.matches_scan(scan_path, ratio) {
        return Err("The cached scan index is outdated; run a full rescan".to_string());
    }
    let mut cached_json: Value =
        serde_json::from_str(&cached_content).map_err(|err| err.to_string())?;

    let current_entries = match current_child_entries(scan_path) {
        Ok(entries) => entries,
        Err(_) => {
            // The cached root may have been evicted to the cloud since the last
            // scan. Reinspect it in the protected sidecar rather than removing
            // its old children and presenting it as an empty directory.
            app_handle.emit("scan_full", ()).ok();
            let (parsed, report) =
                run_pdu_for_paths(app_handle, ratio_arg, &scan_targets(scan_path)).await?;
            let parsed = parsed.ok_or_else(|| "Local scanner produced no result".to_string())?;
            let content = serde_json::to_string(&parsed).map_err(|err| err.to_string())?;
            return write_scan_result(&content)
                .map(|path| (path, report))
                .map_err(|err| err.to_string());
        }
    };
    let indexed_paths: HashSet<String> = index
        .children
        .iter()
        .map(|entry| entry.path.clone())
        .collect();
    let current_paths: HashSet<String> = current_entries
        .iter()
        .map(|entry| entry.path.clone())
        .collect();

    let changed_paths: Vec<String> = current_entries
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
                        || cached.cloud_placeholder != entry.cloud_placeholder
                })
                .unwrap_or(true)
        })
        .map(|entry| entry.path.clone())
        .collect();

    let removed_paths: HashSet<String> = indexed_paths
        .difference(&current_paths)
        .map(|path| path.to_string())
        .collect();

    let cached_report = read_cached_error_report(app_handle, scan_path, ratio)?
        .and_then(|content| serde_json::from_str::<ScanErrorReport>(&content).ok())
        .unwrap_or_default();
    let error_report = if changed_paths.is_empty() {
        merge_scan_error_reports(
            cached_report,
            ScanErrorReport::default(),
            &changed_paths,
            &removed_paths,
        )
    } else {
        let (scan_json, report) = run_pdu_for_paths(app_handle, ratio_arg, &changed_paths).await?;
        merge_changed_children(&mut cached_json, scan_path, &removed_paths, &scan_json)?;
        merge_scan_error_reports(cached_report, report, &changed_paths, &removed_paths)
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
    {
        let _guard = crate::no_cloud_download::ScanIoGuard::new()?;
        if !target.exists() {
            return Err(
                "The selected item no longer exists or its cloud content has not been downloaded"
                    .to_string(),
            );
        }
    }

    let ratio_arg = format!("--min-ratio={ratio}");
    let (scan_json, refreshed_report) =
        run_pdu_for_paths(app_handle, &ratio_arg, &[target_path.to_string()]).await?;
    let refreshed =
        scan_json.ok_or_else(|| "The selected item could not be scanned".to_string())?;
    let refreshed_content = serde_json::to_string(&refreshed).map_err(|err| err.to_string())?;

    let cached_path = cache_path(app_handle, scan_path, ratio)?;
    if target == scan_root {
        write_cached_result(app_handle, scan_path, ratio, &refreshed_content)?;
        let report = serde_json::to_string(&refreshed_report).map_err(|err| err.to_string())?;
        write_cached_error_report(app_handle, scan_path, ratio, &report)?;
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
        let merged_content = serde_json::to_string(&cached_json).map_err(|err| err.to_string())?;
        write_cached_result(app_handle, scan_path, ratio, &merged_content)?;
        let cached_report = read_cached_error_report(app_handle, scan_path, ratio)?
            .and_then(|content| serde_json::from_str::<ScanErrorReport>(&content).ok())
            .unwrap_or_default();
        let merged_report = merge_scan_error_reports(
            cached_report,
            refreshed_report,
            &[target_path.to_string()],
            &HashSet::new(),
        );
        let report = serde_json::to_string(&merged_report).map_err(|err| err.to_string())?;
        write_cached_error_report(app_handle, scan_path, ratio, &report)?;
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

    let (mut rx, child) = app_handle
        .shell()
        .sidecar("pdu")
        .map_err(|err| err.to_string())?
        .args(args)
        .spawn()
        .map_err(|err| err.to_string())?;
    let child_pid = register_child(&app_handle.state::<MyState>(), child);

    let mut stdout = None;
    let mut items = 0;
    let mut total = 0;
    let mut error_records = Vec::new();
    let mut counts = ScanErrorCounts::default();
    let mut last_emit = std::time::Instant::now();
    let mut termination = None;
    let mut diagnostic = String::new();
    let mut reader_error = None;

    while let Some(event) = rx.recv().await {
        match event {
            CommandEvent::Stdout(line) => {
                stdout = Some(String::from_utf8_lossy(&line).into_owned());
            }
            CommandEvent::Stderr(line) => {
                let line = String::from_utf8_lossy(&line);
                if collect_scan_stderr(
                    &line,
                    &progress_regex,
                    &error_regex,
                    &mut items,
                    &mut total,
                    &mut error_records,
                    &mut counts,
                ) {
                    if last_emit.elapsed() >= std::time::Duration::from_millis(100) {
                        emit_scan_status(app_handle, items, total, &counts);
                        last_emit = std::time::Instant::now();
                    }
                } else if !line.trim().is_empty() {
                    diagnostic = line.trim().chars().take(1024).collect();
                }
            }
            CommandEvent::Terminated(result) => {
                termination = Some(result);
            }
            CommandEvent::Error(error) => {
                reader_error = Some(error);
                break;
            }
            _ => {}
        }
    }
    unregister_child(&app_handle.state::<MyState>(), child_pid);
    emit_scan_status(app_handle, items, total, &counts);
    if let Some(error) = reader_error {
        return Err(error);
    }

    if let Some(termination) = termination {
        if termination.signal == Some(9) {
            return Err(SCAN_CANCELLED_MESSAGE.into());
        }
        if termination.code != Some(0) {
            return Err(format!(
                "Local scanner stopped without completing: {diagnostic}"
            ));
        }
    }
    if stdout.is_none() {
        return Err("Local scanner produced no result".into());
    }

    let error_report = build_error_report(error_records);
    let mut parsed = stdout
        .map(|content| parse_pdu_content(&content))
        .transpose()?;
    if let Some(parsed) = parsed.as_mut() {
        annotate_cloud_skips(parsed, &error_report);
    }
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

fn annotate_cloud_skips(parsed: &mut Value, report: &ScanErrorReport) {
    let skipped: HashMap<_, _> = report
        .records
        .iter()
        .filter(|record| record.kind == "cloudPlaceholder")
        .map(|record| {
            (
                PathBuf::from(&record.path),
                "Cloud content not downloaded; skipped to avoid automatic download.",
            )
        })
        .collect();
    fn walk(node: &mut Value, parent: &Path, skipped: &HashMap<PathBuf, &str>) {
        let name = node.get("name").and_then(Value::as_str).unwrap_or_default();
        let path = if name == "(total)" {
            PathBuf::from("/")
        } else if Path::new(name).is_absolute() {
            PathBuf::from(name)
        } else {
            parent.join(name)
        };
        if let Some(reason) = skipped.get(&path) {
            node["scanSkippedReason"] = Value::from(*reason);
        }
        if let Some(children) = node.get_mut("children").and_then(Value::as_array_mut) {
            for child in children {
                walk(child, &path, skipped);
            }
        }
    }
    if let Some(tree) = parsed.get_mut("tree") {
        walk(tree, Path::new("/"), &skipped);
    }
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

    let mut replaced_sizes = None;
    for child in children {
        let Some(name) = child.get("name").and_then(Value::as_str) else {
            continue;
        };
        let child_path = if Path::new(name).is_absolute() {
            PathBuf::from(name)
        } else {
            node_path.join(name)
        };

        let before = (tree_size(child), tree_allocated_size(child));
        let replaced = if child_path == target_path {
            let preserved_name = child["name"].clone();
            *child = refreshed_tree.clone();
            child["name"] = preserved_name;
            true
        } else {
            target_path.starts_with(&child_path)
                && replace_cached_subtree(child, &child_path, target_path, refreshed_tree)
        };
        if replaced {
            replaced_sizes = Some((before, (tree_size(child), tree_allocated_size(child))));
            break;
        }
    }
    if let Some((before, after)) = replaced_sizes {
        adjust_node_totals(node, before, after);
        true
    } else {
        false
    }
}

fn adjusted_tree_total(total: u64, before: u64, after: u64) -> u64 {
    if after >= before {
        total.saturating_add(after - before)
    } else {
        total.saturating_sub(before - after)
    }
}

fn adjust_node_totals(node: &mut Value, before: (u64, u64), after: (u64, u64)) {
    node["size"] = Value::from(adjusted_tree_total(tree_size(node), before.0, after.0));
    node["allocatedSize"] = Value::from(adjusted_tree_total(
        tree_allocated_size(node),
        before.1,
        after.1,
    ));
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
    let totals_before = (tree_size(root), tree_allocated_size(root));
    let children = root
        .get_mut("children")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| "Cached scan tree has no children".to_string())?;
    let children_before = (
        children.iter().map(tree_size).sum::<u64>(),
        children.iter().map(tree_allocated_size).sum::<u64>(),
    );

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
    root["size"] = Value::from(adjusted_tree_total(
        totals_before.0,
        children_before.0,
        size,
    ));
    root["allocatedSize"] = Value::from(adjusted_tree_total(
        totals_before.1,
        children_before.1,
        allocated_size,
    ));
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

fn read_cached_error_report(
    app_handle: &tauri::AppHandle,
    scan_path: &str,
    ratio: &str,
) -> Result<Option<String>, String> {
    let path = cache_error_report_path(app_handle, scan_path, ratio)?;
    if !path.exists() {
        return Ok(None);
    }
    fs::read_to_string(path)
        .map(Some)
        .map_err(|err| err.to_string())
}

pub fn read_cached_scan_error_report(
    app_handle: tauri::AppHandle,
    scan_path: String,
    ratio: String,
) -> Result<Option<String>, String> {
    read_cached_error_report(&app_handle, &scan_path, &ratio)
}

pub fn has_cached_index(
    app_handle: &tauri::AppHandle,
    scan_path: &str,
    ratio: &str,
) -> Result<bool, String> {
    let path = cache_index_path(app_handle, scan_path, ratio)?;
    let Ok(content) = fs::read_to_string(path) else {
        return Ok(false);
    };
    Ok(serde_json::from_str::<CacheIndex>(&content)
        .map(|index| index.matches_scan(scan_path, ratio))
        .unwrap_or(false))
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
    let errors_path = cache_error_report_path(&app_handle, &scan_path, &ratio)?;
    if errors_path.exists() {
        fs::remove_file(errors_path).map_err(|err| err.to_string())?;
    }
    Ok(())
}

pub fn read_result(
    app_handle: tauri::AppHandle,
    path: String,
    scan_path: String,
    ratio: String,
    error_report: String,
) -> Result<String, String> {
    let prefix = format!("duckdisk-scan-{}-", std::process::id());
    let path = validate_result_file(&path, &prefix)?;

    let content = fs::read_to_string(&path).map_err(|err| err.to_string())?;
    fs::remove_file(path).ok();
    write_cached_result(&app_handle, &scan_path, &ratio, &content)?;
    write_cached_error_report(&app_handle, &scan_path, &ratio, &error_report)?;
    Ok(content)
}

pub fn read_error_report(path: String) -> Result<String, String> {
    let prefix = format!("duckdisk-scan-errors-{}-", std::process::id());
    let path = validate_result_file(&path, &prefix)?;

    let content = fs::read_to_string(&path).map_err(|err| err.to_string())?;
    fs::remove_file(path).ok();
    Ok(content)
}

fn register_child(state: &MyState, child: tauri_plugin_shell::process::CommandChild) -> u32 {
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

fn write_cached_error_report(
    app_handle: &tauri::AppHandle,
    scan_path: &str,
    ratio: &str,
    content: &str,
) -> Result<(), String> {
    serde_json::from_str::<ScanErrorReport>(content).map_err(|err| err.to_string())?;
    let path = cache_error_report_path(app_handle, scan_path, ratio)?;
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
    "pdu-0.23.0-dual-size-v3-node-types".hash(&mut hasher);
    scan_path.hash(&mut hasher);
    ratio.hash(&mut hasher);
    let key = hasher.finish();
    let cache_dir = app_handle
        .path()
        .app_cache_dir()
        .map_err(|_| "Could not resolve app cache directory".to_string())?;
    Ok(cache_dir.join("scans").join(format!("{key:016x}.json")))
}

fn cache_index_path(
    app_handle: &tauri::AppHandle,
    scan_path: &str,
    ratio: &str,
) -> Result<PathBuf, String> {
    Ok(cache_path(app_handle, scan_path, ratio)?.with_extension("index.json"))
}

fn cache_error_report_path(
    app_handle: &tauri::AppHandle,
    scan_path: &str,
    ratio: &str,
) -> Result<PathBuf, String> {
    Ok(cache_path(app_handle, scan_path, ratio)?.with_extension("errors.json"))
}

fn write_cache_index(
    app_handle: &tauri::AppHandle,
    scan_path: &str,
    ratio: &str,
    content: &str,
) -> Result<(), String> {
    let _guard = crate::no_cloud_download::ScanIoGuard::new()?;
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
        version: CACHE_INDEX_VERSION.to_string(),
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
    let _guard = crate::no_cloud_download::ScanIoGuard::new()?;
    #[cfg(target_os = "macos")]
    {
        use std::os::macos::fs::MetadataExt;
        if fs::symlink_metadata(scan_path)
            .map(|metadata| metadata.is_dir() && metadata.st_flags() & 0x4000_0000 != 0)
            .unwrap_or(false)
        {
            return Err("Cached root is a cloud placeholder; protected rescan required".into());
        }
    }
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
    #[cfg(target_os = "macos")]
    let cloud_placeholder = {
        use std::os::macos::fs::MetadataExt;
        metadata.st_flags() & 0x4000_0000 != 0
    };
    #[cfg(not(target_os = "macos"))]
    let cloud_placeholder = false;
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
        cloud_placeholder,
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
    fn cloud_skips_are_distinct_from_permission_errors_and_annotate_nested_nodes() {
        let reason = "DuckDisk cloud placeholder skipped: automatic download disabled";
        let report = build_error_report(vec![
            ScanErrorRecord {
                operation: "symlink_metadata".into(),
                path: "/Users/test/cloud".into(),
                reason: reason.into(),
                kind: classify_scan_error(reason).into(),
            },
            ScanErrorRecord {
                operation: "read_dir".into(),
                path: "/Users/test/blocked".into(),
                reason: "Resource deadlock avoided (os error 11)".into(),
                kind: classify_scan_error("Resource deadlock avoided (os error 11)").into(),
            },
        ]);
        assert_eq!(report.counts.cloud_skipped, 2);
        assert_eq!(report.counts.other, 0);
        let mut parsed = json!({"tree": {"name": "(total)", "children": [
            {"name": "/Users", "children": [{"name": "test", "children": [
                {"name": "cloud", "children": []}, {"name": "local", "children": []}
            ]}]}
        ]}});
        annotate_cloud_skips(&mut parsed, &report);
        let children = &parsed["tree"]["children"][0]["children"][0]["children"];
        assert!(children[0]["scanSkippedReason"]
            .as_str()
            .unwrap()
            .contains("not downloaded"));
        assert!(children[1].get("scanSkippedReason").is_none());
    }

    #[test]
    fn older_error_counts_default_cloud_skips_to_zero() {
        let counts: ScanErrorCounts = serde_json::from_value(json!({
            "permissionDenied": 1, "operationNotPermitted": 2, "interrupted": 3, "other": 4
        }))
        .unwrap();
        assert_eq!(counts.cloud_skipped, 0);
        assert_eq!(counts.permission_denied, 1);
    }

    #[test]
    fn stderr_keeps_errors_after_carriage_return_progress() {
        let progress_regex = Regex::new(r"\(scanned ([0-9]+), total ([0-9]+)\)").unwrap();
        let error_regex = Regex::new(r#"^\[error\] (\S+) "(.+)": (.+)$"#).unwrap();
        let mut items = 0;
        let mut total = 0;
        let mut records = Vec::new();
        let mut counts = ScanErrorCounts::default();

        let changed = collect_scan_stderr(
            "\r(scanned 12, total 50)\r[error] read_dir \"/protected\": Operation not permitted (os error 1)\n\r(scanned 25, total 50)\r[error] read_dir \"/restricted\": Permission denied (os error 13)\n",
            &progress_regex,
            &error_regex,
            &mut items,
            &mut total,
            &mut records,
            &mut counts,
        );

        assert!(changed);
        assert_eq!((items, total), (25, 50));
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].path, "/protected");
        assert_eq!(records[0].kind, "operationNotPermitted");
        assert_eq!(records[1].kind, "permissionDenied");
    }

    #[test]
    fn incremental_error_report_replaces_only_changed_subtrees() {
        let record = |path: &str| ScanErrorRecord {
            operation: "read_dir".to_string(),
            path: path.to_string(),
            reason: "Operation not permitted".to_string(),
            kind: "operationNotPermitted".to_string(),
        };
        let cached = build_error_report(vec![
            record("/Users/old"),
            record("/Library/unchanged"),
            record("/Applications/removed"),
        ]);
        let refreshed = build_error_report(vec![record("/Users/new")]);
        let removed = HashSet::from(["/Applications".to_string()]);

        let merged = merge_scan_error_reports(cached, refreshed, &["/Users".to_string()], &removed);

        let paths: Vec<_> = merged
            .records
            .iter()
            .map(|record| record.path.as_str())
            .collect();
        assert_eq!(paths, ["/Library/unchanged", "/Users/new"]);
        assert_eq!(merged.counts.operation_not_permitted, 2);
    }

    #[test]
    fn old_scan_index_requires_a_full_rescan() {
        let mut index = CacheIndex {
            version: "duckdisk-cache-index-v4-skip-dataless-directories".to_string(),
            scan_path: "/".to_string(),
            ratio: "0".to_string(),
            children: Vec::new(),
        };

        assert!(!index.matches_scan("/", "0"));
        index.version = CACHE_INDEX_VERSION.to_string();
        assert!(index.matches_scan("/", "0"));
        assert!(!index.matches_scan("/Users", "0"));
    }

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
                "isDirectory": true,
                "children": []
            }
        })
        .to_string();

        let parsed = parse_pdu_content(&content).expect("dual size output should normalize");
        assert_eq!(parsed["unit"], "bytes");
        assert_eq!(parsed["tree"]["size"], 5);
        assert_eq!(parsed["tree"]["allocatedSize"], 4096);
        assert_eq!(parsed["tree"]["isDirectory"], true);
    }

    #[test]
    fn subtree_refresh_preserves_parent_deduplication() {
        let mut root = json!({
            "name": "(total)", "size": 16, "allocatedSize": 16,
            "children": [
                {"name": "/a", "size": 10, "allocatedSize": 10},
                {"name": "/b", "size": 10, "allocatedSize": 10}
            ]
        });
        let refreshed = json!({"name": "/a", "size": 15, "allocatedSize": 15});
        assert!(replace_cached_subtree(
            &mut root,
            Path::new("/"),
            Path::new("/a"),
            &refreshed
        ));
        assert_eq!(root["allocatedSize"], 21);
        assert_eq!(root["size"], 21);
    }

    #[test]
    fn incremental_merge_preserves_parent_deduplication() {
        let mut cached = json!({"tree": {
            "name": "(total)", "size": 16, "allocatedSize": 16,
            "children": [
                {"name": "/a", "size": 10, "allocatedSize": 10},
                {"name": "/b", "size": 10, "allocatedSize": 10}
            ]
        }});
        let changed = Some(json!({"tree": {"name": "/a", "size": 15, "allocatedSize": 15}}));
        merge_changed_children(&mut cached, "/", &HashSet::new(), &changed).unwrap();
        assert_eq!(cached["tree"]["allocatedSize"], 21);
        assert_eq!(cached["tree"]["size"], 21);
    }
}
