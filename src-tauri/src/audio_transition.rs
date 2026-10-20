//! Gate only Wallpaper Engine's Windows mixer sessions while a new renderer
//! initializes. Its Ctrl+PgDn mute is internal and has no documented CLI query.
//! Snapshot the mixer independently; never toggle or guess the internal state.
use std::{collections::HashMap, path::Path};

trait Volume {
    fn muted(&self) -> Result<bool, String>;
    fn set_muted(&self, muted: bool) -> Result<(), String>;
}

struct Sessions<V: Volume> {
    active: bool,
    originals: HashMap<String, (V, bool)>,
    // New streams may inherit the mute we just set on the same application.
    baselines: HashMap<String, Option<bool>>,
}
impl<V: Volume> Default for Sessions<V> {
    fn default() -> Self { Self { active: true, originals: HashMap::new(), baselines: HashMap::new() } }
}
impl<V: Volume> Sessions<V> {
    fn capture(&mut self, instance: String, owner: String, volume: V, new_session: bool) -> Result<(), String> {
        if !self.active || self.originals.contains_key(&instance) { return Ok(()); }
        let muted = volume.muted()?;
        let original = if new_session {
            self.baselines.get(&owner).copied().flatten().unwrap_or(muted)
        } else {
            self.baselines.entry(owner).and_modify(|baseline| {
                if *baseline != Some(muted) { *baseline = None; }
            }).or_insert(Some(muted));
            muted
        };
        volume.set_muted(true)?;
        self.originals.insert(instance, (volume, original));
        Ok(())
    }
    fn restore(&mut self) {
        self.active = false;
        for (_, (volume, original)) in self.originals.drain() {
            if let Err(error) = volume.set_muted(original) {
                crate::diagnostics::record("warn", "audio", format!("Could not restore an audio session (it may have closed): {error}"));
            }
        }
    }
}
impl<V: Volume> Drop for Sessions<V> { fn drop(&mut self) { self.restore(); } }

pub fn protect<T>(executable: &Path, enabled: bool, operation: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    if !enabled { return operation(); }
    let mut guard = native::Guard::begin(executable).map_err(|e| format!("Could not protect Wallpaper Engine audio before switching: {e}"))?;
    let result = operation();
    // openWallpaper acknowledges the command before Media Foundation has fully
    // initialized. Cover that startup window as well as applyProperties.
    guard.settle();
    drop(guard); // Restore even when openWallpaper/applyProperties failed.
    result
}

#[cfg(not(windows))]
mod native {
    use super::*;
    pub struct Guard;
    impl Guard {
        pub fn begin(_: &Path) -> Result<Self, String> { Ok(Self) }
        pub fn settle(&mut self) {}
    }
}

#[cfg(windows)]
mod native {
    use super::*;
    use std::{path::PathBuf, sync::{Arc, Mutex}, time::{Duration, Instant}};
    use windows::{core::{implement, Interface, Ref, PWSTR, GUID}, Win32::{
        Foundation::CloseHandle,
        Media::Audio::{eRender, DEVICE_STATE_ACTIVE, MMDeviceEnumerator, IMMDeviceEnumerator,
            IAudioSessionControl, IAudioSessionControl2, IAudioSessionManager2,
            IAudioSessionNotification, IAudioSessionNotification_Impl, ISimpleAudioVolume},
        System::{Com::{CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_ALL, COINIT_MULTITHREADED},
            Threading::{OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION}},
    }};
    const CONTEXT: GUID = GUID::from_u128(0x4f30bb54_5ca7_4c5a_b8f1_e60cb4ec0ce0);

    impl Volume for ISimpleAudioVolume {
        fn muted(&self) -> Result<bool, String> { unsafe { self.GetMute().map(|value| value.as_bool()).map_err(|e| e.to_string()) } }
        fn set_muted(&self, muted: bool) -> Result<(), String> { unsafe { self.SetMute(muted, &CONTEXT).map_err(|e| e.to_string()) } }
    }

