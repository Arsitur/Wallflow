//! Snapshot Wallpaper Engine's *manual* pause flag before replacing a renderer.
//! The documented CLI can pause/play, but cannot report the current state.
//! On recognized Windows builds we locate the flag used by the native tray's
//! Pause/Resume label in the installed PE image, then read that single DWORD.
//! No process writes, injected code, hooks, or guessed CPU/focus states.
use std::path::Path;

pub fn paused(executable: &Path) -> Result<bool, String> { native::snapshot(executable).map(|(flags, _)| flags & 1 != 0) }
pub fn repaint_paused(executable: &Path) -> Result<(), String> { native::repaint_paused(executable) }
pub fn owner_pid(executable: &Path) -> Result<u32, String> { native::snapshot(executable).map(|(_, pid)| pid) }
#[cfg(test)]
pub fn muted(executable: &Path) -> Result<bool, String> { native::snapshot(executable).map(|(flags, _)| flags & 64 != 0) }

#[cfg(not(windows))]
mod native {
    use super::*;
    pub fn snapshot(_: &Path) -> Result<(u32, u32), String> { Err("Playback state is only available on Windows".into()) }
    pub fn repaint_paused(_: &Path) -> Result<(), String> { Ok(()) }
}

#[cfg(any(windows, test))]
mod image {
    struct Section { rva: u32, virtual_size: u32, offset: usize, size: usize, flags: u32 }
    pub struct Location { pub rva: u32, pub image_size: u32, pub timestamp: u32, pub machine: u16 }
    fn u16_at(b: &[u8], at: usize) -> Option<u16> { Some(u16::from_le_bytes(b.get(at..at.checked_add(2)?)?.try_into().ok()?)) }
    fn u32_at(b: &[u8], at: usize) -> Option<u32> { Some(u32::from_le_bytes(b.get(at..at.checked_add(4)?)?.try_into().ok()?)) }
    fn relative(rva: u32, size: u32, displacement: u32) -> Option<u32> {
        u32::try_from(i64::from(rva) + i64::from(size) + i64::from(displacement as i32)).ok()
    }

    pub fn locate(bytes: &[u8]) -> Option<Location> {
        if bytes.len() > 32 * 1024 * 1024 || bytes.get(..2)? != b"MZ" { return None; }
        let pe = u32_at(bytes, 0x3c)? as usize;
        if bytes.get(pe..pe.checked_add(4)?)? != b"PE\0\0" { return None; }
        let machine = u16_at(bytes, pe + 4)?;
        let count = usize::from(u16_at(bytes, pe + 6)?);
        if count == 0 || count > 96 { return None; }
        let optional = pe + 24;
        let magic = u16_at(bytes, optional)?;
        let is64 = match (machine, magic) { (0x8664, 0x20b) => true, (0x14c, 0x10b) => false, _ => return None };
        let preferred_base = if is64 { 0 } else { u32_at(bytes, optional + 28)? };
        let image_size = u32_at(bytes, optional + 56)?;
        if image_size == 0 || image_size > 512 * 1024 * 1024 { return None; }
        let table = optional + usize::from(u16_at(bytes, pe + 20)?);
        let mut sections = Vec::new();
        for index in 0..count {
            let at = table + index * 40;
            let section = Section { virtual_size: u32_at(bytes, at + 8)?, rva: u32_at(bytes, at + 12)?, size: u32_at(bytes, at + 16)? as usize, offset: u32_at(bytes, at + 20)? as usize, flags: u32_at(bytes, at + 36)? };
            bytes.get(section.offset..section.offset.checked_add(section.size)?)?;
            if section.rva.checked_add(section.virtual_size.max(section.size as u32))? > image_size { return None; }
            sections.push(section);
        }
        let string_rva = |needle: &[u8]| -> Option<u32> {
            let mut found = None;
            for section in sections.iter().filter(|s| s.flags & 0xa0000000 == 0 && s.flags & 0x40000000 != 0) {
                for (index, _) in bytes[section.offset..section.offset + section.size].windows(needle.len()).enumerate().filter(|(_, b)| *b == needle) {
                    let rva = section.rva.checked_add(index as u32)?;
                    if found.replace(rva).is_some() { return None; }
                }
            }
            found
        };
        let resume = string_rva(b"core_tray_unpause\0")?;
        let pause = string_rva(b"core_tray_pause\0")?;
        let mut found = None;
        for section in sections.iter().filter(|s| s.flags & 0x20000020 == 0x20000020) {
            let code = &bytes[section.offset..section.offset + section.size];
            for index in 0..code.len().saturating_sub(80) {
                let c = &code[index..index + 80];
                let instruction = section.rva.checked_add(index as u32)?;
                let candidate = if is64 {
                    // mov edi,[rip+flags]; lea rax,resume; movzx ebx,dil;
                    // lea r8,pause; ... and bl,1; ... cmovne r8,rax.
                    if c[..2] != [0x8b, 0x3d] || c[6..9] != [0x48, 0x8d, 0x05]
                        || c[13..20] != [0x40, 0x0f, 0xb6, 0xdf, 0x4c, 0x8d, 0x05]
                        || relative(instruction.checked_add(6)?, 7, u32_at(c, 9)?) != Some(resume)
                        || relative(instruction.checked_add(17)?, 7, u32_at(c, 20)?) != Some(pause)
                        || !c[24..48].windows(3).any(|b| b == [0x80, 0xe3, 1])
                        || !c[24..56].windows(4).any(|b| b == [0x4c, 0x0f, 0x45, 0xc0]) { continue; }
                    relative(instruction, 6, u32_at(c, 2)?)?
                } else {
                    // mov eax,[flags]; mov edx,pause; mov cl,al; shr eax,6;
                    // and al,1; and cl,1; ... mov eax,resume; ... cmove eax,edx.
                    if c[0] != 0xa1 || c[5] != 0xba || c[10..20] != [0x8a, 0xc8, 0xc1, 0xe8, 6, 0x24, 1, 0x80, 0xe1, 1]
                        || u32_at(c, 6)?.checked_sub(preferred_base) != Some(pause)
                        || c[25] != 0xb8 || u32_at(c, 26)?.checked_sub(preferred_base) != Some(resume)
                        || c[33..36] != [0x0f, 0x44, 0xc2] { continue; }
                    u32_at(c, 1)?.checked_sub(preferred_base)?
                };
                // The result must be one aligned DWORD in writable, non-executable data.
                if candidate % 4 != 0 || candidate.checked_add(4)? > image_size
                    || !sections.iter().any(|s| s.flags & 0xa0000000 == 0x80000000
                        && candidate >= s.rva && u64::from(candidate) + 4 <= u64::from(s.rva) + u64::from(s.virtual_size)) { continue; }
                if found.replace(candidate).is_some() { return None; }
            }
        }
        Some(Location { rva: found?, image_size, timestamp: u32_at(bytes, pe + 8)?, machine })
    }
}

