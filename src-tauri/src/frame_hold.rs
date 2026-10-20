//! Keep the desktop visually static while WE replaces a paused renderer.
//! WE's openWallpaper command clears manual pause itself. Immutable, opaque
//! bitmap windows cover its desktop layer until Pause has settled again.
//! Captured pixels stay in GDI memory only; never saved or sent anywhere.
use std::path::Path;

pub use native::Hold;

#[cfg(not(windows))]
mod native {
    use super::*;
    pub struct Hold;
    impl Hold {
        pub fn begin(_: &Path) -> Result<Self, String> { Err("Static desktop protection requires Windows".into()) }
        pub fn settle_pause(&self) {}
    }
}

#[cfg(windows)]
mod native {
    use super::*;
    use std::{ptr::{null, null_mut}, sync::{Arc, OnceLock, atomic::{AtomicBool, Ordering}, mpsc}, thread, time::Duration};
    use windows_sys::Win32::{Foundation::{HWND, RECT, POINT, SIZE}, Graphics::{Dwm::DwmFlush, Gdi::*},
        System::LibraryLoader::GetModuleHandleW,
        UI::{HiDpi::{SetThreadDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2}, WindowsAndMessaging::*}};

    struct Target { root: HWND, rect: RECT }
    const CLASS_NAME: &str = "WallflowStaticFrame\0";
    unsafe extern "system" fn window_proc(window: HWND, message: u32, wparam: usize, lparam: isize) -> isize {
        match message {
            WM_NCHITTEST => HTTRANSPARENT as isize,
            WM_MOUSEACTIVATE => MA_NOACTIVATE as isize,
            WM_ERASEBKGND => 1,
            _ => DefWindowProcW(window, message, wparam, lparam),
        }
    }
    fn register_class() -> Result<(), String> {
        static CLASS: OnceLock<Result<u16, String>> = OnceLock::new();
        CLASS.get_or_init(|| unsafe {
            let name: Vec<u16> = CLASS_NAME.encode_utf16().collect();
            let class = WNDCLASSW { lpfnWndProc: Some(window_proc), hInstance: GetModuleHandleW(null()), lpszClassName: name.as_ptr(), ..std::mem::zeroed() };
            let atom = RegisterClassW(&class);
            if atom == 0 { Err(format!("Cannot register the static frame window: {}", std::io::Error::last_os_error())) } else { Ok(atom) }
        }).as_ref().map(|_| ()).map_err(Clone::clone)
    }
    struct Search { pid: u32, targets: Vec<Target> }
    unsafe extern "system" fn inspect(window: HWND, data: isize) -> i32 {
        let search = &mut *(data as *mut Search);
        let mut pid = 0; GetWindowThreadProcessId(window, &mut pid);
        if pid != search.pid || IsWindowVisible(window) == 0 { return 1; }
        let mut class = [0u16; 128];
        let length = GetClassNameW(window, class.as_mut_ptr(), class.len() as i32).max(0) as usize;
        if !String::from_utf16_lossy(&class[..length]).starts_with("WPE") { return 1; }
        let mut rect: RECT = std::mem::zeroed();
        if GetWindowRect(window, &mut rect) == 0 || rect.right - rect.left < 32 || rect.bottom - rect.top < 32 { return 1; }
        let root = GetAncestor(window, GA_ROOT);
        if root.is_null() { return 1; }
        if !search.targets.iter().any(|t| t.root == root && t.rect.left == rect.left && t.rect.top == rect.top && t.rect.right == rect.right && t.rect.bottom == rect.bottom) {
            search.targets.push(Target { root, rect });
        }
        1
    }
    unsafe extern "system" fn top(window: HWND, data: isize) -> i32 {
        inspect(window, data); EnumChildWindows(window, Some(inspect), data); 1
    }

