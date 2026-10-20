use std::{fs, sync::{Mutex, atomic::{AtomicBool, Ordering}}};
use serde::Serialize;
use tauri::{Manager, State};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut};

pub const DEFAULT_SHORTCUT: &str = "Ctrl+Alt+G";
pub struct SettingsState { shortcut: Mutex<String>, preserve_playback: AtomicBool, sync_playlist: AtomicBool, amazing_animations: AtomicBool, recording_shortcut: AtomicBool }
impl Default for SettingsState {
    fn default() -> Self { Self { shortcut: Mutex::new(String::new()), preserve_playback: AtomicBool::new(true), sync_playlist: AtomicBool::new(true), amazing_animations: AtomicBool::new(true), recording_shortcut: AtomicBool::new(false) } }
}
pub fn recording_shortcut(app: &tauri::AppHandle) -> bool { app.state::<SettingsState>().recording_shortcut.load(Ordering::Acquire) }
pub fn stop_recording(app: &tauri::AppHandle) { app.state::<SettingsState>().recording_shortcut.store(false, Ordering::Release); }

#[tauri::command]
pub fn set_shortcut_recording(window: tauri::WebviewWindow, state: State<'_, SettingsState>, recording: bool) -> Result<(), String> {
    if window.label() != "settings" { return Err("Shortcut recording is only available in Settings.".into()); }
    state.recording_shortcut.store(recording, Ordering::Release);
    Ok(())
}
pub fn preserve_playback(app: &tauri::AppHandle) -> bool { app.state::<SettingsState>().preserve_playback.load(Ordering::Acquire) }
pub fn sync_playlist(app: &tauri::AppHandle) -> bool { app.state::<SettingsState>().sync_playlist.load(Ordering::Acquire) }
pub fn amazing_animations(app: &tauri::AppHandle) -> bool { app.state::<SettingsState>().amazing_animations.load(Ordering::Acquire) }

#[derive(Serialize)]
pub struct SettingsInfo { pub playlist: Option<String>, pub shortcut: String, pub playlists: Vec<String>, pub preserve_playback: bool, pub sync_playlist: bool, pub start_with_windows: bool, pub amazing_animations: bool, pub display_mode: &'static str }

pub fn show(app: &tauri::AppHandle) -> Result<(), String> {
    if let Some(window) = app.get_webview_window("settings") {
        return window.unminimize().and_then(|_| window.show()).and_then(|_| window.set_focus()).map_err(|e| e.to_string());
    }
    tauri::WebviewWindowBuilder::new(app, "settings", tauri::WebviewUrl::App("index.html?settings".into()))
        .title("Wallflow — Settings").inner_size(520.0, 760.0).min_inner_size(420.0, 420.0).center()
        .build().map_err(|e| e.to_string())?;
    crate::diagnostics::record("debug", "settings", "Opened standalone Settings window");
    Ok(())
}

fn validate_shortcut(shortcut: &str) -> Result<Shortcut, String> {
    let shortcut = shortcut.trim();
    if !shortcut.split('+').any(|part| ["ctrl", "control", "alt", "super", "meta", "command", "cmd"].contains(&part.to_lowercase().as_str())) {
        return Err("Use Ctrl, Alt or Super with another key, for example Ctrl+Alt+G.".into());
    }
    shortcut.parse().map_err(|_| "Invalid shortcut. Example: Ctrl+Alt+G or Ctrl+Shift+Space.".into())
}

pub fn initialize(app: &tauri::AppHandle) {
    let saved = app.path().app_config_dir().ok().and_then(|dir| fs::read(dir.join("gallery.json")).ok())
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok()).unwrap_or_default();
    app.state::<SettingsState>().preserve_playback.store(saved["preserve_playback"].as_bool().unwrap_or(true), Ordering::Release);
    app.state::<SettingsState>().sync_playlist.store(saved["sync_playlist"].as_bool().unwrap_or(true), Ordering::Release);
    app.state::<SettingsState>().amazing_animations.store(saved["amazing_animations"].as_bool().unwrap_or(true), Ordering::Release);
    let shortcut = saved["shortcut"].as_str().map(str::to_owned)
        .filter(|value| validate_shortcut(value).is_ok()).unwrap_or_else(|| DEFAULT_SHORTCUT.into());
    *app.state::<SettingsState>().shortcut.lock().unwrap() = shortcut.clone();
    if let Err(error) = crate::register_shortcut(app, &shortcut) { crate::diagnostics::record("error", "shortcut", format!("Global shortcut unavailable: {error}")); }
}

#[tauri::command]
pub fn get_settings(app: tauri::AppHandle, state: State<'_, SettingsState>) -> Result<SettingsInfo, String> {
    Ok(SettingsInfo { playlist: crate::wallpaper::selected_playlist(&app), shortcut: state.shortcut.lock().map_err(|e| e.to_string())?.clone(), playlists: crate::wallpaper::playlist_names(&app)?, preserve_playback: preserve_playback(&app), sync_playlist: sync_playlist(&app), start_with_windows: crate::startup::enabled()?, amazing_animations: amazing_animations(&app), display_mode: crate::wallpaper::display_mode(&app)? })
}

