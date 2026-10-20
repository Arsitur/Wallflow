//! Wallflow reads local Steam metadata and invokes Wallpaper Engine's documented CLI.
//! No remote calls, arbitrary shell strings, privilege elevation, or runtime downloads.
use base64::Engine;
use image::{codecs::jpeg::JpegEncoder, DynamicImage, ImageReader};
use serde::Serialize;
use serde_json::Value;
use std::{collections::{HashMap, hash_map::DefaultHasher}, fs, hash::{Hash, Hasher}, path::{Path, PathBuf}, process::{Command, Stdio}, sync::{Mutex, atomic::{AtomicBool, Ordering}}, time::{SystemTime, UNIX_EPOCH}};
use tauri::{Manager, State};
use crate::engine_config::{EngineConfig, normalize_path};

const STEAM_APP_ID: &str = "431960";
const MAX_PREVIEW_BYTES: u64 = 20 * 1024 * 1024;
const MAX_JSON_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Clone, Serialize)]
pub struct WallpaperItem {
    id: String,
    title: String,
    source: String,
    kind: String,
    preview_available: bool,
    preview_revision: String,
    added_at: u64,
}
#[derive(Clone)]
struct WallpaperRecord { public: WallpaperItem, target: PathBuf, preview: Option<PathBuf>, project: Option<PathBuf> }

#[derive(Default)]
pub struct Gallery {
    records: Mutex<HashMap<String, WallpaperRecord>>,
    engine: Mutex<Option<PathBuf>>,
    playlist: Mutex<Option<String>>,
    unavailable: Mutex<HashMap<String, &'static str>>,
    operation: Mutex<()>,
    quitting: AtomicBool,
}

pub fn quit(app: tauri::AppHandle) {
    app.state::<Gallery>().quitting.store(true, Ordering::Release);
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<Gallery>();
        // Let an in-flight audio guard restore the Windows mixer before exit.
        let _operation = state.operation.lock();
        app.exit(0);
    });
}

pub fn initialize(app: &tauri::AppHandle) -> Result<Vec<String>, String> {
    let engine = engine_executable(&libraries()).ok_or("Wallpaper Engine installation not found")?;
    let config = EngineConfig::read(&engine)?;
    let names: Vec<_> = config.playlists.iter().map(|playlist| playlist.name.clone()).collect();
    let saved = app.path().app_config_dir().ok().and_then(|dir| fs::read(dir.join("gallery.json")).ok())
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok());
    let selected = match saved {
        Some(value) => value["playlist"].as_str().map(str::to_owned),
        None => names.iter().find(|name| name.to_lowercase().contains("arthurian")).cloned(),
    };
    let gallery = app.state::<Gallery>();
    *gallery.engine.lock().map_err(|e| e.to_string())? = Some(engine);
    *gallery.playlist.lock().map_err(|e| e.to_string())? = selected;
    Ok(names)
}

pub fn selected_playlist(app: &tauri::AppHandle) -> Option<String> {
    app.state::<Gallery>().playlist.lock().ok().and_then(|value| value.clone())
}

pub fn set_playlist(app: &tauri::AppHandle, name: Option<String>) -> Result<(), String> {
    *app.state::<Gallery>().playlist.lock().map_err(|e| e.to_string())? = name;
    Ok(())
}

pub fn playlist_names(app: &tauri::AppHandle) -> Result<Vec<String>, String> {
    let executable = app.state::<Gallery>().engine.lock().map_err(|e| e.to_string())?.clone().ok_or("Wallpaper Engine not found")?;
    let config = EngineConfig::read(&executable)?;
    let mut names: Vec<_> = config.playlists.iter().chain(config.monitor_playlists.values()).map(|playlist| playlist.name.clone()).collect();
    names.sort_by_key(|name| name.to_lowercase());
    names.dedup();
    Ok(names)
}

pub fn display_mode(app: &tauri::AppHandle) -> Result<&'static str, String> {
    let executable = app.state::<Gallery>().engine.lock().map_err(|e| e.to_string())?.clone().ok_or("Wallpaper Engine not found")?;
    Ok(EngineConfig::read(&executable)?.display_mode())
}

fn matches_file(record: &WallpaperRecord, file: &str) -> bool {
    let file = normalize_path(file);
    let target = normalize_path(&record.target.to_string_lossy());
    if file == target { return true; }
    // Scene playlists and getWallpaper return scene.pkg; the validated launch target is project.json.
    record.target.file_name().and_then(|name| name.to_str()).is_some_and(|name| name.eq_ignore_ascii_case("project.json"))
        && target.rsplit_once('/').map(|(parent, _)| parent) == file.rsplit_once('/').map(|(parent, _)| parent)
}

fn imported_image(file: &str) -> Option<WallpaperRecord> {
    let target = Path::new(file).canonicalize().ok()?;
    if !target.is_file() { return None; }
    let extension = target.extension()?.to_str()?.to_ascii_lowercase();
    if !["jpg", "jpeg", "png", "webp", "gif"].contains(&extension.as_str()) { return None; }
    let metadata = fs::metadata(&target).ok()?;
    let item = WallpaperItem {
        id: format!("image:{}", normalize_path(&target.to_string_lossy())),
        title: target.file_stem()?.to_string_lossy().into_owned(), source: "local".into(), kind: "image".into(),
        preview_available: true, preview_revision: format!("{}-{}", metadata.len(), metadata.modified().map(timestamp).unwrap_or(0)),
        added_at: metadata.created().or_else(|_| metadata.modified()).map(timestamp).unwrap_or(0),
    };
    Some(WallpaperRecord { public: item, target: target.clone(), preview: Some(target), project: None })
}

fn playlist_item_problem(file: &str) -> &'static str {
    let path = Path::new(file);
    if path.parent().is_some_and(|folder| !folder.is_dir()) { "Wallpaper folder is missing from disk" }
    else if !path.is_file() { "Playlist target file is missing from disk" }
    else if path.parent().is_some_and(|folder| !folder.join("project.json").is_file()) { "No project.json found; this is not a supported standalone image" }
    else { "Wallpaper project metadata or media is invalid or unsupported" }
}

fn reconcile_playlist_records(records: &mut HashMap<String, WallpaperRecord>, unavailable: &mut HashMap<String, &'static str>, files: impl IntoIterator<Item = String>) {
    let mut seen = std::collections::HashSet::new();
    let mut current_unavailable = HashMap::new();
    for file in files {
        if !seen.insert(normalize_path(&file)) { continue; }
        if records.values().any(|record| matches_file(record, &file)) { continue; }
        let folder = Path::new(&file).parent();
        let installed = folder.and_then(|folder| {
            let source = if folder.parent().and_then(Path::file_name).is_some_and(|name| name == STEAM_APP_ID) { "workshop" } else { "local" };
            project(folder, source).filter(|record| matches_file(record, &file))
        });
        if let Some(record) = installed.or_else(|| imported_image(&file)) {
            crate::diagnostics::record("info", "scan", format!("Discovered playlist item {} without a full rescan", record.public.id));
            records.insert(record.public.id.clone(), record);
        } else {
            let reason = playlist_item_problem(&file);
            let key = normalize_path(&file);
            if unavailable.get(&key) != Some(&reason) {
                crate::diagnostics::record("warn", "scan", format!("{reason}: {file}. The saved playlist still references this item."));
            }
            current_unavailable.insert(key, reason);
        }
    }
    *unavailable = current_unavailable;
}

fn filter_records(records: &HashMap<String, WallpaperRecord>, files: Option<&[String]>) -> Vec<WallpaperItem> {
    let Some(files) = files else { return ordered_items(records); };
    let mut seen = std::collections::HashSet::new();
    files.iter().filter_map(|file| records.values().find(|record| matches_file(record, file)))
        .filter(|record| seen.insert(&record.public.id)).map(|record| record.public.clone()).collect()
}

