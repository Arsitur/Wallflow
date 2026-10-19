//! WE writes getWallpaper to its console, not to a redirected stdout pipe.
//! A private pseudoconsole captures that response without showing a terminal window.
use std::path::Path;

#[cfg(not(windows))]
pub fn query(_executable: &Path, _monitor: u32) -> Option<Vec<u8>> { None }

#[cfg(windows)]
pub fn query(executable: &Path, monitor: u32) -> Option<Vec<u8>> {
    use std::{fs::File, io::Read, mem::{size_of, zeroed}, os::windows::{ffi::OsStrExt, io::{FromRawHandle, OwnedHandle}}, ptr::{null, null_mut}, time::Duration};
    use windows_sys::Win32::{Foundation::{HANDLE, WAIT_OBJECT_0}, System::{
        Console::{ClosePseudoConsole, CreatePseudoConsole, COORD, HPCON},
        Pipes::CreatePipe,
        Threading::{CreateProcessW, DeleteProcThreadAttributeList, InitializeProcThreadAttributeList,
            UpdateProcThreadAttribute, WaitForSingleObject, TerminateProcess, GetExitCodeProcess,
            EXTENDED_STARTUPINFO_PRESENT, PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE, PROCESS_INFORMATION, STARTF_USESTDHANDLES, STARTUPINFOEXW},
    }};

    struct Console(HPCON);
    impl Drop for Console { fn drop(&mut self) { unsafe { ClosePseudoConsole(self.0); } } }
    struct Attributes(Vec<usize>);
    impl Drop for Attributes {
        fn drop(&mut self) { unsafe { DeleteProcThreadAttributeList(self.0.as_mut_ptr().cast()); } }
    }
    unsafe fn pipe() -> Option<(OwnedHandle, OwnedHandle)> {
        let (mut read, mut write): (HANDLE, HANDLE) = (null_mut(), null_mut());
        if CreatePipe(&mut read, &mut write, null(), 0) == 0 { return None; }
        Some((OwnedHandle::from_raw_handle(read), OwnedHandle::from_raw_handle(write)))
    }
    fn wide(value: &std::ffi::OsStr) -> Vec<u16> { value.encode_wide().chain(Some(0)).collect() }
    use std::os::windows::io::AsRawHandle;

    // Every raw Win32 resource becomes an owning guard immediately after creation.
    unsafe {
        let (input_read, input_write) = pipe()?;
        let (output_read, output_write) = pipe()?;
        let mut console = 0;
        // Wide enough to keep even long wallpaper paths on one terminal line.
        if CreatePseudoConsole(COORD { X: 4096, Y: 8 }, input_read.as_raw_handle(), output_write.as_raw_handle(), 0, &mut console) < 0 { return None; }
        let console = Console(console);
        // Drain concurrently: ClosePseudoConsole can wait for the output pipe to be consumed.
        let reader = std::thread::spawn(move || {
            let mut file = File::from(output_read);
            let mut output = Vec::new();
            let mut buffer = [0u8; 4096];
            while let Ok(count) = file.read(&mut buffer) {
                if count == 0 { break; }
                let remaining = (32usize * 1024).saturating_sub(output.len());
                output.extend_from_slice(&buffer[..count.min(remaining)]);
            }
            output
        });
        let mut size = 0;
        InitializeProcThreadAttributeList(null_mut(), 1, 0, &mut size);
        if size == 0 { return None; }
        let mut storage = vec![0usize; size.div_ceil(size_of::<usize>())];
        if InitializeProcThreadAttributeList(storage.as_mut_ptr().cast(), 1, 0, &mut size) == 0 { return None; }
        let mut attributes = Attributes(storage);
        let list = attributes.0.as_mut_ptr().cast();
        if UpdateProcThreadAttribute(list, 0, PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE as usize,
            console.0 as *const _, size_of::<HPCON>(), null_mut(), null()) == 0 { return None; }
        let mut startup: STARTUPINFOEXW = zeroed();
        startup.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
        // Null standard handles force connection to ConPTY even when Wallflow's own output is redirected.
        startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
        startup.lpAttributeList = list;
        // WE attaches to its PARENT console. A console process must own the pseudoconsole;
        // launching the GUI executable into it directly still produces an empty response.
        // The bridge executes a fixed command with a checked literal executable name and an integer.
        // No playlist name, wallpaper path or user text is ever interpreted as shell syntax.
        let binary = match executable.file_name()?.to_str()? {
            "wallpaper64.exe" => "wallpaper64.exe",
            "wallpaper32.exe" => "wallpaper32.exe",
            _ => return None,
        };
        let bridge = std::path::PathBuf::from(std::env::var_os("SystemRoot")?).join("System32").join("cmd.exe");
        let application = wide(bridge.as_os_str());
        let mut command = wide(std::ffi::OsStr::new(&format!("\"{}\" /d /q /c .\\{binary} -control getWallpaper -monitor {monitor}", bridge.display())));
        // CMD mistakes the extended-length local prefix for a UNC working directory.
        let parent = executable.parent()?.to_string_lossy();
        let directory = wide(std::ffi::OsStr::new(parent.strip_prefix(r"\\?\").unwrap_or(&parent)));
        let mut process: PROCESS_INFORMATION = zeroed();
        if CreateProcessW(application.as_ptr(), command.as_mut_ptr(), null(), null(), 0,
            EXTENDED_STARTUPINFO_PRESENT, null(), directory.as_ptr(), &startup.StartupInfo, &mut process) == 0 { return None; }
        drop(input_read);
        drop(output_write);
        let process_handle = OwnedHandle::from_raw_handle(process.hProcess);
        drop(OwnedHandle::from_raw_handle(process.hThread));
        let timed_out = WaitForSingleObject(process_handle.as_raw_handle(), 1000) != WAIT_OBJECT_0;
        if timed_out {
            TerminateProcess(process_handle.as_raw_handle(), 1);
            WaitForSingleObject(process_handle.as_raw_handle(), 250);
        }
        let mut code = 1;
        GetExitCodeProcess(process_handle.as_raw_handle(), &mut code);
        // Allow the console host to deliver the final response before closing its stream.
        std::thread::sleep(Duration::from_millis(30));
        drop(input_write);
        drop(console);
        let output = reader.join().ok()?;
        if timed_out || code != 0 { None } else { Some(output) }
    }
}
