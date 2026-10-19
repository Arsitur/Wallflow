mod wallpaper;
mod covers;
mod focus;
mod engine_config;
mod engine_query;
mod lifecycle;
mod settings;
use wallpaper::Gallery;
use std::sync::{Arc, atomic::Ordering};
use tauri::{Emitter, Manager, PhysicalPosition, PhysicalSize};
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{TrayIconBuilder, TrayIconEvent, MouseButton, MouseButtonState};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

#[tauri::command]
fn show_gallery(app: tauri::AppHandle) -> Result<(), String> {
    let window = app.get_webview_window("main").ok_or("Gallery window is unavailable")?;
    let history = app.state::<Arc<focus::FocusHistory>>();
    if !window.is_visible().map_err(|e| e.to_string())? || app.state::<lifecycle::Lifecycle>().is_closing() {
        history.visible.store(true, Ordering::Release);
        history.remember();
    }
    let epoch = app.state::<lifecycle::Lifecycle>().open();
    // Hide the native window before any size, monitor or DPI change can be displayed.
    window.hide().map_err(|e| e.to_string())?;
    let point = history.monitor_point().or_else(|| window.cursor_position().ok().map(|position| (position.x, position.y)));
    if let Some((x, y)) = point {
        if let Some(monitor) = window.monitor_from_point(x, y).map_err(|e| e.to_string())? {
            fit_monitor(&window, &monitor)?;
        }
    }
    window.emit("gallery-opening", epoch).map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
fn present_gallery(app: tauri::AppHandle, epoch: u64) -> Result<bool, String> {
    let window = app.get_webview_window("main").ok_or("Gallery window is unavailable")?;
    if !app.state::<lifecycle::Lifecycle>().present(epoch) { return Ok(false); }
    // Windows may adjust physical size during WM_DPICHANGED after a monitor move.
    // Reconcile once more while hidden, never from a delayed visible resize callback.
    fit_gallery(window.clone())?;
    // The frontend acknowledges its new layout while the native window is still hidden.
    window.show().and_then(|_| window.set_focus()).map_err(|e| e.to_string())?;
    Ok(true)
}

#[tauri::command]
fn gallery_ready(app: tauri::AppHandle) -> Result<(), String> { show_gallery(app) }

#[tauri::command]
fn hide_gallery(app: tauri::AppHandle) -> Result<(), String> {
    hide_gallery_inner(app, true)
}

fn hide_gallery_inner(app: tauri::AppHandle, restore_focus: bool) -> Result<(), String> {
    let window = app.get_webview_window("main").ok_or("Gallery window is unavailable")?;
    if !window.is_visible().map_err(|e| e.to_string())? && !app.state::<lifecycle::Lifecycle>().is_preparing() { return Ok(()); }
    let Some(epoch) = app.state::<lifecycle::Lifecycle>().close(restore_focus) else { return Ok(()); };
    window.emit("gallery-closing", epoch).map_err(|e| e.to_string())?;
    // Fallback if the frontend is being reloaded or cannot acknowledge its exit animation.
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(260));
        let handle = app.clone();
        let _ = app.run_on_main_thread(move || { let _ = finish_hide(handle, epoch); });
    });
    Ok(())
}

#[tauri::command]
fn finish_hide(app: tauri::AppHandle, epoch: u64) -> Result<(), String> {
    let Some(restore_focus) = app.state::<lifecycle::Lifecycle>().finish(epoch) else { return Ok(()); };
    let window = app.get_webview_window("main").ok_or("Gallery window is unavailable")?;
    let history = app.state::<Arc<focus::FocusHistory>>();
    let restore = restore_focus && focus::owns_foreground();
    window.hide().map_err(|e| e.to_string())?;
    if restore { history.restore(); }
    history.visible.store(false, Ordering::Release);
    Ok(())
}