fn current_wallpaper(executable: &Path, monitor: u32) -> Option<String> {
    let output = crate::engine_query::query(executable, monitor).and_then(|bytes| wallpaper_output(&bytes));
    if output.is_none() { crate::diagnostics::record("warn", "engine.query", format!("No live wallpaper response for Monitor{monitor}; using saved configuration")); }
    output
}

fn wallpaper_output(bytes: &[u8]) -> Option<String> {
    let text = if bytes.starts_with(&[0xff, 0xfe]) || bytes.get(1) == Some(&0) {
        let words: Vec<_> = bytes.chunks_exact(2).map(|pair| u16::from_le_bytes([pair[0], pair[1]])).collect();
        String::from_utf16_lossy(&words)
    } else { String::from_utf8_lossy(bytes).into_owned() };
    let text = strip_terminal_sequences(&text);
    text.lines().map(|line| line.trim_matches('\u{feff}').trim().trim_matches('"').trim_end_matches('\0'))
        .find(|line| Path::new(line).is_absolute()).map(str::to_owned)
}

fn strip_terminal_sequences(text: &str) -> String {
    let mut result = String::new();
    let mut chars = text.chars();
    while let Some(ch) = chars.next() {
        if ch != '\u{1b}' { result.push(ch); continue; }
        match chars.next() {
            Some('[') => { for ch in chars.by_ref() { if ('\u{40}'..='\u{7e}').contains(&ch) { break; } } }
            Some(']') => { while let Some(ch) = chars.next() { if ch == '\u{7}' || ch == '\u{1b}' && chars.next() == Some('\\') { break; } } }
            _ => {}
        }
    }
    result
}

fn active_record(records: &HashMap<String, WallpaperRecord>, live: Option<&str>, saved: Option<&str>) -> Option<String> {
    // A CLI diagnostic or stale/unrecognized path must not mask a valid saved selection.
    [live, saved].into_iter().flatten().find_map(|file|
        records.values().find(|record| matches_file(record, file)).map(|record| record.public.id.clone()))
}

#[derive(Serialize)]
pub struct GallerySnapshot {
    items: Vec<WallpaperItem>, active_id: Option<String>, playlist: Option<String>, monitor: u32,
    display_mode: &'static str,
    amazing_animations: bool,
}

#[tauri::command]
pub async fn gallery_snapshot(app: tauri::AppHandle) -> Result<GallerySnapshot, String> {
    let monitor_name = crate::target_monitor(&app)?;
    tauri::async_runtime::spawn_blocking(move || {
        let gallery = app.state::<Gallery>();
        let needs_scan = gallery.records.lock().map_err(|e| e.to_string())?.is_empty();
        if needs_scan {
            let (records, engine) = scan_wallpapers();
            *gallery.records.lock().map_err(|e| e.to_string())? = records;
            *gallery.engine.lock().map_err(|e| e.to_string())? = engine;
        }
        let executable = gallery.engine.lock().map_err(|e| e.to_string())?.clone().ok_or("Wallpaper Engine not found")?;
        let config = EngineConfig::read(&executable)?;
        let physical = config.monitor_index(&monitor_name).or_else(|| single_monitor(&config))
            .ok_or("Cannot match the current display with Wallpaper Engine")?;
        let monitor = config.wallpaper_monitor(physical);
        let display_mode = config.display_mode();
        let playlist = gallery.playlist.lock().map_err(|e| e.to_string())?.clone();
        let selected = playlist.as_ref().map(|name| config.playlist(name)
            .ok_or_else(|| format!("Playlist '{name}' no longer exists. Choose another from Settings in the tray menu."))).transpose()?;
        let live = current_wallpaper(&executable, monitor);
        // WE may flush a recent selection while processing the query. Read its current state again.
        let current = EngineConfig::read(&executable).ok().and_then(|latest| latest.current.get(&monitor).cloned())
            .or_else(|| config.current.get(&monitor).cloned());
        let mut records = gallery.records.lock().map_err(|e| e.to_string())?;
        let mut unavailable = gallery.unavailable.lock().map_err(|e| e.to_string())?;
        reconcile_playlist_records(&mut records, &mut unavailable, config.playlists.iter().chain(config.monitor_playlists.values()).flat_map(|playlist| playlist.items.iter().cloned()));
        let mut items = filter_records(&records, selected.as_ref().map(|playlist| playlist.items.as_slice()));
        let cache_generation = app.path().app_cache_dir().ok().map(|dir| crate::cover_cache::generation(&dir.join("previews"))).unwrap_or_default();
        for item in &mut items { item.preview_revision.push_str(&format!(":{cache_generation}")); }
        let active_id = active_record(&records, live.as_deref(), current.as_deref())
            .filter(|id| items.iter().any(|item| &item.id == id));
        crate::diagnostics::record("info", "snapshot", format!("Display {monitor_name} (Monitor{physical}) -> renderer Monitor{monitor}; mode={display_mode}; playlist={playlist:?}; {} items; active={active_id:?}; live query={}", items.len(), live.is_some()));
        Ok(GallerySnapshot { items, active_id, playlist, monitor, display_mode, amazing_animations: crate::settings::amazing_animations(&app) })
    }).await.map_err(|e| e.to_string())?
}

fn path_in_folder(folder: &Path, relative: &str) -> Option<PathBuf> {
    let root = folder.canonicalize().ok()?;
    let path = root.join(relative).canonicalize().ok()?;
    if path.is_file() && path.starts_with(root) { Some(path) } else { None }
}

fn steam_base() -> Vec<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    #[cfg(windows)] {
        use winreg::{enums::HKEY_CURRENT_USER, RegKey};
        if let Ok(reg) = RegKey::predef(HKEY_CURRENT_USER).open_subkey("Software\\Valve\\Steam") {
            if let Ok(path) = reg.get_value::<String, _>("SteamPath") { candidates.push(PathBuf::from(path)); }
        }
    }
    candidates.push(PathBuf::from(r"C:\Program Files (x86)\Steam"));
    candidates.push(PathBuf::from(r"C:\Program Files\Steam"));
    candidates
}

fn libraries() -> Vec<PathBuf> {
    let mut found = HashMap::new();
    for root in steam_base() {
        if !root.is_dir() { continue; }
        if let Ok(canonical) = root.canonicalize() { found.insert(path_key(&canonical), canonical); }
        if let Ok(vdf) = fs::read_to_string(root.join("steamapps").join("libraryfolders.vdf")) {
            for line in vdf.lines().map(str::trim) {
                if !line.starts_with("\"path\"") { continue; }
                // VDF layout: "path" "D:\\SteamLibrary" (old/new Steam formats)
                if let Some(p) = line.split('"').nth(3) {
                    let fixed = p.replace("\\\\", "\\");
                    let lib = PathBuf::from(fixed);
                    if let Ok(canonical) = lib.canonicalize() { found.insert(path_key(&canonical), canonical); }
                }
            }
        }
    }
    let mut sorted: Vec<_> = found.into_values().collect();
    sorted.sort();
    sorted
}

// Windows paths are case insensitive; registry and VDF paths need one identity.
fn path_key(path: &Path) -> String {
    let key = path.to_string_lossy().replace('\\', "/");
    if cfg!(windows) { key.to_lowercase() } else { key }
}

fn timestamp(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs()
}

fn ordered_items(records: &HashMap<String, WallpaperRecord>) -> Vec<WallpaperItem> {
    let mut items: Vec<_> = records.values().map(|record| record.public.clone()).collect();
    items.sort_by(|a, b| b.added_at.cmp(&a.added_at).then_with(|| a.id.cmp(&b.id)));
    items
}