    struct Cover { window: HWND, bitmap: HBITMAP, root: HWND, rect: RECT }
    impl Drop for Cover {
        fn drop(&mut self) { unsafe { DestroyWindow(self.window); DeleteObject(self.bitmap); } }
    }
    impl Cover {
        unsafe fn capture(target: Target) -> Result<Self, String> {
            register_class()?;
            let width = target.rect.right - target.rect.left;
            let height = target.rect.bottom - target.rect.top;
            let screen = GetDC(null_mut());
            if screen.is_null() { return Err("Cannot capture the paused desktop".into()); }
            let memory = CreateCompatibleDC(screen);
            let bitmap = CreateCompatibleBitmap(screen, width, height);
            if memory.is_null() || bitmap.is_null() {
                if !memory.is_null() { DeleteDC(memory); }
                if !bitmap.is_null() { DeleteObject(bitmap); }
                ReleaseDC(null_mut(), screen);
                return Err("Cannot allocate a static desktop frame".into());
            }
            let previous = SelectObject(memory, bitmap);
            let copied = BitBlt(memory, 0, 0, width, height, screen, target.rect.left, target.rect.top, SRCCOPY | CAPTUREBLT);
            SelectObject(memory, previous); DeleteDC(memory); ReleaseDC(null_mut(), screen);
            if copied == 0 { DeleteObject(bitmap); return Err("Cannot capture the paused desktop frame".into()); }
            let class: Vec<u16> = CLASS_NAME.encode_utf16().collect();
            let title: Vec<u16> = "Wallflow static wallpaper frame\0".encode_utf16().collect();
            let window = CreateWindowExW(WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_LAYERED | WS_EX_TRANSPARENT,
                class.as_ptr(), title.as_ptr(), WS_POPUP, target.rect.left, target.rect.top, width, height,
                null_mut(), null_mut(), GetModuleHandleW(null()), null());
            if window.is_null() { DeleteObject(bitmap); return Err("Cannot create the static wallpaper cover".into()); }
            let cover = Self { window, bitmap, root: target.root, rect: target.rect };
            cover.present()?;
            Ok(cover)
        }
        unsafe fn present(&self) -> Result<(), String> {
            let screen = GetDC(null_mut());
            let memory = CreateCompatibleDC(screen);
            if screen.is_null() || memory.is_null() {
                if !memory.is_null() { DeleteDC(memory); }
                if !screen.is_null() { ReleaseDC(null_mut(), screen); }
                return Err("Cannot present the static wallpaper frame".into());
            }
            let previous = SelectObject(memory, self.bitmap);
            let destination = POINT { x: self.rect.left, y: self.rect.top };
            let origin = POINT { x: 0, y: 0 };
            let size = SIZE { cx: self.rect.right - self.rect.left, cy: self.rect.bottom - self.rect.top };
            // Supply the compositor an opaque immutable bitmap directly. This
            // does not depend on deferred WM_PAINT/redirection of a control.
            let result = UpdateLayeredWindow(self.window, screen, &destination, &size, memory, &origin, 0, null(), ULW_OPAQUE);
            SelectObject(memory, previous); DeleteDC(memory); ReleaseDC(null_mut(), screen);
            if result == 0 { return Err(format!("Cannot present the static wallpaper frame: {}", std::io::Error::last_os_error())); }
            Ok(())
        }
        unsafe fn show(&self) -> Result<(), String> {
            // Immediately above the desktop host, below foreground apps. The
            // cover is a sibling of the host, so newly created WE children
            // cannot paint above it. Never activate or move existing windows.
            let previous = GetWindow(self.root, GW_HWNDPREV);
            if SetWindowPos(self.window, if previous.is_null() { HWND_TOP } else { previous },
                self.rect.left, self.rect.top, self.rect.right - self.rect.left, self.rect.bottom - self.rect.top,
                SWP_NOACTIVATE | SWP_NOOWNERZORDER | SWP_SHOWWINDOW) == 0 { return Err("Cannot position the static wallpaper cover".into()); }
            if !self.root.is_null() {
                let mut above = GetWindow(self.root, GW_HWNDPREV);
                let mut covers_host = false;
                for _ in 0..16384 {
                    if above.is_null() { break; }
                    if above == self.window { covers_host = true; break; }
                    above = GetWindow(above, GW_HWNDPREV);
                }
                if !covers_host { return Err("Windows did not place the static frame above the wallpaper; cancelled before loading".into()); }
            }
            Ok(())
        }
    }

    pub struct Hold { worker: Option<thread::JoinHandle<()>>, cancel: Arc<AtomicBool> }
    impl Hold {
        pub fn begin(executable: &Path) -> Result<Self, String> {
            let pid = crate::playback_state::owner_pid(executable)?;
            let (sender, receiver) = mpsc::sync_channel(1);
            let cancel = Arc::new(AtomicBool::new(false));
            let cancelled = cancel.clone();
            let worker = thread::spawn(move || unsafe {
                SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
                let result = (|| {
                    let mut search = Search { pid, targets: Vec::new() };
                    EnumWindows(Some(top), &mut search as *mut Search as isize);
                    if search.targets.is_empty() { return Err("Cannot locate Wallpaper Engine's desktop rendering windows".to_owned()); }
                    let pixels: u64 = search.targets.iter().map(|t| (t.rect.right - t.rect.left) as u64 * (t.rect.bottom - t.rect.top) as u64).sum();
                    if pixels > 64 * 1024 * 1024 { return Err("Desktop is too large to protect a paused transition".into()); }
                    // Capture every region before showing any cover.
                    let covers = search.targets.into_iter().map(|t| Cover::capture(t)).collect::<Result<Vec<_>, _>>()?;
                    for cover in &covers { cover.show()?; }
                    GdiFlush();
                    if DwmFlush() < 0 { return Err("Cannot synchronize the static wallpaper cover with Windows".into()); }
                    Ok(covers)
                })();
                match result {
                    Err(error) => { let _ = sender.send(Err(error)); }
                    Ok(covers) => {
                        if cancelled.load(Ordering::Acquire) || sender.send(Ok(covers.len())).is_err() { return; }
                        let mut message: MSG = std::mem::zeroed();
                        // Covers own their UI thread and are removed on every
                        // normal/error return, including a readiness timeout.
                        while !cancelled.load(Ordering::Acquire) {
                            while PeekMessageW(&mut message, null_mut(), 0, 0, PM_REMOVE) != 0 {
                                TranslateMessage(&message); DispatchMessageW(&message);
                            }
                            thread::sleep(Duration::from_millis(5));
                        }
                        drop(covers);
                    }
                }
            });
            match receiver.recv_timeout(Duration::from_secs(3)) {
                Ok(Ok(count)) => {
                    crate::diagnostics::record("debug", "playback", format!("Holding {count} desktop regions as static frames before loading the paused replacement"));
                    Ok(Self { worker: Some(worker), cancel })
                }
                Ok(Err(error)) => { let _ = worker.join(); Err(error) }
                Err(_) => { cancel.store(true, Ordering::Release); Err("Timed out preparing a static wallpaper frame; no wallpaper command was sent".into()) }
            }
        }
        pub fn settle_pause(&self) {
            // Pause acknowledges the global control before the media renderer
            // completes its asynchronous pause. Keep all moving frames covered.
            thread::sleep(Duration::from_millis(150));
            unsafe { DwmFlush(); DwmFlush(); }
        }
    }
    impl Drop for Hold {
        fn drop(&mut self) {
            self.cancel.store(true, Ordering::Release);
            if let Some(worker) = self.worker.take() { let _ = worker.join(); }
            crate::diagnostics::record("debug", "playback", "Removed static desktop frames after pause settled");
        }
    }

