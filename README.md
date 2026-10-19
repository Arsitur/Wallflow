# Wallflow v0.1 🗡️

An animated, auditable wallpaper selector for **Wallpaper Engine on Windows 11**.

- Transparent, image-only carousel with slim portrait previews and smooth scaling.
- Browse installed Steam Workshop wallpapers and local Wallpaper Engine projects. The Arthurian playlist is selected by default when present; right-click the carousel to choose a playlist or all installed wallpapers. Settings have no overlay button or keyboard shortcut.
- Deduplicated library sorted by local installation/creation date, newest first; built-in default projects are excluded.
- Browse one combined library with smooth mouse wheel, trackpad, drag or arrow-key navigation. Press R to rescan.
- Cards bend slightly during scrolling and settle back afterward; reduced-motion preferences disable this effect. Each cover has a centered loading spinner until its image is ready.
- `Ctrl + Alt + G` toggles the popup globally by default; change the shortcut in Settings. `Esc` closes Settings first, then hides the selector.
- `Enter` applies the centered wallpaper; one click on any visible card applies that wallpaper immediately using Wallpaper Engine's *official* CLI.
- Opens on the monitor of the previously focused app, matches that display to Wallpaper Engine, and centers its current wallpaper on each opening. Applying a wallpaper uses that monitor, restores the previous app focus, then resumes playback.
- Full monitor width with a centered transparent window (66% of monitor height), plus fade/scale animations. Native resize and DPI changes finish while hidden before the frontend acknowledges display readiness.

## Requirements

- Windows 10/11, Steam + Wallpaper Engine installed.
- Wallpaper Engine already running before applying wallpapers.
- Node.js 20+, npm.
- Rust/MSVC toolchain for building native Tauri app, including Microsoft C++ Build Tools and WebView2 runtime.
- Latest Tauri global shortcut plugin may require Rust 1.90+. Run `rustup update` if necessary.

See Tauri prerequisites: https://v2.tauri.app/start/prerequisites/

## Quick start (PowerShell)

```powershell
# Extract ZIP to a normal folder, open PowerShell there
npm install
rustup update stable
npm run tauri dev
```

Once it starts, press `Ctrl + Alt + G` from anywhere. It remains in memory when hidden. Click the Wallflow icon in the notification area to reopen the gallery. Right-click it and choose **Quit Wallflow** to exit entirely. Esc and Alt + F4 hide the gallery and restore the previous foreground window. Switching focus to another application dismisses the gallery without changing the new app focus.

For frontend development (the browser shows an empty state; installed wallpapers require Tauri):

```powershell
npm install
npm run dev
```

Then visit http://127.0.0.1:1420. Demo wallpapers and the standalone concept preview have been removed.

## Build your own Windows app

```powershell
npm run tauri build
```

After successful compilation, the NSIS installer should be in `src-tauri/target/release/bundle/nsis/`. Do **not** install an unsigned binary from a stranger. Your own locally built binary is much easier to inspect.

## How it works