fn toggle_gallery(app: tauri::AppHandle) -> Result<(), String> {
    let window = app.get_webview_window("main").ok_or("Gallery window is unavailable")?;
    if (window.is_visible().map_err(|e| e.to_string())? || app.state::<lifecycle::Lifecycle>().is_preparing()) && !app.state::<lifecycle::Lifecycle>().is_closing() { hide_gallery(app) } else { show_gallery(app) }
}

pub(crate) fn register_shortcut(app: &tauri::AppHandle, shortcut: &str) -> Result<(), String> {
    app.global_shortcut().on_shortcut(shortcut, |app, _, event| {
        if event.state == ShortcutState::Pressed {
            let handle = app.clone();
            let _ = app.run_on_main_thread(move || {
                if let Err(error) = toggle_gallery(handle) { eprintln!("{error}"); }
            });
        }
    }).map_err(|e| e.to_string())
}

#[tauri::command]
fn fit_gallery(window: tauri::WebviewWindow) -> Result<(), String> {
    if let Some(monitor) = window.current_monitor().map_err(|e| e.to_string())? {
        fit_monitor(&window, &monitor)?;
    }
    Ok(())
}

fn fit_monitor(window: &tauri::WebviewWindow, monitor: &tauri::Monitor) -> Result<(), String> {
        let size = monitor.size();
        let origin = monitor.position();
        let height = (f64::from(size.height) * 0.66).round() as u32;
        let desired = PhysicalSize::new(size.width, height);
        if window.inner_size().map_err(|e| e.to_string())? != desired {
            window.set_size(desired).map_err(|e| e.to_string())?;
        }
        let position = PhysicalPosition::new(origin.x, origin.y + ((size.height - height) / 2) as i32);
        if window.outer_position().map_err(|e| e.to_string())? != position {
            window.set_position(position).map_err(|e| e.to_string())?;
        }
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let history = Arc::new(focus::FocusHistory::default());
    history.remember();
    focus::FocusHistory::start_tracking(&history);
    tauri::Builder::default()
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .manage(Gallery::default())
        .manage(history)
        .manage(lifecycle::Lifecycle::default())
        .manage(settings::SettingsState::default())
        .setup(|app| {
            let _ = wallpaper::initialize(app.handle()).map_err(|error| eprintln!("{error}"));
            settings::initialize(app.handle());
            let show = MenuItem::with_id(app, "show", "Open Wallflow", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit Wallflow", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show, &quit])?;
            let icon = app.default_window_icon().ok_or("Wallflow icon is missing")?.clone();
            TrayIconBuilder::with_id("wallflow")
                .icon(icon)
                .tooltip("Wallflow — click to open")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
                        if let Err(error) = show_gallery(tray.app_handle().clone()) { eprintln!("{error}"); }
                    }
                })
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "show" => { if let Err(error) = show_gallery(app.clone()) { eprintln!("{error}"); } }
                    "quit" => { let _ = hide_gallery(app.clone()); app.exit(0); }
                    _ => {}
                })
                .build(app)?;
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::Focused(false) = event {
                // The newly focused application keeps focus; hide_gallery never steals it back.
                if window.is_visible().unwrap_or(false) && !window.state::<lifecycle::Lifecycle>().is_preparing() {
                    if let Err(error) = hide_gallery_inner(window.app_handle().clone(), false) { eprintln!("{error}"); }
                }
            }
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                if let Err(error) = hide_gallery(window.app_handle().clone()) { eprintln!("{error}"); }
            }
        })
        .invoke_handler(tauri::generate_handler![
            show_gallery,
            gallery_ready,
            present_gallery,
            finish_hide,
            hide_gallery,
            fit_gallery,
            wallpaper::list_wallpapers,
            wallpaper::gallery_snapshot,
            wallpaper::wallpaper_preview,
            wallpaper::apply_wallpaper,
            wallpaper::resume_wallpapers
            ,settings::get_settings
            ,settings::save_settings
        ])
        .run(tauri::generate_context!())
        .expect("Wallflow failed to start");
}