fn persist(app: &tauri::AppHandle, playlist: &Option<String>, shortcut: &str, preserve: bool, sync: bool, animations: bool) -> Result<(), String> {
    let dir = app.path().app_config_dir().map_err(|e| e.to_string())?;
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let temporary = dir.join("gallery.json.tmp");
    fs::write(&temporary, serde_json::to_vec_pretty(&serde_json::json!({"playlist":playlist,"shortcut":shortcut,"preserve_playback":preserve,"sync_playlist":sync,"amazing_animations":animations})).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    fs::rename(temporary, dir.join("gallery.json")).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn save_settings(app: tauri::AppHandle, window: tauri::WebviewWindow, playlist: Option<String>, shortcut: String, preserve_playback: bool, sync_playlist: bool, start_with_windows: bool, amazing_animations: bool) -> Result<(), String> {
    let display = window.current_monitor().map_err(|e| e.to_string())?.and_then(|monitor| monitor.name().cloned()).ok_or("Settings display is unavailable")?;
    tauri::async_runtime::spawn_blocking(move || save(&app, playlist, shortcut, preserve_playback, sync_playlist, start_with_windows, amazing_animations, &display)).await.map_err(|e| e.to_string())?
}

fn save(app: &tauri::AppHandle, playlist: Option<String>, shortcut: String, preserve: bool, sync: bool, start_with_windows: bool, animations: bool, display: &str) -> Result<(), String> {
    let state = app.state::<SettingsState>();
    let new = validate_shortcut(&shortcut)?;
    if let Some(name) = &playlist {
        if !crate::wallpaper::playlist_names(&app)?.contains(name) { return Err("This playlist no longer exists. Reopen settings to refresh the list.".into()); }
    }
    let mut current = state.shortcut.lock().map_err(|e| e.to_string())?;
    let old = validate_shortcut(&current)?;
    let changed = new != old;
    let mut startup_change = crate::startup::Change::apply(start_with_windows)?;
    // Register first: a conflicting shortcut cannot remove the existing working binding.
    if changed { crate::register_shortcut(&app, &shortcut).map_err(|e| format!("Shortcut unavailable: {e}. Your previous shortcut is still active."))?; }
    if let Err(error) = persist(app, &playlist, shortcut.trim(), preserve, sync, animations) {
        if changed { let _ = app.global_shortcut().unregister(new); }
        return Err(error);
    }
    if changed {
        if let Err(error) = app.global_shortcut().unregister(old) {
            let _ = app.global_shortcut().unregister(new);
            let _ = persist(app, &crate::wallpaper::selected_playlist(app), &current, preserve_playback(app), sync_playlist(app), amazing_animations(app));
            return Err(format!("Could not replace the shortcut: {error}"));
        }
    }
    if sync && (playlist != crate::wallpaper::selected_playlist(app) || !sync_playlist(app)) {
        if let Some(name) = &playlist {
            if let Err(error) = crate::wallpaper::synchronize_playlist(app, name, display) {
                if changed { let _ = app.global_shortcut().unregister(new); let _ = crate::register_shortcut(app, &current); }
                let _ = persist(app, &crate::wallpaper::selected_playlist(app), &current, preserve_playback(app), sync_playlist(app), amazing_animations(app));
                crate::diagnostics::record("error", "playlist", &error);
                return Err(error);
            }
        }
    }
    crate::wallpaper::set_playlist(app, playlist.clone())?;
    state.preserve_playback.store(preserve, Ordering::Release);
    state.sync_playlist.store(sync, Ordering::Release);
    state.amazing_animations.store(animations, Ordering::Release);
    *current = shortcut.trim().to_owned();
    startup_change.commit();
    crate::diagnostics::record("info", "settings", format!("Saved playlist={playlist:?}, shortcut={shortcut}, preserve playback={preserve}, sync playlist={sync}, start with Windows={start_with_windows}, amazing animations={animations}"));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shortcut_validation_preserves_regular_gallery_keys() {
        assert!(validate_shortcut("Ctrl+Alt+G").is_ok());
        assert!(validate_shortcut("Ctrl+Shift+Space").is_ok());
        for shortcut in ["G", "ArrowLeft", "Shift+G", "Ctrl+NotAKey", ""] { assert!(validate_shortcut(shortcut).is_err()); }
    }
    #[test]
    fn recorder_key_names_are_accepted_by_native_parser() {
        for shortcut in ["Ctrl+Alt+G", "Ctrl+Shift+Equal", "Alt+Shift+1", "Ctrl+Space", "Ctrl+ArrowLeft", "Ctrl+F12", "Ctrl+Numpad7", "Ctrl+NumpadAdd", "Ctrl+Enter", "Super+G"] {
            assert!(validate_shortcut(shortcut).is_ok(), "{shortcut}");
        }
    }
}
