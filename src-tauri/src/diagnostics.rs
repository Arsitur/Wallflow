use std::{collections::VecDeque, fs::{self, OpenOptions}, io::Write, path::PathBuf, sync::{Mutex, OnceLock}, time::{SystemTime, UNIX_EPOCH}};
use serde::Serialize;
use tauri::Manager;

const LIMIT: usize = 2000;
const FILE_LIMIT: u64 = 2 * 1024 * 1024;
#[derive(Clone, Serialize)]
pub struct Entry { pub id: u64, pub time: u64, pub level: String, pub source: String, pub message: String }
#[derive(Default)]
struct Logs { next: u64, entries: VecDeque<Entry>, path: Option<PathBuf> }
static LOGS: OnceLock<Mutex<Logs>> = OnceLock::new();
fn logs() -> &'static Mutex<Logs> { LOGS.get_or_init(Default::default) }

pub fn initialize(app: &tauri::AppHandle) {
    if let Ok(dir) = app.path().app_log_dir() {
        if fs::create_dir_all(&dir).is_ok() {
            if let Ok(mut logs) = logs().lock() { logs.path = Some(dir.join("wallflow.jsonl")); }
        }
    }
    record("info", "app", "Wallflow started");
    std::panic::set_hook(Box::new(|info| record("error", "panic", info.to_string())));
}

pub fn record(level: &str, source: &str, message: impl AsRef<str>) {
    let Ok(mut logs) = logs().lock() else { return; };
    logs.next += 1;
    let entry = Entry { id: logs.next, time: SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as u64,
        level: level.chars().take(12).collect(), source: source.chars().take(64).collect(), message: message.as_ref().chars().take(6000).collect() };
    eprintln!("[{}] {}: {}", entry.level, entry.source, entry.message);
    if let Some(path) = &logs.path {
        if fs::metadata(path).is_ok_and(|meta| meta.len() >= FILE_LIMIT) {
            let backup = path.with_extension("previous.jsonl");
            let _ = fs::remove_file(&backup);
            let _ = fs::rename(path, backup);
        }
        if let (Ok(mut file), Ok(json)) = (OpenOptions::new().create(true).append(true).open(path), serde_json::to_string(&entry)) { let _ = writeln!(file, "{json}"); }
    }
    logs.entries.push_back(entry);
    while logs.entries.len() > LIMIT { logs.entries.pop_front(); }
}

#[tauri::command]
pub fn frontend_log(level: String, source: String, message: String) { record(&level, &source, message); }
#[tauri::command]
pub fn get_logs(after: u64) -> Vec<Entry> { logs().lock().map(|logs| logs.entries.iter().filter(|entry| entry.id > after).cloned().collect()).unwrap_or_default() }
#[tauri::command]
pub fn log_path() -> Option<String> { logs().lock().ok()?.path.as_ref().map(|path| path.to_string_lossy().into_owned()) }

pub fn show(app: &tauri::AppHandle) -> Result<(), String> {
    if let Some(window) = app.get_webview_window("logs") {
        return window.show().and_then(|_| window.set_focus()).map_err(|error| error.to_string());
    }
    tauri::WebviewWindowBuilder::new(app, "logs", tauri::WebviewUrl::App("index.html?logs".into()))
        .title("Wallflow — Logs / Debugger").inner_size(1050.0, 680.0).min_inner_size(640.0, 360.0)
        .build().map_err(|error| error.to_string())?;
    Ok(())
}