    struct State { sessions: Sessions<ISimpleAudioVolume>, last_session: Instant }
    #[implement(IAudioSessionNotification)]
    struct Notifications { state: Arc<Mutex<State>>, folder: PathBuf, endpoint: u32 }
    impl IAudioSessionNotification_Impl for Notifications_Impl {
        fn OnSessionCreated(&self, session: Ref<IAudioSessionControl>) -> windows::core::Result<()> {
            if let Some(session) = session.as_ref() {
                if let Err(error) = capture(&self.state, &self.folder, self.endpoint, session, true) {
                    crate::diagnostics::record("warn", "audio", format!("New Wallpaper Engine audio session: {error}"));
                }
            }
            Ok(())
        }
    }
    fn process_path(pid: u32) -> Option<PathBuf> {
        // A session name is user-controlled. Match the process executable inside
        // the installed engine folder, never another application or master volume.
        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
            let mut path = vec![0u16; 32768];
            let mut size = path.len() as u32;
            let result = QueryFullProcessImageNameW(handle, PROCESS_NAME_WIN32, PWSTR(path.as_mut_ptr()), &mut size);
            let _ = CloseHandle(handle);
            result.ok()?;
            Some(PathBuf::from(String::from_utf16_lossy(&path[..size as usize])))
        }
    }
    fn task_string(value: PWSTR) -> Result<String, String> {
        unsafe {
            let result = value.to_string().map_err(|e| e.to_string());
            CoTaskMemFree(Some(value.0.cast()));
            result
        }
    }
    fn capture(state: &Mutex<State>, folder: &Path, endpoint: u32, session: &IAudioSessionControl, new_session: bool) -> Result<(), String> {
        unsafe {
            let control: IAudioSessionControl2 = session.cast().map_err(|e| e.to_string())?;
            let pid = control.GetProcessId().map_err(|e| e.to_string())?;
            let Some(path) = process_path(pid) else { return Ok(()); };
            if !is_engine_process(folder, &path) { return Ok(()); }
            let instance = task_string(control.GetSessionInstanceIdentifier().map_err(|e| e.to_string())?)?;
            let owner = task_string(control.GetSessionIdentifier().map_err(|e| e.to_string())?)?;
            let volume = session.cast::<ISimpleAudioVolume>().map_err(|e| e.to_string())?;
            let mut state = state.lock().map_err(|e| e.to_string())?;
            let before = state.sessions.originals.len();
            state.sessions.capture(format!("{endpoint}:{instance}"), format!("{endpoint}:{owner}"), volume, new_session)?;
            if state.sessions.originals.len() != before { state.last_session = Instant::now(); }
            Ok(())
        }
    }

    struct Com;
    impl Drop for Com { fn drop(&mut self) { unsafe { CoUninitialize(); } } }
    pub struct Guard {
        registrations: Vec<(IAudioSessionManager2, IAudioSessionNotification)>,
        state: Arc<Mutex<State>>,
        // Declared last: every COM interface must be released before uninitializing.
        _com: Com,
    }
    impl Guard {
        pub fn begin(executable: &Path) -> Result<Self, String> {
            unsafe { CoInitializeEx(None, COINIT_MULTITHREADED).ok().map_err(|e| e.to_string())?; }
            let mut guard = Self { registrations: Vec::new(), state: Arc::new(Mutex::new(State { sessions: Sessions::default(), last_session: Instant::now() })), _com: Com };
            let folder = executable.parent().ok_or("Wallpaper Engine directory unavailable")?.to_owned();
            unsafe {
                let devices: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).map_err(|e| e.to_string())?;
                let endpoints = devices.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE).map_err(|e| e.to_string())?;
                for index in 0..endpoints.GetCount().map_err(|e| e.to_string())? {
                    let device = endpoints.Item(index).map_err(|e| e.to_string())?;
                    let manager: IAudioSessionManager2 = device.Activate(CLSCTX_ALL, None).map_err(|e| e.to_string())?;
                    let notifications: IAudioSessionNotification = Notifications { state: guard.state.clone(), folder: folder.clone(), endpoint: index }.into();
                    manager.RegisterSessionNotification(&notifications).map_err(|e| e.to_string())?;
                    guard.registrations.push((manager.clone(), notifications));
                    let sessions = manager.GetSessionEnumerator().map_err(|e| e.to_string())?;
                    // GetCount is required to enable session-created notifications.
                    for i in 0..sessions.GetCount().map_err(|e| e.to_string())? {
                        capture(&guard.state, &folder, index, &sessions.GetSession(i).map_err(|e| e.to_string())?, false)?;
                    }
                }
            }
            let count = guard.state.lock().map_err(|e| e.to_string())?.sessions.originals.len();
            crate::diagnostics::record("debug", "audio", format!("Audio guard armed before switch: {count} Wallpaper Engine sessions; watching for new streams"));
            Ok(guard)
        }
        pub fn settle(&mut self) {
            let start = Instant::now();
            loop {
                let quiet = self.state.lock().map(|state| state.last_session.elapsed()).unwrap_or_default();
                if (start.elapsed() >= Duration::from_millis(500) && quiet >= Duration::from_millis(150)) || start.elapsed() >= Duration::from_millis(1500) { break; }
                std::thread::sleep(Duration::from_millis(25));
            }
        }
    }
    impl Drop for Guard {
        fn drop(&mut self) {
            // Restore outside notification callbacks, including on an early error.
            if let Ok(mut state) = self.state.lock() { state.sessions.restore(); }
            for (manager, notifications) in self.registrations.drain(..) {
                if let Err(error) = unsafe { manager.UnregisterSessionNotification(&notifications) } {
                    crate::diagnostics::record("warn", "audio", format!("Could not unregister audio notifications: {error}"));
                }
            }
            crate::diagnostics::record("debug", "audio", "Audio guard released; previous Windows mixer mute state restored");
        }
    }

    #[cfg(test)]
    #[test]
    #[ignore = "requires an interactive Windows audio endpoint; run a copy of the test executable named wallpaper64.exe from an isolated test folder"]
    fn silent_renderer_sessions_are_gated_and_restored_on_windows() {
        use windows::Win32::Media::Audio::{eConsole, IAudioClient, IAudioRenderClient,
            AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_NOPERSIST, AUDCLNT_BUFFERFLAGS_SILENT};
        unsafe fn silent_client(device: &windows::Win32::Media::Audio::IMMDevice, id: &GUID) -> (IAudioClient, ISimpleAudioVolume) {
            let client: IAudioClient = device.Activate(CLSCTX_ALL, None).unwrap();
            let format = client.GetMixFormat().unwrap();
            let initialized = client.Initialize(AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_NOPERSIST, 1_000_000, 0, format, Some(id));
            CoTaskMemFree(Some(format.cast()));
            initialized.unwrap();
            let volume: ISimpleAudioVolume = client.GetService().unwrap();
            let render: IAudioRenderClient = client.GetService().unwrap();
            let frames = client.GetBufferSize().unwrap();
            render.GetBuffer(frames).unwrap();
            render.ReleaseBuffer(frames, AUDCLNT_BUFFERFLAGS_SILENT.0 as u32).unwrap();
            client.Start().unwrap();
            (client, volume)
        }
        let executable = std::env::current_exe().unwrap();
        assert_eq!(executable.file_name().unwrap(), "wallpaper64.exe", "Use an isolated copy; this test only gates its own silent streams.");
        unsafe { CoInitializeEx(None, COINIT_MULTITHREADED).ok().unwrap(); }
        let _com = Com;
        unsafe {
            let enumerator: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).unwrap();
            let device = enumerator.GetDefaultAudioEndpoint(eRender, eConsole).unwrap();
            let (first, first_volume) = silent_client(&device, &GUID::from_u128(0x11abf6b1_bb4d_4648_a444_87eb5681d4de));
            first_volume.SetMute(false, &CONTEXT).unwrap();
            let mut guard = Guard::begin(&executable).unwrap();
            assert!(first_volume.GetMute().unwrap().as_bool(), "Existing stream must be muted before switching");
            let (second, second_volume) = silent_client(&device, &GUID::from_u128(0xc9902204_64b6_40fb_8dd1_871e8d371abc));
            let start = Instant::now();
            while !second_volume.GetMute().unwrap().as_bool() && start.elapsed() < Duration::from_secs(2) {
                std::thread::sleep(Duration::from_millis(10));
            }
            assert!(second_volume.GetMute().unwrap().as_bool(), "A new stream must be captured by the notification callback");
            guard.settle();
            drop(guard);
            assert!(!first_volume.GetMute().unwrap().as_bool());
            assert!(!second_volume.GetMute().unwrap().as_bool());
            first.Stop().unwrap();
            second.Stop().unwrap();
        }
    }
}