fn engine_executable(roots: &[PathBuf]) -> Option<PathBuf> {
    // Only run Wallpaper Engine's executable from an installed Steam library.
    for root in roots {
        let folder = root.join("steamapps/common/wallpaper_engine");
        for binary in ["wallpaper64.exe", "wallpaper32.exe"] {
            let candidate = folder.join(binary);
            if candidate.is_file() { return candidate.canonicalize().ok(); }
        }
    }
    None
}

fn project(folder: &Path, source: &str) -> Option<WallpaperRecord> {
    let folder = folder.canonicalize().ok()?;
    let folder = folder.as_path();
    let json_path = folder.join("project.json");
    if !json_path.is_file() || fs::metadata(&json_path).ok()?.len() > MAX_JSON_BYTES { return None; }
    let json: Value = serde_json::from_str(&fs::read_to_string(&json_path).ok()?).ok()?;
    let title = json.get("title").and_then(Value::as_str)?.trim().to_string();
    if title.is_empty() || title.len() > 350 { return None; }
    let kind = json.get("type").and_then(Value::as_str).unwrap_or("scene").to_ascii_lowercase();
    let target = match kind.as_str() {
        "video" | "web" | "application" => {
            let relative = json.get("file").and_then(Value::as_str)?;
            path_in_folder(folder, relative)?
        }
        _ => json_path.canonicalize().ok()?,
    };
    let preview = json.get("preview").and_then(Value::as_str)
        .and_then(|name| path_in_folder(folder, name))
        .or_else(|| ["preview.jpg", "preview.jpeg", "preview.png", "preview.webp", "preview.gif"]
            .iter().find_map(|name| path_in_folder(folder, name)));
    let workshop_id = if source == "workshop" {
        folder.file_name()?.to_str()?.parse::<u64>().ok()
    } else {
        json.get("workshopid").and_then(|value| value.as_u64().or_else(|| value.as_str()?.parse().ok()))
    }.filter(|id| *id > 0);
    let id = workshop_id.map(|id| format!("workshop:{id}"))
        .unwrap_or_else(|| format!("local:{}", path_key(&target)));
    // Creation time reflects when this PC installed/added the project, not the author's update date.
    let metadata = fs::metadata(folder).ok()?;
    let added_at = metadata.created().or_else(|_| metadata.modified()).map(timestamp).unwrap_or(0);
    let item = WallpaperItem {
        id,
        title,
        source: source.to_owned(),
        kind: kind.chars().take(30).collect(),
        preview_available: preview.is_some() || kind == "scene" || kind == "video",
        preview_revision: if kind == "scene" { crate::covers::revision(&target) } else { preview.as_ref().and_then(|path| fs::metadata(path).ok()).map(|meta| {
            format!("{}-{}", meta.len(), meta.modified().ok().and_then(|time| time.duration_since(UNIX_EPOCH).ok()).map(|time| time.as_nanos()).unwrap_or(0))
        }).unwrap_or_default() },
        added_at,
    };
    Some(WallpaperRecord { public: item, target, preview, project: Some(json_path) })
}

fn scan_wallpapers() -> (HashMap<String, WallpaperRecord>, Option<PathBuf>) {
    scan_roots(libraries())
}

fn scan_roots(roots: Vec<PathBuf>) -> (HashMap<String, WallpaperRecord>, Option<PathBuf>) {
    let engine = engine_executable(&roots);
    let mut records = HashMap::<String, WallpaperRecord>::new();
    for root in roots {
        let sources = [
            (root.join("steamapps/workshop/content").join(STEAM_APP_ID), "workshop"),
            (root.join("steamapps/common/wallpaper_engine/projects/myprojects"), "local"),
        ];
        for (dir, source) in sources {
            if let Ok(folders) = fs::read_dir(&dir) {
                for entry in folders.flatten() {
                    let folder = entry.path();
                    if !folder.is_dir() { continue; }
                    if let Some(w) = project(&folder, source) {
                        // Prefer the installed Workshop copy over a local published copy.
                        records.entry(w.public.id.clone()).and_modify(|existing| {
                            if w.public.source == "workshop" && existing.public.source != "workshop" { *existing = w.clone(); }
                        }).or_insert(w);
                    }
                }
            }
        }
    }
    (records, engine)
}

#[tauri::command]
pub async fn list_wallpapers(state: State<'_, Gallery>) -> Result<Vec<WallpaperItem>, String> {
    let (records, engine) = tauri::async_runtime::spawn_blocking(scan_wallpapers).await.map_err(|e| e.to_string())?;
    let items = ordered_items(&records);
    crate::diagnostics::record("info", "scan", format!("Found {} wallpapers; engine={engine:?}", items.len()));
    *state.records.lock().map_err(|e| e.to_string())? = records;
    *state.engine.lock().map_err(|e| e.to_string())? = engine;
    Ok(items)
}

fn preview_data(path: &Path, cache_dir: Option<PathBuf>) -> Result<Option<String>, String> {
    image_preview(path, cache_dir, false)
}

