use std::sync::{Arc, Mutex, atomic::{AtomicBool, Ordering}};
use std::time::Duration;

#[derive(Clone, Copy, Debug)]
struct Foreground { handle: usize, process_id: u32 }

#[derive(Default)]
pub struct FocusHistory {
    previous: Mutex<Option<Foreground>>,
    pub visible: AtomicBool,
}

impl FocusHistory {
    pub fn remember(&self) {
        if let Some(window) = foreground() {
            if let Ok(mut previous) = self.previous.lock() { *previous = Some(window); }
        }
    }
    pub fn restore(&self) {
        let previous = self.previous.lock().ok().and_then(|previous| *previous);
        if let Some(previous) = previous { restore(previous); }
    }
    pub fn monitor_point(&self) -> Option<(f64, f64)> {
        self.previous.lock().ok().and_then(|previous| *previous).and_then(monitor_point)
    }
    pub fn start_tracking(history: &Arc<Self>) {
        // Clicking the tray focuses Explorer; retain the app that preceded it.
        let weak = Arc::downgrade(history);
        std::thread::spawn(move || loop {
            let Some(history) = weak.upgrade() else { break; };
            if !history.visible.load(Ordering::Acquire) { history.remember(); }
            drop(history);
            std::thread::sleep(Duration::from_millis(150));
        });
    }
}

fn should_remember(process_id: u32, class: &str) -> bool {
    process_id != 0 && process_id != std::process::id()
        && class != "Shell_TrayWnd" && class != "Shell_SecondaryTrayWnd"
        && class != "NotifyIconOverflowWindow" && class != "#32768"
}

#[cfg(windows)]
fn foreground() -> Option<Foreground> {
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId, GetClassNameW};
    unsafe {
        let window = GetForegroundWindow();
        if window.is_null() { return None; }
        let mut process_id = 0;
        GetWindowThreadProcessId(window, &mut process_id);
        let mut class = [0u16; 256];
        let length = GetClassNameW(window, class.as_mut_ptr(), class.len() as i32);
        let class = String::from_utf16_lossy(&class[..length.max(0) as usize]);
        should_remember(process_id, &class).then_some(Foreground { handle: window as usize, process_id })
    }
}

#[cfg(windows)]
pub fn owns_foreground() -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};
    unsafe {
        let window = GetForegroundWindow();
        let mut process_id = 0;
        if !window.is_null() { GetWindowThreadProcessId(window, &mut process_id); }
        window.is_null() || process_id == std::process::id()
    }
}

#[cfg(windows)]
fn restore(previous: Foreground) {
    use windows_sys::Win32::{Foundation::HWND, UI::WindowsAndMessaging::{IsWindow, IsWindowVisible, GetWindowThreadProcessId, SetForegroundWindow}};
    unsafe {
        let window = previous.handle as HWND;
        let mut process_id = 0;
        if IsWindow(window) == 0 || IsWindowVisible(window) == 0 { return; }
        GetWindowThreadProcessId(window, &mut process_id);
        // Ignore destroyed/reused HWNDs instead of focusing an unrelated process.
        if process_id == previous.process_id && SetForegroundWindow(window) == 0 {
            crate::diagnostics::record("warn", "focus", "Windows declined restoring the previous foreground window");
        }
    }
}

#[cfg(windows)]
fn monitor_point(previous: Foreground) -> Option<(f64, f64)> {
    use windows_sys::Win32::{Foundation::HWND, Graphics::Gdi::{GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST}, UI::WindowsAndMessaging::{IsWindow, GetWindowThreadProcessId}};
    unsafe {
        let window = previous.handle as HWND;
        let mut process_id = 0;
        if IsWindow(window) == 0 { return None; }
        GetWindowThreadProcessId(window, &mut process_id);
        if process_id != previous.process_id { return None; }
        // The desktop spans displays, so its HWND otherwise always selects the primary display.
        let mut class = [0u16; 256];
        let length = windows_sys::Win32::UI::WindowsAndMessaging::GetClassNameW(window, class.as_mut_ptr(), 256);
        if matches!(String::from_utf16_lossy(&class[..length.max(0) as usize]).as_str(), "Progman" | "WorkerW") {
            let mut point = std::mem::zeroed();
            if windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos(&mut point) != 0 { return Some((point.x as f64, point.y as f64)); }
        }
        let monitor = MonitorFromWindow(window, MONITOR_DEFAULTTONEAREST);
        let mut info: MONITORINFO = std::mem::zeroed();
        info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        if GetMonitorInfoW(monitor, &mut info) == 0 { return None; }
        let rect = info.rcMonitor;
        Some(((f64::from(rect.left) + f64::from(rect.right)) / 2.0, (f64::from(rect.top) + f64::from(rect.bottom)) / 2.0))
    }
}

#[cfg(windows)]
pub fn exclude_from_switcher(window: &tauri::WebviewWindow) -> Result<(), String> {
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetWindowLongPtrW, SetWindowLongPtrW, SetWindowPos, GWL_EXSTYLE, WS_EX_APPWINDOW, WS_EX_TOOLWINDOW, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER};
    let hwnd = window.hwnd().map_err(|e| e.to_string())?.0 as _;
    unsafe {
        let style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, (style & !(WS_EX_APPWINDOW as isize)) | WS_EX_TOOLWINDOW as isize);
        if SetWindowPos(hwnd, std::ptr::null_mut(), 0, 0, 0, 0, SWP_FRAMECHANGED | SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER) == 0 { return Err(std::io::Error::last_os_error().to_string()); }
    }
    Ok(())
}
#[cfg(not(windows))]
pub fn exclude_from_switcher(_: &tauri::WebviewWindow) -> Result<(), String> { Ok(()) }

#[cfg(not(windows))]
fn foreground() -> Option<Foreground> { None }
#[cfg(not(windows))]
pub fn owns_foreground() -> bool { true }
#[cfg(not(windows))]
fn restore(_: Foreground) {}
#[cfg(not(windows))]
fn monitor_point(_: Foreground) -> Option<(f64, f64)> { None }

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn taskbar_menus_and_our_window_do_not_replace_the_previous_app() {
        assert!(!should_remember(std::process::id(), "Wallflow"));
        for class in ["Shell_TrayWnd", "Shell_SecondaryTrayWnd", "NotifyIconOverflowWindow", "#32768"] {
            assert!(!should_remember(u32::MAX, class));
        }
        assert!(should_remember(u32::MAX, "Chrome_WidgetWin_1"));
        assert!(should_remember(u32::MAX, "CabinetWClass"));
    }
}