1. `src-tauri/src/wallpaper.rs` locates Steam from `HKCU\Software\Valve\Steam`, the default install locations, and `steamapps/libraryfolders.vdf`.
2. It reads `steamapps/workshop/content/431960/*/project.json` and local project metadata without altering files.
3. The JavaScript/Svelte frontend loads one 560 × 1240 cover per wallpaper through a two-request priority queue, nearest cards first. For scenes, Rust reads original artwork from the installed project or PKGV archive and decodes TEXV textures (RGBA, BC1/BC2/BC3, embedded images and LZ4 blocks). Candidate artwork is compared with the author's preview to avoid isolated layers and masks. Only the portrait crop is resized; cached covers reopen directly without thumbnail-to-HQ swaps. Images use their original file; videos use a frame from an existing FFmpeg installation. Unsupported scenes fall back to the author's preview. Source path, size and modification time invalidate cached images. Wallpaper scripts and effects are not executed for covers.
4. Every display uses the same named playlist membership, including unsaved additions to that playlist on any display. Each opening queries Wallpaper Engine through an invisible Windows pseudoconsole and a fixed system-console bridge (its CLI attaches to its parent's console), with saved configuration as fallback. The current wallpaper is centered only when it belongs to the selected playlist.
5. On Apply, the backend invokes the **installed** `wallpaper64.exe` or `wallpaper32.exe` directly with:

```text
-control openWallpaper -file <validated-wallpaper-path> -monitor 0
```

Apply and playback use `std::process::Command` with separated arguments. The read-only current-wallpaper query uses the Windows system `cmd.exe` as an invisible console owner with a fixed command, a checked executable name and a numeric monitor index; user text and wallpaper paths never become shell commands. Official CLI documentation: https://help.wallpaperengine.io/en/functionality/cli.html

## Security model

- No external assets, cloud access, telemetry, auto-updates, administrator rights, or startup registry modifications.
- No third-party executable bundled in this source ZIP.
- Native access is restricted to local installed wallpaper assets, cache files and your installed Wallpaper Engine executable.
- Wallpaper project `file` paths are canonicalized and checked to remain inside their project directory.
- Arbitrary user-entered executable paths are **not** accepted for wallpaper application.
- Browser preview cannot access the filesystem or Wallpaper Engine.
- NPM and Cargo fetch ordinary **development dependencies** when you build; inspect dependency changes before distributing binaries.

## Limitations in v0.1

- Source-code app, with frontend and native automated checks; Wallpaper Engine interaction still needs validation against your installation.
- Detects wallpapers only in discoverable Steam libraries and expected project folders, not every manually imported video/image.
- Original source covers are generated once in the background, then reused from disk and memory. Scene covers show static source artwork, so shader effects and the final arrangement of complex animated layers can differ. Unsupported texture formats fall back to the author's preview. FFmpeg is optional for video frames; the app never installs it automatically. Installation dates use local folder creation times, with modification time as a fallback; copied/restored libraries may have different dates.
- May not restore customized Wallpaper Engine *per-wallpaper property presets*. We should support `-preset` next.
- No click-through desktop effect, audio previews or file drag/drop yet. If the active wallpaper is outside the selected playlist, the carousel starts at its first item instead of including unrelated wallpapers.
- The toggle requires the Wallflow process to remain running; it does not register automatic startup.
- Popup transparency can vary with Windows compositor, GPU driver and WebView2.

## Roadmap

- [ ] Test on Windows 11 against real Wallpaper Engine library; fix any integration quirks
- [x] Persistent preview thumbnail cache + virtualized rendering
- [ ] Saved wallpaper-specific presets and favorites
- [x] Detect the focused monitor and its currently selected wallpaper
- [x] Tray icon with open and quit actions
- [x] Settings popup for current playlists and persistent, conflict-checked hotkeys
- [ ] Start-with-Windows **opt-in**
- [ ] Custom animations, adjustable tilt, themes, blur and glow
- [ ] Keyboard/controller accessibility and performance benchmark
- [ ] Signed release builds after security review

## Key files

- `src/App.svelte` - main experience
- `src/lib/WallpaperCard.svelte` - image-only animated cards styled with Tailwind
- Adjust `CARD_GAP_PX` near the top of `src/App.svelte` to change spacing: `0` touches, positive values add space, negative values overlap. The default is `8` CSS pixels.
- `src/app.css` - Tailwind import and transparent document base styles
- `src-tauri/src/wallpaper.rs` - Steam scan, preview, Wallpaper Engine CLI
- `src-tauri/capabilities/default.json` - Tauri capabilities
- `src/lib/previews.js` - bounded preview queue and memory cache
- `src-tauri/src/engine_config.rs` - named playlists and Wallpaper Engine display mapping
- `src-tauri/src/engine_query.rs` - invisible console capture for the live wallpaper query
- `src-tauri/src/covers.rs` - bounded read-only PKG/TEX artwork decoding; format reference: [RePKG](https://github.com/notscuffed/repkg)
- `src-tauri/src/focus.rs` - previous app focus and monitor detection
- `src-tauri/src/lifecycle.rs` - animation lifecycle and stale-close protection

License: MIT (see LICENSE).