    #[cfg(test)]
    #[test]
    #[ignore = "Requires unobstructed desktop: briefly paints a 64px test square; never saves pixels to disk"]
    fn static_cover_hides_every_underlying_update_and_never_activates() {
        unsafe fn fill(cover: &Cover, color: u32) {
            let screen = GetDC(null_mut());
            let memory = CreateCompatibleDC(screen);
            let previous = SelectObject(memory, cover.bitmap);
            let brush = CreateSolidBrush(color);
            let rect = RECT { left: 0, top: 0, right: 64, bottom: 64 };
            FillRect(memory, &rect, brush);
            assert_eq!(GetPixel(memory, 32, 32), color, "Controlled source pixels must be updated");
            DeleteObject(brush); SelectObject(memory, previous); DeleteDC(memory); ReleaseDC(null_mut(), screen);
            cover.present().unwrap();
            GdiFlush(); DwmFlush();
        }
        unsafe fn pixel() -> u32 {
            let screen = GetDC(null_mut());
            let memory = CreateCompatibleDC(screen);
            let bitmap = CreateCompatibleBitmap(screen, 1, 1);
            let previous = SelectObject(memory, bitmap);
            // GetPixel(screen) omits layered windows on some drivers. Capture
            // the composed desktop, as production capture does, including them.
            assert_ne!(BitBlt(memory, 0, 0, 1, 1, screen, 48, 48, SRCCOPY | CAPTUREBLT), 0);
            let color = GetPixel(memory, 0, 0);
            SelectObject(memory, previous); DeleteObject(bitmap); DeleteDC(memory);
            ReleaseDC(null_mut(), screen); color
        }
        unsafe {
            SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
            let foreground = GetForegroundWindow();
            let rect = RECT { left: 16, top: 16, right: 80, bottom: 80 };
            let source = Cover::capture(Target { root: null_mut(), rect }).unwrap();
            fill(&source, 0x0000ff);
            source.show().unwrap();
            // Only the isolated 64px test square is topmost; production covers
            // deliberately stay at the desktop layer, below foreground apps.
            assert_ne!(SetWindowPos(source.window, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE), 0);
            GdiFlush(); DwmFlush();
            let mut message: MSG = std::mem::zeroed();
            for _ in 0..20 {
                while PeekMessageW(&mut message, null_mut(), 0, 0, PM_REMOVE) != 0 { TranslateMessage(&message); DispatchMessageW(&message); }
                thread::sleep(Duration::from_millis(5));
            }
            assert_eq!(pixel(), 0x0000ff, "The controlled source must initially be visible");
            let frozen = Cover::capture(Target { root: source.window, rect }).unwrap();
            frozen.show().unwrap();
            SetWindowPos(frozen.window, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
            GdiFlush(); DwmFlush();
            for i in 0..20 {
                fill(&source, if i % 2 == 0 { 0xff0000 } else { 0x00ff00 });
                assert_eq!(pixel(), 0x0000ff, "No moving background frame may leak through the static cover");
                assert_eq!(GetForegroundWindow(), foreground, "A static wallpaper cover must never activate");
                thread::sleep(Duration::from_millis(10));
            }
            fill(&source, 0xff0000);
            drop(frozen); GdiFlush(); DwmFlush();
            assert_eq!(pixel(), 0xff0000, "Removing the cover must reveal only the final background frame");
        }
    }
}