fn image_preview(path: &Path, cache_dir: Option<PathBuf>, detailed: bool) -> Result<Option<String>, String> {
    let meta = fs::metadata(path).map_err(|e| e.to_string())?;
    if meta.len() > if detailed { 64 * 1024 * 1024 } else { MAX_PREVIEW_BYTES } { return Ok(None); }
    let mut hash = DefaultHasher::new();
    path_key(path).hash(&mut hash);
    meta.len().hash(&mut hash);
    meta.modified().ok().hash(&mut hash);
    // Version the key when thumbnail settings change.
    ("panorama-cover-v4", detailed).hash(&mut hash);
    let cache_path = cache_dir.map(|dir| dir.join(format!("{:016x}.jpg", hash.finish())));
    if let Some(bytes) = cache_path.as_ref().and_then(|file| fs::read(file).ok()) {
        return Ok(Some(format!("data:image/jpeg;base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes))));
    }
    let decoded = ImageReader::open(path)
        .map_err(|e| format!("Preview open: {e}"))?
        .with_guessed_format().map_err(|e| e.to_string())?
        .decode().map_err(|e| format!("Preview decode: {e}"))?;
    let bytes = portrait_bytes(decoded, detailed)?;
    if let Some(file) = cache_path {
        if let Some(parent) = file.parent() {
            if fs::create_dir_all(parent).is_ok() { let _ = fs::write(file, &bytes); }
        }
    }
    Ok(Some(jpeg_data(&bytes)))
}

fn portrait_bytes(decoded: DynamicImage, detailed: bool) -> Result<Vec<u8>, String> {
    // Retain the entire artwork so the card can pan left/center/right without
    // another decode. Never enlarge a tiny Workshop preview and call it HD.
    let (width, height, quality) = if detailed { (2560, 1440, 96) } else { (280, 224, 82) };
    let scaled = if decoded.width() > width || decoded.height() > height {
        decoded.resize(width, height, image::imageops::FilterType::Lanczos3)
    } else { decoded };
    let thumb = DynamicImage::ImageRgb8(scaled.to_rgb8());
    let mut bytes = Vec::new();
    JpegEncoder::new_with_quality(&mut bytes, quality)
        .encode_image(&thumb).map_err(|e| e.to_string())?;
    Ok(bytes)
}

fn jpeg_data(bytes: &[u8]) -> String {
    format!("data:image/jpeg;base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes))
}

fn original_scene_cover(record: &WallpaperRecord, cache_dir: &Path) -> Option<String> {
    let revision = crate::covers::revision(&record.target);
    let mut hash = DefaultHasher::new();
    ("source-panorama-v4", path_key(&record.target), revision).hash(&mut hash);
    let cache = cache_dir.join(format!("source-{:016x}.jpg", hash.finish()));
    if let Ok(bytes) = fs::read(&cache) { return Some(jpeg_data(&bytes)); }
    let image = crate::covers::scene_image(&record.target, record.preview.as_deref())?;
    crate::diagnostics::record("debug", "preview", format!("{}: generated scene cover from source layers, {}x{}", record.public.id, image.width(), image.height()));
    let bytes = portrait_bytes(image, true).ok()?;
    if fs::create_dir_all(cache_dir).is_ok() { let _ = fs::write(cache, &bytes); }
    Some(jpeg_data(&bytes))
}

fn video_poster(path: &Path, cache_dir: &Path) -> Option<String> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT_POSTER: AtomicU64 = AtomicU64::new(0);
    let metadata = fs::metadata(path).ok()?;
    let mut hash = DefaultHasher::new();
    ("video-panorama-v2", path_key(path), metadata.len(), metadata.modified().ok()).hash(&mut hash);
    let key = hash.finish();
    let cache = cache_dir.join(format!("video-{key:016x}.jpg"));
    if let Ok(bytes) = fs::read(&cache) {
        return Some(format!("data:image/jpeg;base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes)));
    }
    let ffmpeg = std::env::var_os("PATH").into_iter().flat_map(|paths| std::env::split_paths(&paths).collect::<Vec<_>>())
        .map(|dir| dir.join(if cfg!(windows) { "ffmpeg.exe" } else { "ffmpeg" }))
        .chain(std::iter::once(PathBuf::from(r"C:\ffmpeg\bin\ffmpeg.exe"))).find(|path| path.is_file())?;
    fs::create_dir_all(cache_dir).ok()?;
    let temporary = cache_dir.join(format!("video-{key:016x}-{}-{}.jpg", std::process::id(), NEXT_POSTER.fetch_add(1, Ordering::Relaxed)));
    let mut child = engine_command(&ffmpeg).args(["-nostdin", "-hide_banner", "-loglevel", "error", "-protocol_whitelist", "file,pipe", "-ss", "1", "-i"])
        .arg(path).args(["-frames:v", "1", "-vf", "scale=w='min(2560,iw)':h='min(1440,ih)':force_original_aspect_ratio=decrease", "-q:v", "1", "-y"])
        .arg(&temporary).spawn().ok()?;
    let start = std::time::Instant::now();
    let success = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.success(),
            Err(_) => break false,
            _ if start.elapsed() > std::time::Duration::from_secs(4) => { let _ = child.kill(); let _ = child.wait(); break false; }
            _ => std::thread::sleep(std::time::Duration::from_millis(20)),
        }
    };
    let bytes = if success { fs::read(&temporary).ok() } else { None };
    if let Some(bytes) = &bytes { let _ = fs::write(cache, bytes); }
    let _ = fs::remove_file(temporary);
    bytes.map(|bytes| format!("data:image/jpeg;base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes)))
}

#[tauri::command]
pub async fn rebuild_cover_cache(app: tauri::AppHandle, window: tauri::WebviewWindow) -> Result<(),String> {
    if window.label()!="settings" { return Err("Cover rebuilding is only available in Settings.".into()); }
    tauri::async_runtime::spawn_blocking(move || {
        let root=app.path().app_cache_dir().map_err(|e| e.to_string())?.join("previews");
        crate::cover_cache::rebuild(&root)?;
        crate::diagnostics::record("info","preview","Cover cache rebuilt. Visible covers will regenerate on the next selector opening.");
        Ok(())
    }).await.map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn wallpaper_preview(id: String, quality: Option<String>, state: State<'_, Gallery>, app: tauri::AppHandle) -> Result<Option<String>, String> {
    let record = state.records.lock().map_err(|e| e.to_string())?.get(&id).cloned();
    let Some(record) = record else { return Ok(None); };
    let cache_dir = app.path().app_cache_dir().ok().map(|dir| crate::cover_cache::directory(&dir.join("previews")));
    let preview_id = id.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        let detailed = quality.as_deref() == Some("high");
        if detailed && record.public.kind == "scene" {
            if let Some(data) = cache_dir.as_deref().and_then(|dir| original_scene_cover(&record, dir)) { return Ok(Some(data)); }
            crate::diagnostics::record("debug", "preview", format!("{}: no source artwork matching the composed preview; using author preview", record.public.id));
        }
        if detailed && record.public.kind == "video" {
            if let Some(data) = cache_dir.as_deref().and_then(|dir| video_poster(&record.target, dir)) { return Ok(Some(data)); }
            crate::diagnostics::record("debug", "preview", format!("{}: video frame unavailable; using author preview", record.public.id));
        }
        let Some(path) = record.preview else { return Ok(None); };
        if detailed {
            if let Some((width,height)) = ImageReader::open(&path).ok().and_then(|reader| reader.with_guessed_format().ok()).and_then(|reader| reader.into_dimensions().ok()) {
                crate::diagnostics::record("debug","preview",format!("{}: author preview fallback, {width}x{height}",record.public.id));
            }
        }
        if detailed { image_preview(&path, cache_dir, true) } else { preview_data(&path, cache_dir) }
    }).await.map_err(|e| e.to_string())?;
    if let Err(error) = &result { crate::diagnostics::record("warn", "preview", format!("{preview_id}: {error}")); }
    result
}

#[tauri::command]
pub async fn apply_wallpaper(id: String, monitor: u32, app: tauri::AppHandle) -> Result<(), String> {
    // Pin the original selection's display before a reopened gallery can move.
    let display = crate::target_monitor(&app)?;
    tauri::async_runtime::spawn_blocking(move || {
        let result = apply(&app, &id, monitor, &display);
        if let Err(error) = &result { crate::diagnostics::record("error", "apply", error); }
        result
    }).await.map_err(|e| e.to_string())?
}

fn apply(app: &tauri::AppHandle, id: &str, monitor: u32, display: &str) -> Result<(), String> {
    let state = app.state::<Gallery>();
    let _operation = state.operation.lock().map_err(|e| e.to_string())?;
    if state.quitting.load(Ordering::Acquire) { return Err("Wallflow is quitting".into()); }
    let record = state.records.lock().map_err(|e| e.to_string())?
        .get(id).cloned().ok_or("Wallpaper not found in current local scan")?;
    let target = &record.target;
    if !target.is_file() { return Err("Wallpaper was moved or deleted".into()); }
    let executable = state.engine.lock().map_err(|e| e.to_string())?
        .clone().ok_or("Wallpaper Engine installation was not found in Steam libraries")?;
    if !executable.is_file() { return Err("Wallpaper Engine executable no longer exists".into()); }
    let config = EngineConfig::read(&executable)?;
    if config.monitor_index(display).or_else(|| single_monitor(&config)).map(|physical| config.wallpaper_monitor(physical)) != Some(monitor) {
        return Err("The target display changed. Reopen Wallflow before selecting a wallpaper.".into());
    }
    let identity = launch_target(target);
    let properties = config.properties_for(&identity, monitor);
    let launch = command_target(&record, &identity);
    if launch != identity { crate::diagnostics::record("debug", "apply", "Using project metadata to preserve the Unicode wallpaper filename"); }
    let preserve = crate::settings::preserve_playback(app);
    let paused = capture_pause(&executable, preserve);
    if paused == Some(true) {
        // Let the existing short exit animation finish before capturing pixels;
        // do not preserve an image of the selector on the desktop itself.
        if let Some(window) = app.get_webview_window("main") {
            let start = std::time::Instant::now();
            while window.is_visible().map_err(|e| e.to_string())? {
                if start.elapsed() > std::time::Duration::from_millis(350) { return Err("Close the selector before switching a paused wallpaper".into()); }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }
    }
    change_with_playback(&executable, preserve, paused, || {
        if crate::settings::sync_playlist(app) {
            if let Some(selected) = selected_playlist(app) {
                if config.monitor_playlists.get(&monitor).map(|playlist| &playlist.name) != Some(&selected) {
                    open_playlist(&executable, &config, &selected, monitor)?;
                }
            }
        }
        // WE keys saved settings by the actual wallpaper file and ordinary Windows paths.
        let file = launch.to_string_lossy();
        let file = file.strip_prefix(r"\\?\").unwrap_or(&file);
        run_display_control(&executable, monitor, &["-control", "openWallpaper", "-file", file])?;
        restore_properties(&executable, properties, monitor)?;
        if !preserve {
            run_control(&executable, &["-control", "play"])?;
            run_control(&executable, &["-control", "unmute"])?;
        }
        Ok(())
    })?;
    crate::diagnostics::record("info", "apply", format!("Applied {id} on Monitor{monitor}; preserve pause/sound={preserve}; captured pause={}", paused.map(|p| p.to_string()).unwrap_or_else(|| "unavailable".into())));
    Ok(())
}

fn single_monitor(config: &EngineConfig) -> Option<u32> {
    if config.current.len() <= 1 { config.current.keys().next().copied().or(Some(0)) } else { None }
}

fn launch_target(target: &Path) -> PathBuf {
    if target.file_name().is_some_and(|name| name.eq_ignore_ascii_case("project.json")) {
        if let Some(parent) = target.parent() {
            if let Some(file) = fs::read(target).ok().and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
                .and_then(|json| json["file"].as_str().map(str::to_owned))
                .and_then(|file| path_in_folder(parent, &file)) { return file; }
        }
    }
    target.to_owned()
}

fn command_target(record: &WallpaperRecord, identity: &Path) -> PathBuf {
    // WE's CLI can replace non-ANSI characters with '?' even when CreateProcessW
    // receives Unicode. Its UTF-8 project reader resolves the original media name.
    if !identity.to_string_lossy().is_ascii() {
        if let Some(project) = record.project.as_ref().filter(|path| path.is_file() && path.to_string_lossy().is_ascii()) { return project.clone(); }
    }
    identity.to_owned()
}

fn restore_properties(executable: &Path, properties: Option<Value>, monitor: u32) -> Result<(), String> {
    if let Some(properties) = properties.filter(|value| value.as_object().is_some_and(|object| !object.is_empty())) {
        crate::diagnostics::record("debug", "properties", format!("Restoring {} saved properties on Monitor{monitor}", properties.as_object().unwrap().len()));
        run_display_control(executable, monitor, &["-control", "applyProperties", "-properties", &format!("RAW~({properties})~END")])?;
    }
    Ok(())
}

fn open_playlist(executable: &Path, config: &EngineConfig, name: &str, monitor: u32) -> Result<(), String> {
    if !config.playlists.iter().any(|playlist| playlist.name == name) { return Err("Save this playlist in Wallpaper Engine before synchronizing it.".into()); }
    run_display_control(executable, monitor, &["-control", "openPlaylist", "-playlist", name])
}

pub fn synchronize_playlist(app: &tauri::AppHandle, name: &str, display: &str) -> Result<(), String> {
    let state = app.state::<Gallery>();
    let _operation = state.operation.lock().map_err(|e| e.to_string())?;
    if state.quitting.load(Ordering::Acquire) { return Err("Wallflow is quitting".into()); }
    let executable = state.engine.lock().map_err(|e| e.to_string())?.clone().ok_or("Wallpaper Engine not found")?;
    let config = EngineConfig::read(&executable)?;
    let physical = config.monitor_index(display).or_else(|| single_monitor(&config)).ok_or("Cannot match the Settings display with Wallpaper Engine")?;
    let monitor = config.wallpaper_monitor(physical);
    let preserve = crate::settings::preserve_playback(app);
    let paused = capture_pause(&executable, preserve);
    change_with_playback(&executable, preserve, paused, || open_playlist(&executable, &config, name, monitor))
}

fn change_with_playback(executable: &Path, preserve: bool, paused: Option<bool>, operation: impl FnOnce() -> Result<(), String>) -> Result<(), String> {
    let hold = if paused == Some(true) { Some(crate::frame_hold::Hold::begin(executable)?) } else { None };
    crate::audio_transition::protect(executable, preserve, || {
        if paused == Some(true) {
            crate::diagnostics::record("debug", "playback", "Loading replacement behind a static desktop frame; no temporary Play command");
        }
        operation()
    }, |result| {
        let result = finish_playback(executable, paused, result);
        if paused == Some(true) {
            if let Err(error) = crate::playback_state::repaint_paused(executable) {
                crate::diagnostics::record("warn", "playback", format!("Could not request a paused video repaint: {error}"));
            }
        }
        if let Some(hold) = &hold { hold.settle_pause(); }
        result
    })
}

fn capture_pause(executable: &Path, preserve: bool) -> Option<bool> {
    if !preserve { return None; }
    match crate::playback_state::paused(executable) {
        Ok(paused) => {
            crate::diagnostics::record("debug", "playback", if paused { "Keeping manual pause after the wallpaper change" } else { "Keeping playback enabled; automatic performance rules remain in control" });
            Some(paused)
        }
        Err(error) => {
            // Audio protection remains available on an unrecognized future WE
            // version. Never guess pause from CPU usage or automatic rules.
            crate::diagnostics::record("warn", "playback", format!("Could not capture pause state; pause preservation is unavailable for this change: {error}"));
            None
        }
    }
}

fn finish_playback(executable: &Path, paused: Option<bool>, result: Result<(), String>) -> Result<(), String> {
    let restore = if paused == Some(true) { run_control(executable, &["-control", "pause"]) } else { Ok(()) };
    match (result, restore) {
        (Err(error), Err(restore)) => Err(format!("{error}; could not restore pause: {restore}")),
        (Err(error), _) | (_, Err(error)) => Err(error),
        _ => Ok(()),
    }
}

fn display_args(monitor: u32, args: &[&str]) -> Vec<String> {
    // Name WE's persistent location explicitly. Shared clone/stretch layouts
    // are resolved to their common renderer before constructing the command.
    args.iter().map(|arg| (*arg).to_owned()).chain(["-location".into(), format!("Monitor{monitor}")]).collect()
}

fn run_display_control(executable: &Path, monitor: u32, args: &[&str]) -> Result<(), String> {
    let args = display_args(monitor, args);
    run_control(executable, &args.iter().map(String::as_str).collect::<Vec<_>>())
}

fn run_control(executable: &Path, args: &[&str]) -> Result<(), String> {
    crate::diagnostics::record("debug", "engine.command", args.join(" "));
    let operation = args.windows(2).find(|pair| pair[0] == "-control").map(|pair| pair[1]).unwrap_or("command");
    let location = args.windows(2).find(|pair| pair[0] == "-location").map(|pair| pair[1]).unwrap_or("all displays");
    let mut child = engine_command(executable).args(args).spawn().map_err(|e| format!("Couldn't send command to Wallpaper Engine: {e}"))?;
    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => {
                crate::diagnostics::record("debug", "engine.result", format!("{operation} on {location} completed in {} ms", start.elapsed().as_millis()));
                return Ok(());
            }
            Ok(Some(status)) => return Err(format!("Wallpaper Engine {operation} on {location} failed: {status}")),
            Err(error) => { let _ = child.kill(); let _ = child.wait(); return Err(error.to_string()); }
            _ if start.elapsed() > std::time::Duration::from_secs(4) => {
                let _ = child.kill(); let _ = child.wait(); return Err(format!("Wallpaper Engine {operation} on {location} timed out"));
            }
            _ => std::thread::sleep(std::time::Duration::from_millis(20)),
        }
    }
}

fn engine_command(executable: &Path) -> Command {
    let mut command = Command::new(executable);
    if let Some(folder) = executable.parent() { command.current_dir(folder); }
    command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    #[cfg(windows)] {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000); // CREATE_NO_WINDOW: no helper console stealing focus.
    }
    command
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("wallflow-test-{}-{}-{}", std::process::id(), timestamp(SystemTime::now()), NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
        fn project(&self, relative: &str, extra: &str) -> PathBuf {
            let path = self.0.join(relative);
            fs::create_dir_all(&path).unwrap();
            fs::write(path.join("project.json"), format!(r#"{{"title":"Test","type":"scene"{extra}}}"#)).unwrap();
            path
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); }
    }
    #[test]
    fn steam_id_is_correct() { assert_eq!(STEAM_APP_ID, "431960"); }
    #[test]
    fn path_rejects_nonexistent() { assert!(path_in_folder(Path::new("."), "file-does-not-exist.test").is_none()); }
    #[test]
    fn scan_deduplicates_roots_and_published_copies_and_excludes_defaults() {
        let fixture = Fixture::new();
        fixture.project("steamapps/workshop/content/431960/12345", "");
        fixture.project("steamapps/common/wallpaper_engine/projects/myprojects/published", r#", "workshopid":"12345""#);
        fixture.project("steamapps/common/wallpaper_engine/projects/myprojects/unpublished", "");
        fixture.project("steamapps/common/wallpaper_engine/projects/defaultprojects/builtin", "");
        let (records, _) = scan_roots(vec![fixture.0.clone(), fixture.0.join(".")]);
        assert_eq!(records.len(), 2);
        assert_eq!(records["workshop:12345"].public.source, "workshop");
        assert!(records.values().all(|record| !record.target.to_string_lossy().contains("defaultprojects")));
    }
    #[test]
    fn installed_date_controls_order_with_stable_ties() {
        let fixture = Fixture::new();
        let path = fixture.project("sample", "");
        let base = project(&path, "local").unwrap();
        let mut records = HashMap::new();
        for (id, date) in [("old", 10), ("new-b", 50), ("new-a", 50)] {
            let mut item = base.clone();
            item.public.id = id.into();
            item.public.added_at = date;
            records.insert(id.into(), item);
        }
        assert_eq!(ordered_items(&records).iter().map(|item| item.id.as_str()).collect::<Vec<_>>(), vec!["new-a", "new-b", "old"]);
    }
    #[test]
    fn playlist_order_overrides_install_dates_and_ignores_duplicates_and_missing_items() {
        let fixture = Fixture::new();
        let mut first = project(&fixture.project("first", ""), "local").unwrap();
        let mut second = project(&fixture.project("second", ""), "local").unwrap();
        first.public.added_at = 100;
        second.public.added_at = 1;
        let files = vec![second.target.to_string_lossy().into_owned(), "missing".into(), first.target.to_string_lossy().into_owned(), second.target.to_string_lossy().into_owned()];
        let records = HashMap::from([(first.public.id.clone(), first.clone()), (second.public.id.clone(), second.clone())]);
        assert_eq!(filter_records(&records, Some(&files)).iter().map(|item| &item.id).collect::<Vec<_>>(), vec![&second.public.id, &first.public.id]);
        assert_eq!(filter_records(&records, None)[0].id, first.public.id);
    }
    #[test]
    fn scene_launch_uses_the_engine_settings_identity_and_rejects_project_path_escape() {
        let fixture = Fixture::new();
        let folder = fixture.project("scene", r#", "file":"scene.pkg""#);
        fs::write(folder.join("scene.pkg"), "test").unwrap();
        let target = folder.join("project.json").canonicalize().unwrap();
        assert_eq!(launch_target(&target), folder.join("scene.pkg").canonicalize().unwrap());
        fs::write(fixture.0.join("outside.pkg"), "test").unwrap();
        fs::write(&target, r#"{"file":"../outside.pkg"}"#).unwrap();
        assert_eq!(launch_target(&target), target);
    }
    #[test]
    fn new_playlist_video_is_discovered_after_initial_scan_with_its_installed_workshop_id() {
        let fixture = Fixture::new();
        let (mut records, _) = scan_roots(vec![fixture.0.clone()]);
        assert!(records.is_empty());
        let folder = fixture.project("steamapps/workshop/content/431960/12345", "");
        let filename = "Dance_極楽浄土.mp4";
        fs::write(folder.join(filename), b"video").unwrap();
        fs::write(folder.join("project.json"), serde_json::to_vec(&serde_json::json!({
            "title":"New video", "type":"Video", "file":filename, "workshopid":"99999"
        })).unwrap()).unwrap();
        let file = folder.join(filename).to_string_lossy().into_owned();
        reconcile_playlist_records(&mut records, &mut HashMap::new(), vec![file.clone(), file.clone()]);
        assert_eq!(records.len(), 1);
        assert_eq!(records["workshop:12345"].public.kind, "video");
        assert_eq!(filter_records(&records, Some(&[file]))[0].id, "workshop:12345");
    }
    #[test]
    fn missing_playlist_items_report_the_reason_and_recover_after_installation() {
        let fixture = Fixture::new();
        let folder = fixture.0.join("missing");
        let file = folder.join("scene.pkg").to_string_lossy().into_owned();
        let mut records = HashMap::new();
        let mut unavailable = HashMap::new();
        reconcile_playlist_records(&mut records, &mut unavailable, vec![file.clone(), file.clone()]);
        assert_eq!(unavailable.len(), 1);
        assert_eq!(unavailable[&normalize_path(&file)], "Wallpaper folder is missing from disk");
        fs::create_dir(&folder).unwrap();
        reconcile_playlist_records(&mut records, &mut unavailable, vec![file.clone()]);
        assert_eq!(unavailable[&normalize_path(&file)], "Playlist target file is missing from disk");
        fs::write(&file, b"scene").unwrap();
        reconcile_playlist_records(&mut records, &mut unavailable, vec![file.clone()]);
        assert_eq!(unavailable[&normalize_path(&file)], "No project.json found; this is not a supported standalone image");
        fs::write(folder.join("project.json"), r#"{"type":"scene","file":"scene.pkg","title":"Installed"}"#).unwrap();
        reconcile_playlist_records(&mut records, &mut unavailable, vec![file]);
        assert_eq!(records.len(), 1);
        assert!(unavailable.is_empty());
    }
    #[test]
    fn unicode_video_uses_metadata_for_the_command_but_keeps_its_media_settings_identity() {
        let fixture = Fixture::new();
        let folder = fixture.project("video", "");
        let filename = "少女映画.mp4";
        fs::write(folder.join(filename), b"video").unwrap();
        fs::write(folder.join("project.json"), serde_json::to_vec(&serde_json::json!({
            "title":"Video", "type":"video", "file":filename
        })).unwrap()).unwrap();
        let mut record = project(&folder, "local").unwrap();
        let identity = launch_target(&record.target);
        assert_eq!(identity, folder.join(filename).canonicalize().unwrap());
        assert_eq!(command_target(&record, &identity), folder.join("project.json").canonicalize().unwrap());
        let mut config = EngineConfig::default();
        config.properties.insert(normalize_path(&identity.to_string_lossy()), serde_json::json!({"Monitor0":{"volume":0}}));
        assert_eq!(config.properties_for(&identity, 0).unwrap()["volume"], 0);
        record.project = None;
        assert_eq!(command_target(&record, &identity), identity);
        let ascii = folder.join("video.mp4");
        fs::write(&ascii, b"video").unwrap();
        assert_eq!(command_target(&record, &ascii), ascii);
    }
    #[test]
    #[ignore = "set WALLFLOW_PLAYLIST_TEST_FILE to an installed wallpaper file; read-only"]
    fn installed_playlist_item_is_discovered_without_a_rescan() {
        let file = std::env::var("WALLFLOW_PLAYLIST_TEST_FILE").expect("WALLFLOW_PLAYLIST_TEST_FILE is required");
        let mut records = HashMap::new();
        reconcile_playlist_records(&mut records, &mut HashMap::new(), vec![file.clone()]);
        assert_eq!(records.len(), 1, "Installed playlist file must be discoverable");
        let record = records.values().next().unwrap();
        assert!(matches_file(record, &file));
        let identity = launch_target(&record.target);
        let command = command_target(record, &identity);
        assert!(command.is_file());
        eprintln!("Discovered {}; command target {}", record.public.id, command.display());
    }
    #[test]
    fn playing_or_disabled_preservation_never_forces_play_or_pause() {
        let missing = Path::new("wallflow-test-engine-that-does-not-exist.exe");
        assert_eq!(capture_pause(missing, false), None);
        for paused in [None, Some(false)] {
            assert!(finish_playback(missing, paused, Ok(())).is_ok());
            assert_eq!(finish_playback(missing, paused, Err("Wallpaper failed".into())), Err("Wallpaper failed".into()));
        }
        let error = finish_playback(missing, Some(true), Err("Properties failed".into())).unwrap_err();
        assert!(error.starts_with("Properties failed; could not restore pause:"));
    }
    #[test]
    #[cfg(windows)]
    #[ignore = "Reloads the current wallpaper in all pause/mute combinations; restores original manual state"]
    fn preserves_manual_pause_when_reloading_the_live_wallpaper() {
        let executable = engine_executable(&libraries()).unwrap();
        let config = EngineConfig::read(&executable).unwrap();
        let original = crate::playback_state::paused(&executable).unwrap();
        let original_mute = crate::playback_state::muted(&executable).unwrap();
        struct Restore<'a>(&'a Path, bool, bool, Option<(u32, PathBuf, Option<Value>)>);
        impl Drop for Restore<'_> {
            fn drop(&mut self) {
                if let Some((monitor, target, properties)) = &self.3 {
                    let _ = change_with_playback(self.0, true, Some(self.1), || {
                        run_display_control(self.0, *monitor, &["-control", "openWallpaper", "-file", &target.to_string_lossy()])?;
                        restore_properties(self.0, properties.clone(), *monitor)
                    });
                }
                let _ = run_control(self.0, &["-control", if self.1 { "pause" } else { "play" }]);
                let _ = run_control(self.0, &["-control", if self.2 { "mute" } else { "unmute" }]);
            }
        }
        let mut restore = Restore(&executable, original, original_mute, None);
        let monitor = *config.current.keys().min().unwrap();
        let output = crate::engine_query::query(&executable, monitor).unwrap();
        let file = wallpaper_output(&output).expect("Live wallpaper query must return a file");
        let target = PathBuf::from(&file);
        assert!(target.is_file());
        let folder = target.parent().unwrap();
        let record = project(folder, "local").expect("Current wallpaper must have project metadata");
        let identity = launch_target(&record.target);
        let launch = command_target(&record, &identity);
        let replacements = if let Some(folder) = std::env::var_os("WALLFLOW_VIDEO_SWITCH_TEST_DIR") {
            restore.3 = Some((monitor, launch, config.properties_for(&identity, monitor)));
            ["red", "blue"].into_iter().map(|name| project(&PathBuf::from(&folder).join(name), "local").expect("Synthetic video fixture must have project metadata")).collect::<Vec<_>>()
        } else { vec![record] };
        for (paused, muted) in [(true, true), (true, false), (false, true), (false, false)] {
          for record in &replacements {
            let identity = launch_target(&record.target);
            let launch = command_target(record, &identity);
            run_control(&executable, &["-control", if paused { "pause" } else { "play" }]).unwrap();
            run_control(&executable, &["-control", if muted { "mute" } else { "unmute" }]).unwrap();
            assert_eq!(capture_pause(&executable, true), Some(paused));
            change_with_playback(&executable, true, Some(paused), || {
                assert_eq!(crate::playback_state::paused(&executable).unwrap(), paused, "Loading must never temporarily resume a paused wallpaper");
                run_display_control(&executable, monitor, &["-control", "openWallpaper", "-file", &launch.to_string_lossy()])?;
                // WE clears manual pause inside openWallpaper. A static cover
                // must remain present until the final Pause has settled.
                restore_properties(&executable, config.properties_for(&identity, monitor), monitor)
            }).unwrap();
            let live = wallpaper_output(&crate::engine_query::query(&executable, monitor).unwrap()).unwrap();
            assert!(matches_file(record, &live), "Engine must select the replacement, not keep the previous video");
            assert_eq!(crate::playback_state::paused(&executable).unwrap(), paused);
            assert_eq!(crate::playback_state::muted(&executable).unwrap(), muted);
            // The same control used by the Engine shortcut must remain usable
            // after the renderer switch, in either playback state.
            run_control(&executable, &["-control", if muted { "unmute" } else { "mute" }]).unwrap();
            assert_eq!(crate::playback_state::muted(&executable).unwrap(), !muted);
            assert_eq!(crate::playback_state::paused(&executable).unwrap(), paused);
            eprintln!("Verified wallpaper reload and mute toggle on Monitor{monitor}: paused={paused}, initial mute={muted}");
          }
        }
    }
    #[test]
    #[cfg(windows)]
    fn control_waits_for_success_and_reports_a_failed_process() {
        let executable = PathBuf::from(std::env::var_os("SystemRoot").unwrap()).join("System32/WindowsPowerShell/v1.0/powershell.exe");
        run_control(&executable, &["-NoProfile", "-NonInteractive", "-Command", "exit 0"]).unwrap();
        assert!(run_control(&executable, &["-NoProfile", "-NonInteractive", "-Command", "exit 7"]).unwrap_err().contains("failed"));
    }
    #[test]
    fn display_commands_address_persistent_secondary_location_without_changing_arguments() {
        for args in [
            vec!["-control", "openWallpaper", "-file", "C:/A wallpaper/scene.pkg"],
            vec!["-control", "openPlaylist", "-playlist", "Playlist & name"],
            vec!["-control", "applyProperties", "-properties", r#"RAW~({"volume":0})~END"#],
        ] {
            let command = display_args(1, &args);
            assert_eq!(&command[..args.len()], args.as_slice());
            assert_eq!(&command[args.len()..], ["-location", "Monitor1"]);
            assert!(!command.iter().any(|arg| arg == "-monitor"));
        }
    }
    #[test]
    fn playlist_membership_matches_scene_files_and_keeps_empty_playlists_empty() {
        let fixture = Fixture::new();
        let folder = fixture.project("scene", "");
        let other = fixture.project("other", "");
        let record = project(&folder, "local").unwrap();
        let other = project(&other, "local").unwrap();
        let mut records = HashMap::new();
        records.insert(record.public.id.clone(), record.clone());
        records.insert(other.public.id.clone(), other);
        let files = vec![folder.join("scene.pkg").to_string_lossy().into_owned(), folder.join("scene.pkg").to_string_lossy().into_owned()];
        let items = filter_records(&records, Some(&files));
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, record.public.id);
        assert_eq!(filter_records(&records, Some(&[])).len(), 0);
        assert_eq!(filter_records(&records, None).len(), 2);
        assert!(matches_file(&record, &folder.join("scene.json").to_string_lossy()));
    }
    #[test]
    fn current_selection_prefers_live_state_and_uses_saved_state_for_invalid_cli_results() {
        let fixture = Fixture::new();
        let first = project(&fixture.project("first", ""), "local").unwrap();
        let second = project(&fixture.project("second", ""), "local").unwrap();
        let records = HashMap::from([(first.public.id.clone(), first.clone()), (second.public.id.clone(), second.clone())]);
        let live = second.target.parent().unwrap().join("scene.pkg").to_string_lossy().into_owned();
        let saved = first.target.to_string_lossy();
        assert_eq!(active_record(&records, Some(&live), Some(&saved)), Some(second.public.id));
        assert_eq!(active_record(&records, Some("unknown path"), Some(&saved)), Some(first.public.id.clone()));
        assert_eq!(active_record(&records, None, Some(&saved)), Some(first.public.id));
    }
    #[test]
    fn current_selection_parses_terminal_utf8_and_utf16_without_accepting_diagnostics() {
        let path = std::env::temp_dir().join("壁紙 preview.png").to_string_lossy().into_owned();
        let terminal = format!("\u{1b}[?25l\u{1b}[2J\u{1b}[H{path}\r\n\u{1b}[?25h");
        assert_eq!(wallpaper_output(terminal.as_bytes()), Some(path.clone()));
        let unicode: Vec<_> = format!("\u{feff}\"{path}\"\r\n").encode_utf16().flat_map(u16::to_le_bytes).collect();
        assert_eq!(wallpaper_output(&unicode), Some(path));
        assert_eq!(wallpaper_output(b"Wallpaper Engine error: not running"), None);
        assert_eq!(wallpaper_output(b""), None);
    }
    #[test]
    #[cfg(windows)]
    #[ignore = "requires Wallpaper Engine running in the interactive desktop session"]
    fn reads_live_wallpaper_from_engine_without_a_visible_terminal() {
        let executable = engine_executable(&libraries()).unwrap();
        let config = EngineConfig::read(&executable).unwrap();
        assert!(!config.current.is_empty());
        for monitor in config.current.keys() {
            let output = crate::engine_query::query(&executable, *monitor).expect("live console query must succeed");
            let file = wallpaper_output(&output).unwrap_or_else(|| panic!("live console response: {:?}", String::from_utf8_lossy(&output)));
            eprintln!("Monitor{monitor}: {file}");
            assert!(Path::new(&normalize_path(&file)).is_file(), "{file}");
        }
    }
    #[test]
    fn playlist_imported_images_can_be_previewed_but_unrelated_files_cannot() {
        let fixture = Fixture::new();
        let image = fixture.0.join("portrait.png");
        image::RgbImage::new(20, 30).save(&image).unwrap();
        let record = imported_image(image.to_str().unwrap()).unwrap();
        assert_eq!(record.public.kind, "image");
        assert!(matches_file(&record, image.to_str().unwrap()));
        let unrelated = fixture.0.join("document.txt");
        fs::write(&unrelated, "text").unwrap();
        assert!(imported_image(unrelated.to_str().unwrap()).is_none());
    }
    #[test]
    fn preview_is_a_small_cached_image_and_invalidates_when_source_changes() {
        let fixture = Fixture::new();
        let source = fixture.0.join("preview.png");
        let cache = fixture.0.join("cache");
        image::RgbImage::from_pixel(1200, 900, image::Rgb([40, 70, 90])).save(&source).unwrap();
        let first = preview_data(&source, Some(cache.clone())).unwrap().unwrap();
        let bytes = base64::engine::general_purpose::STANDARD.decode(first.split_once(',').unwrap().1).unwrap();
        let thumb = image::load_from_memory(&bytes).unwrap();
        assert!(thumb.width() <= 280 && thumb.height() <= 560);
        assert_eq!(preview_data(&source, Some(cache.clone())).unwrap().unwrap(), first);
        assert_eq!(fs::read_dir(&cache).unwrap().count(), 1);
        image::RgbImage::from_pixel(800, 1200, image::Rgb([90, 20, 80])).save(&source).unwrap();
        assert_ne!(preview_data(&source, Some(cache.clone())).unwrap().unwrap(), first);
        assert_eq!(fs::read_dir(&cache).unwrap().count(), 2);
    }
    #[test]
    fn detailed_covers_retain_full_resolution_and_both_edges_for_panning() {
        let fixture = Fixture::new();
        let source = fixture.0.join("wide.png");
        let cache = fixture.0.join("cache");
        image::RgbImage::from_fn(1920, 1080, |x,_| if x < 640 { image::Rgb([240,10,10]) } else if x > 1280 { image::Rgb([10,10,240]) } else { image::Rgb([70,110,160]) }).save(&source).unwrap();
        let quick = image_preview(&source, Some(cache.clone()), false).unwrap().unwrap();
        let high = image_preview(&source, Some(cache.clone()), true).unwrap().unwrap();
        let decode = |data: &str| image::load_from_memory(&base64::engine::general_purpose::STANDARD.decode(data.split_once(',').unwrap().1).unwrap()).unwrap();
        assert_eq!((decode(&quick).width(), decode(&quick).height()), (280, 158));
        let panorama = decode(&high).to_rgb8();
        assert_eq!(panorama.dimensions(), (1920,1080));
        assert!(panorama.get_pixel(10,540)[0] > 230);
        assert!(panorama.get_pixel(1910,540)[2] > 230);
        assert_eq!(image_preview(&source, Some(cache.clone()), true).unwrap().unwrap(), high);
        assert_eq!(fs::read_dir(cache).unwrap().count(), 2);
    }
    #[test]
    fn scene_cover_uses_original_art_and_reuses_its_cache_until_the_source_changes() {
        let fixture = Fixture::new();
        let folder = fixture.project("original", r#", "preview":"preview.jpg""#);
        let original = folder.join("artwork.png");
        image::RgbImage::from_pixel(900,600,image::Rgb([10,80,190])).save(&original).unwrap();
        image::RgbImage::from_pixel(90,60,image::Rgb([10,80,190])).save(folder.join("preview.jpg")).unwrap();
        let record = project(&folder, "local").unwrap();
        let cache = fixture.0.join("cache");
        let first = original_scene_cover(&record, &cache).unwrap();
        let bytes = base64::engine::general_purpose::STANDARD.decode(first.split_once(',').unwrap().1).unwrap();
        let cover = image::load_from_memory(&bytes).unwrap().to_rgb8();
        assert_eq!(cover.dimensions(), (900,600));
        assert!(cover.get_pixel(450,300).0[0] < 30 && cover.get_pixel(450,300).0[2] > 150);
        assert_eq!(original_scene_cover(&record, &cache).unwrap(), first);
        assert_eq!(fs::read_dir(&cache).unwrap().count(), 1);
        image::RgbImage::from_pixel(901,601,image::Rgb([90,180,10])).save(&original).unwrap();
        image::RgbImage::from_pixel(90,60,image::Rgb([90,180,10])).save(folder.join("preview.jpg")).unwrap();
        assert_ne!(original_scene_cover(&record, &cache).unwrap(), first);
        assert_eq!(fs::read_dir(cache).unwrap().count(), 2);
    }
    #[test]
    #[ignore = "requires an installed FFmpeg"]
    fn video_frame_is_extracted_once_and_reused_from_cache() {
        let fixture = Fixture::new();
        let source = fixture.0.join("video.mp4");
        let status = engine_command(Path::new(r"C:\ffmpeg\bin\ffmpeg.exe"))
            .args(["-nostdin", "-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i", "color=c=blue:s=1280x720:r=1", "-t", "2", "-pix_fmt", "yuv420p", "-y"]).arg(&source).status().unwrap();
        assert!(status.success());
        let cache = fixture.0.join("cache");
        let first = video_poster(&source, &cache).unwrap();
        let bytes = base64::engine::general_purpose::STANDARD.decode(first.split_once(',').unwrap().1).unwrap();
        let image = image::load_from_memory(&bytes).unwrap();
        assert_eq!((image.width(), image.height()), (1280,720));
        assert_eq!(video_poster(&source, &cache).unwrap(), first);
        assert_eq!(fs::read_dir(cache).unwrap().count(), 1);
    }
    #[test]
    #[cfg(windows)]
    fn windows_path_identity_ignores_case_and_slashes() {
        assert_eq!(path_key(Path::new(r"C:\Steam\Wallpapers")), path_key(Path::new("c:/steam/wallpapers")));
    }
}