fn is_engine_process(folder: &Path, process: &Path) -> bool {
    let folder = crate::engine_config::normalize_path(&folder.to_string_lossy());
    let process = crate::engine_config::normalize_path(&process.to_string_lossy());
    process.strip_prefix(&format!("{}/", folder.trim_end_matches('/'))).is_some_and(|relative| {
        let name = relative.rsplit('/').next().unwrap_or("");
        matches!(name, "wallpaper32.exe" | "wallpaper64.exe" | "webwallpaper32.exe" | "webwallpaper64.exe")
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::Cell, rc::Rc};

    #[derive(Clone)]
    struct FakeVolume { muted: Rc<Cell<bool>>, fail: bool }
    impl FakeVolume {
        fn new(muted: bool) -> Self { Self { muted: Rc::new(Cell::new(muted)), fail: false } }
    }
    impl Volume for FakeVolume {
        fn muted(&self) -> Result<bool, String> { Ok(self.muted.get()) }
        fn set_muted(&self, muted: bool) -> Result<(), String> {
            if self.fail { return Err("device unavailable".into()); }
            self.muted.set(muted);
            Ok(())
        }
    }
    #[test]
    fn mutes_before_switch_and_restores_both_muted_and_unmuted_sessions() {
        let unmuted = FakeVolume::new(false);
        let muted = FakeVolume::new(true);
        let mut gate = Sessions::default();
        gate.capture("a".into(), "engine".into(), unmuted.clone(), false).unwrap();
        gate.capture("b".into(), "engine".into(), muted.clone(), false).unwrap();
        assert!(unmuted.muted.get() && muted.muted.get());
        gate.restore();
        assert!(!unmuted.muted.get());
        assert!(muted.muted.get());
    }
    #[test]
    fn a_recreated_stream_does_not_keep_the_guards_inherited_mute() {
        let original = FakeVolume::new(false);
        let new_stream = FakeVolume::new(true); // Inherited our Windows mixer mute.
        let mut gate = Sessions::default();
        gate.capture("old".into(), "engine".into(), original.clone(), false).unwrap();
        gate.capture("new".into(), "engine".into(), new_stream.clone(), true).unwrap();
        gate.restore();
        assert!(!original.muted.get());
        assert!(!new_stream.muted.get());
    }
    #[test]
    fn duplicate_notifications_do_not_overwrite_the_original_state() {
        let volume = FakeVolume::new(false);
        let mut gate = Sessions::default();
        gate.capture("a".into(), "engine".into(), volume.clone(), false).unwrap();
        gate.capture("a".into(), "engine".into(), volume.clone(), true).unwrap();
        drop(gate);
        assert!(!volume.muted.get());
    }
    #[test]
    fn partial_setup_failure_restores_previously_muted_sessions_on_drop() {
        let volume = FakeVolume::new(false);
        let result = (|| -> Result<(), String> {
            let mut gate = Sessions::default();
            gate.capture("a".into(), "engine".into(), volume.clone(), false)?;
            gate.capture("b".into(), "other".into(), FakeVolume { fail: true, ..FakeVolume::new(false) }, false)?;
            Ok(())
        })();
        assert!(result.is_err());
        assert!(!volume.muted.get());
    }
    #[test]
    fn late_callbacks_after_restore_cannot_mute_a_session_again() {
        let volume = FakeVolume::new(false);
        let mut gate = Sessions::default();
        gate.restore();
        gate.capture("late".into(), "engine".into(), volume.clone(), true).unwrap();
        assert!(!volume.muted.get());
    }
    #[test]
    fn only_known_renderer_executables_inside_engine_folder_match() {
        let folder = Path::new("C:/Steam/wallpaper_engine");
        assert!(is_engine_process(folder, Path::new("C:/Steam/wallpaper_engine/wallpaper64.exe")));
        assert!(is_engine_process(folder, Path::new(r"\\?\C:\Steam\wallpaper_engine\bin\WEBWALLPAPER32.EXE")));
        for path in ["C:/Steam/wallpaper_engine_backup/wallpaper64.exe", "C:/Other/wallpaper64.exe", "C:/Steam/wallpaper_engine/launcher.exe"] {
            assert!(!is_engine_process(folder, Path::new(path)), "{path}");
        }
    }
}
