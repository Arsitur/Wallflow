use std::{fs, sync::Mutex};
use serde::Serialize;
use tauri::{Manager, State};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut};

pub const DEFAULT_SHORTCUT: &str = "Ctrl+Alt+G";
#[derive(Default)]
pub struct SettingsState { shortcut: Mutex<String> }

#[derive(Serialize)]
pub struct SettingsInfo { pub playlist: Option<String>, pub shortcut: String, pub playlists: Vec<String> }

fn validate_shortcut(shortcut: &str) -> Result<Shortcut, String> {
    let shortcut = shortcut.trim();
    if !shortcut.split('+').any(|part| ["ctrl", "control", "alt", "super", "meta", "command", "cmd"].contains(&part.to_lowercase().as_str())) {
        return Err("Use Ctrl, Alt or Super with another key, for example Ctrl+Alt+G.".into());
    }
    shortcut.parse().map_err(|_| "Invalid shortcut. Example: Ctrl+Alt+G or Ctrl+Shift+Space.".into())
}

pub fn initialize(app: &tauri::AppHandle) {
    let shortcut = app.path().app_config_dir().ok().and_then(|dir| fs::read(dir.join("gallery.json")).ok())
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .and_then(|value| value["shortcut"].as_str().map(str::to_owned))
        .filter(|value| validate_shortcut(value).is_ok()).unwrap_or_else(|| DEFAULT_SHORTCUT.into());
    *app.state::<SettingsState>().shortcut.lock().unwrap() = shortcut.clone();
    if let Err(error) = crate::register_shortcut(app, &shortcut) { eprintln!("Global shortcut unavailable: {error}"); }
}

#[tauri::command]
pub fn get_settings(app: tauri::AppHandle, state: State<'_, SettingsState>) -> Result<SettingsInfo, String> {
    Ok(SettingsInfo { playlist: crate::wallpaper::selected_playlist(&app), shortcut: state.shortcut.lock().map_err(|e| e.to_string())?.clone(), playlists: crate::wallpaper::playlist_names(&app)? })
}

fn persist(app: &tauri::AppHandle, playlist: &Option<String>, shortcut: &str) -> Result<(), String> {
    let dir = app.path().app_config_dir().map_err(|e| e.to_string())?;
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let temporary = dir.join("gallery.json.tmp");
    fs::write(&temporary, serde_json::to_vec_pretty(&serde_json::json!({"playlist":playlist,"shortcut":shortcut})).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    fs::rename(temporary, dir.join("gallery.json")).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn save_settings(app: tauri::AppHandle, state: State<'_, SettingsState>, playlist: Option<String>, shortcut: String) -> Result<(), String> {
    let new = validate_shortcut(&shortcut)?;
    if let Some(name) = &playlist {
        if !crate::wallpaper::playlist_names(&app)?.contains(name) { return Err("This playlist no longer exists. Reopen settings to refresh the list.".into()); }
    }
    let mut current = state.shortcut.lock().map_err(|e| e.to_string())?;
    let old = validate_shortcut(&current)?;
    let changed = new != old;
    // Register first: a conflicting shortcut cannot remove the existing working binding.
    if changed { crate::register_shortcut(&app, &shortcut).map_err(|e| format!("Shortcut unavailable: {e}. Your previous shortcut is still active."))?; }
    if let Err(error) = persist(&app, &playlist, shortcut.trim()) {
        if changed { let _ = app.global_shortcut().unregister(new); }
        return Err(error);
    }
    if changed {
        if let Err(error) = app.global_shortcut().unregister(old) {
            let _ = app.global_shortcut().unregister(new);
            let _ = persist(&app, &crate::wallpaper::selected_playlist(&app), &current);
            return Err(format!("Could not replace the shortcut: {error}"));
        }
    }
    crate::wallpaper::set_playlist(&app, playlist)?;
    *current = shortcut.trim().to_owned();
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
}
