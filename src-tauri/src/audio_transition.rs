//! Gate only Wallpaper Engine's Windows mixer sessions while a new renderer
//! initializes. Use temporary volume attenuation, never the mute switch that
//! Wallpaper Engine and its Ctrl+PgDn shortcut also control.
use std::{collections::HashMap, path::Path};

trait Volume {
    fn level(&self) -> Result<f32, String>;
    fn set_level(&self, level: f32) -> Result<(), String>;
}

struct Sessions<V: Volume> {
    active: bool,
    originals: HashMap<String, (V, f32)>,
    // New streams may inherit the zero volume set on the same session group.
    baselines: HashMap<String, Option<f32>>,
}
impl<V: Volume> Default for Sessions<V> {
    fn default() -> Self { Self { active: true, originals: HashMap::new(), baselines: HashMap::new() } }
}
impl<V: Volume> Sessions<V> {
    fn capture(&mut self, instance: String, owner: String, volume: V, new_session: bool) -> Result<(), String> {
        if !self.active || self.originals.contains_key(&instance) { return Ok(()); }
        let level = volume.level()?;
        let original = if new_session {
            self.baselines.get(&owner).copied().flatten().unwrap_or(level)
        } else {
            self.baselines.entry(owner).and_modify(|baseline| {
                if *baseline != Some(level) { *baseline = None; }
            }).or_insert(Some(level));
            level
        };
        volume.set_level(0.0)?;
        self.originals.insert(instance, (volume, original));
        Ok(())
    }
    fn restore(&mut self) {
        self.active = false;
        for (_, (volume, original)) in self.originals.drain() {
            if let Err(error) = volume.set_level(original) {
                crate::diagnostics::record("warn", "audio", format!("Could not restore an audio session (it may have closed): {error}"));
            }
        }
    }
}
impl<V: Volume> Drop for Sessions<V> { fn drop(&mut self) { self.restore(); } }

// Finalize renderer state after startup, but before restoring audible levels.
// A paused transition remains visually frozen by frame_hold while WE loads.
pub fn protect<T>(executable: &Path, enabled: bool, operation: impl FnOnce() -> Result<T, String>, finish: impl FnOnce(Result<T, String>) -> Result<T, String>) -> Result<T, String> {
    if !enabled { return finish(operation()); }
    let guard = native::Guard::begin(executable).map_err(|e| format!("Could not protect Wallpaper Engine audio before switching: {e}"))?;
    guarded(guard, operation, finish)
}

