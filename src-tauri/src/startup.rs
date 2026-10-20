//! Per-user, opt-in sign-in launch and one silent instance per Windows session.
use std::path::Path;

fn command(executable: &Path) -> String { format!("\"{}\" --background", executable.display()) }

#[cfg(windows)]
mod platform {
    use winreg::{enums::{HKEY_CURRENT_USER, KEY_READ, KEY_SET_VALUE}, RegKey};
    const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    const VALUE: &str = "Wallflow";
    pub fn read() -> Result<Option<String>, String> {
        let root = RegKey::predef(HKEY_CURRENT_USER);
        let key = match root.open_subkey_with_flags(RUN, KEY_READ) {
            Ok(key) => key,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.to_string()),
        };
        match key.get_value(VALUE) {
            Ok(value) => Ok(Some(value)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.to_string()),
        }
    }
    pub fn write(value: Option<&str>) -> Result<(), String> {
        let root = RegKey::predef(HKEY_CURRENT_USER);
        if let Some(value) = value {
            root.create_subkey(RUN).map_err(|e| e.to_string())?.0.set_value(VALUE, &value).map_err(|e| e.to_string())
        } else {
            let key = match root.open_subkey_with_flags(RUN, KEY_SET_VALUE) {
                Ok(key) => key,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
                Err(error) => return Err(error.to_string()),
            };
            match key.delete_value(VALUE) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(error.to_string()),
            }
        }
    }

    pub struct Instance(windows_sys::Win32::Foundation::HANDLE);
    impl Instance {
        pub fn acquire() -> Result<Option<Self>, String> { Self::named(r"Local\app.wallflow.desktop") }
        fn named(name: &str) -> Result<Option<Self>, String> {
            use windows_sys::Win32::{Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS}, System::Threading::CreateMutexW};
            let name: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
            unsafe {
                let handle = CreateMutexW(std::ptr::null(), 0, name.as_ptr());
                if handle.is_null() { return Err(std::io::Error::last_os_error().to_string()); }
                if GetLastError() == ERROR_ALREADY_EXISTS { CloseHandle(handle); return Ok(None); }
                Ok(Some(Self(handle)))
            }
        }
    }
    impl Drop for Instance { fn drop(&mut self) { unsafe { windows_sys::Win32::Foundation::CloseHandle(self.0); } } }
    #[cfg(test)]
    #[test]
    fn a_second_launch_cannot_start_another_instance_and_exit_releases_the_lock() {
        let name = format!(r"Local\Wallflow-test-{}", std::process::id());
        let first = Instance::named(&name).unwrap().unwrap();
        assert!(Instance::named(&name).unwrap().is_none());
        drop(first);
        assert!(Instance::named(&name).unwrap().is_some());
    }
}
#[cfg(not(windows))]
mod platform {
    pub fn read() -> Result<Option<String>, String> { Ok(None) }
    pub fn write(_: Option<&str>) -> Result<(), String> { Err("Start with Windows is only available on Windows".into()) }
    pub struct Instance;
    impl Instance { pub fn acquire() -> Result<Option<Self>, String> { Ok(Some(Self)) } }
}
pub use platform::Instance;

pub fn enabled() -> Result<bool, String> {
    let current = std::env::current_exe().map_err(|e| e.to_string())?;
    Ok(platform::read()?.is_some_and(|entry| entry.eq_ignore_ascii_case(&command(&current))))
}

// Registry and settings form one save: roll back the sign-in entry if saving
// the shortcut, playlist, properties or settings file fails later.
pub struct Change { previous: Option<String>, changed: bool, committed: bool }
impl Change {
    pub fn apply(enabled: bool) -> Result<Self, String> {
        let previous = platform::read()?;
        let next = if enabled { Some(command(&std::env::current_exe().map_err(|e| e.to_string())?)) } else { None };
        let changed = previous != next;
        if changed { platform::write(next.as_deref()).map_err(|e| format!("Could not update Start with Windows: {e}"))?; }
        Ok(Self { previous, changed, committed: false })
    }
    pub fn commit(&mut self) { self.committed = true; }
}
impl Drop for Change {
    fn drop(&mut self) {
        if self.changed && !self.committed {
            if let Err(error) = platform::write(self.previous.as_deref()) { crate::diagnostics::record("error", "startup", format!("Could not restore the previous sign-in entry: {error}")); }
        }
    }
}
#[cfg(test)]
#[test]
fn sign_in_command_quotes_paths_with_spaces_and_requests_background_launch() {
    assert_eq!(command(Path::new("C:/Program Files/Wallflow/wallflow.exe")), "\"C:/Program Files/Wallflow/wallflow.exe\" --background");
}