#[cfg(windows)]
mod native {
    use super::*;
    use std::{ffi::OsString, os::windows::{ffi::{OsStrExt, OsStringExt}}, fs};
    use windows_sys::Win32::{Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE},
        System::{Diagnostics::{Debug::ReadProcessMemory, ToolHelp::{CreateToolhelp32Snapshot, Module32FirstW, MODULEENTRY32W, TH32CS_SNAPMODULE, TH32CS_SNAPMODULE32}},
            Threading::{OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_VM_READ}},
        UI::WindowsAndMessaging::{FindWindowExW, GetWindowThreadProcessId}};

    struct Handle(HANDLE);
    impl Drop for Handle { fn drop(&mut self) { unsafe { CloseHandle(self.0); } } }
    fn read(process: HANDLE, address: usize, bytes: &mut [u8]) -> Result<(), String> {
        let mut count = 0;
        if unsafe { ReadProcessMemory(process, address as _, bytes.as_mut_ptr().cast(), bytes.len(), &mut count) } == 0 || count != bytes.len() {
            return Err("Could not read Wallpaper Engine playback state".into());
        }
        Ok(())
    }
    pub fn snapshot(executable: &Path) -> Result<(u32, u32), String> {
        let folder = executable.parent().ok_or("Wallpaper Engine directory unavailable")?.canonicalize().map_err(|e| e.to_string())?;
        let class: Vec<u16> = std::ffi::OsStr::new("WPETrayWindow").encode_wide().chain(Some(0)).collect();
        let mut window = std::ptr::null_mut();
        for _ in 0..16 {
            window = unsafe { FindWindowExW(std::ptr::null_mut(), window, class.as_ptr(), std::ptr::null()) };
            if window.is_null() { break; }
            let mut pid = 0;
            unsafe { GetWindowThreadProcessId(window, &mut pid); }
            let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_VM_READ, 0, pid) };
            if process.is_null() { continue; }
            let process = Handle(process);
            let mut path = vec![0u16; 32768];
            let mut length = path.len() as u32;
            if unsafe { QueryFullProcessImageNameW(process.0, 0, path.as_mut_ptr(), &mut length) } == 0 { continue; }
            let path = std::path::PathBuf::from(OsString::from_wide(&path[..length as usize]));
            if !path.file_name().is_some_and(|n| n.eq_ignore_ascii_case("wallpaper64.exe") || n.eq_ignore_ascii_case("wallpaper32.exe"))
                || path.parent().and_then(|p| p.canonicalize().ok()).as_ref() != Some(&folder) { continue; }
            let size = fs::metadata(&path).map_err(|e| e.to_string())?.len();
            if size > 32 * 1024 * 1024 { return Err("Unrecognized Wallpaper Engine executable size".into()); }
            let bytes = fs::read(&path).map_err(|e| e.to_string())?;
            let location = image::locate(&bytes).ok_or("This Wallpaper Engine build has an unrecognized playback-state layout")?;
            let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPMODULE | TH32CS_SNAPMODULE32, pid) };
            if snapshot == INVALID_HANDLE_VALUE { return Err("Could not inspect Wallpaper Engine's main module".into()); }
            let snapshot = Handle(snapshot);
            let mut module: MODULEENTRY32W = unsafe { std::mem::zeroed() };
            module.dwSize = std::mem::size_of::<MODULEENTRY32W>() as u32;
            if unsafe { Module32FirstW(snapshot.0, &mut module) } == 0 { return Err("Wallpaper Engine's main module is unavailable".into()); }
            let module_path = std::path::PathBuf::from(OsString::from_wide(&module.szExePath[..module.szExePath.iter().position(|c| *c == 0).unwrap_or(module.szExePath.len())]));
            if module_path.canonicalize().map_err(|e| e.to_string())? != path.canonicalize().map_err(|e| e.to_string())? || module.modBaseSize != location.image_size { return Err("Wallpaper Engine changed while reading playback state".into()); }
            let base = module.modBaseAddr as usize;
            // Reject disk/process version mismatches, e.g. an update pending restart.
            let mut dos = [0u8; 64]; read(process.0, base, &mut dos)?;
            let pe = u32::from_le_bytes(dos[60..64].try_into().unwrap()) as usize;
            if &dos[..2] != b"MZ" || pe > 1024 * 1024 { return Err("Unrecognized Wallpaper Engine module header".into()); }
            let mut header = [0u8; 12]; read(process.0, base + pe, &mut header)?;
            if &header[..4] != b"PE\0\0" || u16::from_le_bytes(header[4..6].try_into().unwrap()) != location.machine
                || u32::from_le_bytes(header[8..12].try_into().unwrap()) != location.timestamp { return Err("Restart Wallpaper Engine after its update to read playback state".into()); }
            let mut flags = [0u8; 4]; read(process.0, base + location.rva as usize, &mut flags)?;
            return Ok((u32::from_le_bytes(flags), pid));
        }
        Err("The installed Wallpaper Engine process could not be found".into())
    }

    pub fn repaint_paused(executable: &Path) -> Result<(), String> {
        use windows_sys::Win32::{Foundation::HWND, Graphics::Gdi::{RedrawWindow, RDW_INVALIDATE, RDW_NOERASE},
            UI::WindowsAndMessaging::{EnumWindows, EnumChildWindows, GetClassNameW}};
        let (flags, pid) = snapshot(executable)?;
        if flags & 1 == 0 { return Ok(()); } // The user may have resumed meanwhile.
        struct Request { pid: u32, count: u32 }
        unsafe extern "system" fn inspect(window: HWND, data: isize) -> i32 {
            let request = &mut *(data as *mut Request);
            let mut pid = 0;
            GetWindowThreadProcessId(window, &mut pid);
            if pid != request.pid { return 1; }
            let mut name = [0u16; 128];
            let length = GetClassNameW(window, name.as_mut_ptr(), name.len() as i32).max(0) as usize;
            if matches!(String::from_utf16_lossy(&name[..length]).as_str(), "WPEVideoWallpaper" | "WPECloneView") {
                // Queue paint only. Do not advance the media clock, resize the
                // window, erase its current image, or synchronously block on WE.
                if RedrawWindow(window, std::ptr::null(), std::ptr::null_mut(), RDW_INVALIDATE | RDW_NOERASE) != 0 { request.count += 1; }
            }
            1
        }
        unsafe extern "system" fn top(window: HWND, data: isize) -> i32 {
            inspect(window, data);
            EnumChildWindows(window, Some(inspect), data);
            1
        }
        let mut request = Request { pid, count: 0 };
        unsafe { EnumWindows(Some(top), &mut request as *mut Request as isize); }
        crate::diagnostics::record("debug", "playback", format!("Requested static repaint for {} paused video/clone windows; playback remains paused", request.count));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture(is64: bool) -> Vec<u8> {
        fn put(b: &mut [u8], at: usize, n: u32) { b[at..at + 4].copy_from_slice(&n.to_le_bytes()); }
        let mut b = vec![0; 0x800];
        b[..2].copy_from_slice(b"MZ"); put(&mut b, 60, 0x80);
        b[0x80..0x84].copy_from_slice(b"PE\0\0");
        b[0x84..0x86].copy_from_slice(&(if is64 { 0x8664u16 } else { 0x14cu16 }).to_le_bytes());
        b[0x86] = 3; b[0x94] = 0x80;
        b[0x98..0x9a].copy_from_slice(&(if is64 { 0x20bu16 } else { 0x10bu16 }).to_le_bytes());
        put(&mut b, 0x98 + 28, 0x400000); put(&mut b, 0x98 + 56, 0x4000);
        for (i, (rva, off, flags)) in [(0x1000, 0x200, 0x60000020), (0x2000, 0x400, 0x40000040), (0x3000, 0x600, 0xc0000040)].into_iter().enumerate() {
            let s = 0x118 + i * 40;
            put(&mut b, s + 8, 0x300); put(&mut b, s + 12, rva);
            put(&mut b, s + 16, 0x200); put(&mut b, s + 20, off); put(&mut b, s + 36, flags);
        }
        b[0x400..0x412].copy_from_slice(b"core_tray_unpause\0");
        b[0x430..0x440].copy_from_slice(b"core_tray_pause\0");
        if is64 {
            b[0x200..0x202].copy_from_slice(&[0x8b, 0x3d]); put(&mut b, 0x202, 0x3200 - 0x1006);
            b[0x206..0x209].copy_from_slice(&[0x48, 0x8d, 0x05]); put(&mut b, 0x209, 0x2000 - 0x100d);
            b[0x20d..0x214].copy_from_slice(&[0x40, 0x0f, 0xb6, 0xdf, 0x4c, 0x8d, 0x05]); put(&mut b, 0x214, 0x2030 - 0x1018);
            b[0x220..0x223].copy_from_slice(&[0x80, 0xe3, 1]); b[0x228..0x22c].copy_from_slice(&[0x4c, 0x0f, 0x45, 0xc0]);
        } else {
            b[0x200] = 0xa1; put(&mut b, 0x201, 0x403200); b[0x205] = 0xba; put(&mut b, 0x206, 0x402030);
            b[0x20a..0x214].copy_from_slice(&[0x8a, 0xc8, 0xc1, 0xe8, 6, 0x24, 1, 0x80, 0xe1, 1]);
            b[0x219] = 0xb8; put(&mut b, 0x21a, 0x402000); b[0x221..0x224].copy_from_slice(&[0x0f, 0x44, 0xc2]);
        }
        b
    }
    #[test]
    fn locates_pause_flag_in_32_and_64_bit_images_including_uninitialized_data() {
        for is64 in [false, true] {
            let location = image::locate(&fixture(is64)).unwrap();
            assert_eq!(location.rva, 0x3200); assert_eq!(location.image_size, 0x4000);
        }
    }
    #[test]
    fn rejects_ambiguous_signatures_and_non_data_targets() {
        for is64 in [false, true] {
            let b = fixture(is64);
            let mut duplicate = b.clone(); duplicate.copy_within(0x200..0x250, 0x280);
            if is64 {
                // Relocate the RIP-relative operands to reference the same targets.
                for offset in [2, 9, 20] {
                    let n = u32::from_le_bytes(duplicate[0x280 + offset..0x284 + offset].try_into().unwrap()) - 0x80;
                    duplicate[0x280 + offset..0x284 + offset].copy_from_slice(&n.to_le_bytes());
                }
            }
            assert!(image::locate(&duplicate).is_none());
            let mut readonly = b.clone(); readonly[0x118 + 80 + 36..0x118 + 80 + 40].copy_from_slice(&0x40000040u32.to_le_bytes());
            assert!(image::locate(&readonly).is_none());
            let mut unknown = b.clone(); unknown[0x20a] ^= 0xff;
            assert!(image::locate(&unknown).is_none());
            for length in 0..b.len() { assert!(image::locate(&b[..length]).is_none()); }
        }
    }
    #[test]
    fn malformed_images_never_produce_an_address() {
        for size in [0, 2, 63, 64, 1024] { assert!(image::locate(&vec![0; size]).is_none()); }
        let mut bytes = vec![0; 64]; bytes[..2].copy_from_slice(b"MZ"); bytes[60..64].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(image::locate(&bytes).is_none());
    }
    #[test]
    #[ignore = "Read-only integration check against the locally installed Wallpaper Engine"]
    fn installed_engine_pause_state() {
        let executable = std::env::var_os("WALLFLOW_ENGINE_TEST_EXE").expect("Set WALLFLOW_ENGINE_TEST_EXE");
        println!("Wallpaper Engine manually paused: {}", paused(Path::new(&executable)).unwrap());
    }
}