trait StartupGuard { fn settle(&mut self); }
impl StartupGuard for native::Guard { fn settle(&mut self) { native::Guard::settle(self); } }
fn guarded<T>(mut guard: impl StartupGuard, operation: impl FnOnce() -> Result<T, String>, finish: impl FnOnce(Result<T, String>) -> Result<T, String>) -> Result<T, String> {
    let result = operation();
    // openWallpaper acknowledges the command before Media Foundation has fully
    // initialized. Cover that startup window as well as applyProperties.
    guard.settle();
    let result = finish(result);
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
        fn level(&self) -> Result<f32, String> { unsafe { self.GetMasterVolume().map_err(|e| e.to_string()) } }
        fn set_level(&self, level: f32) -> Result<(), String> { unsafe { self.SetMasterVolume(level, &CONTEXT).map_err(|e| e.to_string()) } }
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
            crate::diagnostics::record("debug", "audio", format!("Audio volume guard armed before switch: {count} Wallpaper Engine sessions; mute controls remain independent"));
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
            crate::diagnostics::record("debug", "audio", "Audio volume guard released; original levels restored without changing mute");
        }
    }

    #[cfg(test)]
    #[test]
    #[ignore = "Installed audio diagnostics; explicit WALLFLOW_REPAIR_STUCK_MUTE=1 repairs old guard mute when WE itself is unmuted"]
    fn installed_audio_session_state() {
        let executable = PathBuf::from(std::env::var_os("WALLFLOW_ENGINE_TEST_EXE").unwrap());
        let folder = executable.parent().unwrap();
        let repair = std::env::var("WALLFLOW_REPAIR_STUCK_MUTE").ok().as_deref() == Some("1");
        if repair { assert!(!crate::playback_state::muted(&executable).expect("Must verify Engine is unmuted before repairing stale mixer mute")); }
        unsafe { CoInitializeEx(None, COINIT_MULTITHREADED).ok().unwrap(); }
        let _com = Com;
        unsafe {
            let devices: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).unwrap();
            let endpoints = devices.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE).unwrap();
            for index in 0..endpoints.GetCount().unwrap() {
                let manager: IAudioSessionManager2 = endpoints.Item(index).unwrap().Activate(CLSCTX_ALL, None).unwrap();
                let sessions = manager.GetSessionEnumerator().unwrap();
                for i in 0..sessions.GetCount().unwrap() {
                    let control: IAudioSessionControl2 = sessions.GetSession(i).unwrap().cast().unwrap();
                    let pid = control.GetProcessId().unwrap();
                    if process_path(pid).is_some_and(|p| is_engine_process(&folder, &p)) {
                        let volume: ISimpleAudioVolume = control.cast().unwrap();
                        println!("Endpoint {index} engine PID {pid}: mute={}, volume={}, state={:?}", volume.GetMute().unwrap().as_bool(), volume.GetMasterVolume().unwrap(), control.GetState().unwrap());
                        if repair && volume.GetMute().unwrap().as_bool() {
                            volume.SetMute(false, &CONTEXT).unwrap();
                            assert!(!volume.GetMute().unwrap().as_bool());
                            println!("Cleared stale Windows mixer mute for this Wallpaper Engine session");
                        }
                    }
                }
            }
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
            assert_eq!(first_volume.GetMasterVolume().unwrap(), 0.0, "Existing stream must be silent before switching");
            assert!(!first_volume.GetMute().unwrap().as_bool(), "The guard must not change mute");
            let (second, second_volume) = silent_client(&device, &GUID::from_u128(0xc9902204_64b6_40fb_8dd1_871e8d371abc));
            let start = Instant::now();
            while second_volume.GetMasterVolume().unwrap() != 0.0 && start.elapsed() < Duration::from_secs(2) {
                std::thread::sleep(Duration::from_millis(10));
            }
            assert_eq!(second_volume.GetMasterVolume().unwrap(), 0.0, "A new stream must be captured by the notification callback");
            // Simulate an engine mute change during switching: restoring levels
            // must not undo the shortcut's decision, even on another stream.
            first_volume.SetMute(true, &CONTEXT).unwrap();
            second_volume.SetMute(false, &CONTEXT).unwrap();
            guard.settle();
            drop(guard);
            assert_eq!(first_volume.GetMasterVolume().unwrap(), 1.0);
            assert_eq!(second_volume.GetMasterVolume().unwrap(), 1.0);
            assert!(first_volume.GetMute().unwrap().as_bool());
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

    #[test]
    fn startup_finishes_before_restoring_pause_and_sound_even_on_failure() {
        use std::cell::RefCell;
        struct Guard(Rc<RefCell<Vec<&'static str>>>);
        impl StartupGuard for Guard { fn settle(&mut self) { self.0.borrow_mut().push("video startup"); } }
        impl Drop for Guard { fn drop(&mut self) { self.0.borrow_mut().push("restore volume"); } }
        for fail in [false, true] {
            let steps = Rc::new(RefCell::new(Vec::new()));
            let result = guarded(Guard(steps.clone()), || {
                steps.borrow_mut().push("open replacement without changing pause");
                if fail { Err("Loading failed".to_owned()) } else { Ok(()) }
            }, |result| {
                steps.borrow_mut().push("restore pause");
                result
            });
            assert_eq!(result.is_err(), fail);
            assert_eq!(*steps.borrow(), ["open replacement without changing pause", "video startup", "restore pause", "restore volume"]);
        }
    }

    #[derive(Clone)]
    struct FakeVolume { level: Rc<Cell<f32>>, muted: Rc<Cell<bool>>, fail: bool }
    impl FakeVolume {
        fn new(level: f32, muted: bool) -> Self { Self { level: Rc::new(Cell::new(level)), muted: Rc::new(Cell::new(muted)), fail: false } }
    }
    impl Volume for FakeVolume {
        fn level(&self) -> Result<f32, String> { Ok(self.level.get()) }
        fn set_level(&self, level: f32) -> Result<(), String> {
            if self.fail { return Err("device unavailable".into()); }
            self.level.set(level);
            Ok(())
        }
    }
    #[test]
    fn silences_before_switch_and_restores_levels_without_changing_mute() {
        let unmuted = FakeVolume::new(0.7, false);
        let muted = FakeVolume::new(0.3, true);
        let mut gate = Sessions::default();
        gate.capture("a".into(), "engine".into(), unmuted.clone(), false).unwrap();
        gate.capture("b".into(), "engine".into(), muted.clone(), false).unwrap();
        assert_eq!(unmuted.level.get(), 0.0);
        assert_eq!(muted.level.get(), 0.0);
        assert!(!unmuted.muted.get() && muted.muted.get());
        gate.restore();
        assert!(!unmuted.muted.get());
        assert!(muted.muted.get());
        assert_eq!(unmuted.level.get(), 0.7);
        assert_eq!(muted.level.get(), 0.3);
    }
    #[test]
    fn a_recreated_stream_does_not_keep_the_guards_inherited_zero_volume() {
        let original = FakeVolume::new(0.8, false);
        let new_stream = FakeVolume::new(0.0, false);
        let mut gate = Sessions::default();
        gate.capture("old".into(), "engine".into(), original.clone(), false).unwrap();
        gate.capture("new".into(), "engine".into(), new_stream.clone(), true).unwrap();
        gate.restore();
        assert_eq!(original.level.get(), 0.8);
        assert_eq!(new_stream.level.get(), 0.8);
    }
    #[test]
    fn duplicate_notifications_do_not_overwrite_the_original_state() {
        let volume = FakeVolume::new(0.4, false);
        let mut gate = Sessions::default();
        gate.capture("a".into(), "engine".into(), volume.clone(), false).unwrap();
        gate.capture("a".into(), "engine".into(), volume.clone(), true).unwrap();
        drop(gate);
        assert_eq!(volume.level.get(), 0.4);
    }
    #[test]
    fn partial_setup_failure_restores_previously_silenced_sessions_on_drop() {
        let volume = FakeVolume::new(0.5, false);
        let result = (|| -> Result<(), String> {
            let mut gate = Sessions::default();
            gate.capture("a".into(), "engine".into(), volume.clone(), false)?;
            gate.capture("b".into(), "other".into(), FakeVolume { fail: true, ..FakeVolume::new(1.0, false) }, false)?;
            Ok(())
        })();
        assert!(result.is_err());
        assert_eq!(volume.level.get(), 0.5);
    }
    #[test]
    fn late_callbacks_after_restore_cannot_silence_a_session_again() {
        let volume = FakeVolume::new(0.9, false);
        let mut gate = Sessions::default();
        gate.restore();
        gate.capture("late".into(), "engine".into(), volume.clone(), true).unwrap();
        assert_eq!(volume.level.get(), 0.9);
    }
    #[test]
    fn mute_and_unmute_during_a_switch_survive_restoration() {
        for original_mute in [false, true] {
            let volume = FakeVolume::new(0.6, original_mute);
            let mut gate = Sessions::default();
            gate.capture("a".into(), "engine".into(), volume.clone(), false).unwrap();
            volume.muted.set(!original_mute); // Wallpaper Engine's shortcut.
            gate.restore();
            assert_eq!(volume.muted.get(), !original_mute);
            assert_eq!(volume.level.get(), 0.6);
            // It must remain possible to toggle after the guard has gone, too.
            volume.muted.set(original_mute);
            assert_eq!(volume.muted.get(), original_mute);
        }
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
