# Developing Wallflow

[Back to the README](../README.md)

Build, release and implementation notes for anyone who wants to explore or fork the project. For installation and the author's tested setup, use the main README.

## Develop from source (PowerShell)

Development requires Node.js 22+, npm, a current stable Rust/MSVC toolchain, Microsoft C++ Build Tools and WebView2. See [Tauri's Windows prerequisites](https://v2.tauri.app/start/prerequisites/). These tools are only needed by developers and release maintainers.

```powershell
# Clone the repo or extract its source ZIP, then open PowerShell there
npm ci
rustup update stable
npm run tauri dev
```

Starting the executable, launching it at sign-in, or reloading the frontend never opens the selector automatically. A second launch silently exits instead of creating another tray icon or competing for the shortcut. Press `Ctrl + Alt + G` from anywhere to open it; a shortcut pressed while the frontend is starting is queued. Click the Wallflow icon in the notification area to reopen the gallery. Right-click it for **Settings**, **Logs / Debugger**, or **Quit Wallflow**. Settings opens in its own ordinary window without opening the selector. Save, Cancel, Esc or the native close button closes only Settings; losing focus does not dismiss it. Playlist synchronization uses the display containing the Settings window. Esc and Alt + F4 hide the gallery and restore the previous foreground window. Switching focus to another application dismisses the gallery without changing the new app focus.

Selecting a wallpaper dismisses the gallery immediately with its normal 180 ms exit animation. Wallpaper loading and the audio startup guard continue in the background. Completing that operation never closes a gallery you reopened meanwhile. Failed changes are logged and shown the next time you open the selector, without automatically opening a window.

The debugger is a separate window with search, severity filters, and auto-scroll. It collects native lifecycle, monitor mapping, scans, preview fallbacks, Wallpaper Engine commands, settings changes, frontend console output, and uncaught errors. It retains the latest 2,000 events in memory and writes local JSONL logs, rotating at 2 MB with one previous file. The debugger displays the file path. Logs include local wallpaper paths and property values; review them before sharing.

For frontend development (the browser shows an empty state; installed wallpapers require Tauri):

```powershell
npm install
npm run dev
```

Then visit http://127.0.0.1:1420. Demo wallpapers and the standalone concept preview have been removed.

## Build an installer

```powershell
npm run release:check
npm run tauri build -- --bundles nsis
```

The installer is generated in `src-tauri/target/release/bundle/nsis/`. It uses a per-user install, English UI and automatic WebView2 setup when necessary. Building requires internet access for uncached build/installer dependencies.

## Publish a Windows release (maintainers)

The **Windows installer** GitHub Actions workflow builds and tests on Windows. **Run workflow** builds an installer artifact without publishing a release. Pushing a version tag such as `v0.1.0` builds the installer and attaches it to a **draft** GitHub Release.

1. Keep the version in `package.json`, `package-lock.json`, `src-tauri/Cargo.toml`, `src-tauri/Cargo.lock` and `src-tauri/tauri.conf.json` consistent. Run `npm run release:check`.
2. Commit and push the source, then create and push the matching `v<version>` tag. For the current version:

   ```powershell
   git tag v0.1.0
   git push origin v0.1.0
   ```

3. Wait for **Windows installer** in Actions. Test its `-setup.exe` on Windows with Wallpaper Engine installed.
4. Open the draft under Releases, edit the release notes, and choose **Publish release**. Normal users can now download the installer from the link above.

No external signing secrets are required for unsigned builds; the workflow uses GitHub's built-in token to attach the installer. Code signing is a separate future step to improve the Windows trust prompt. No releases are automatically published and no updater is installed.

## Following a wallpaper selection into Rust

`invoke` comes from Tauri's `@tauri-apps/api/core`, which the Svelte frontend imports. It sends a command to the native Rust side. See [Tauri's command guide](https://v2.tauri.app/develop/calling-rust/).

For one selection, the path is:

```text
src/App.svelte: apply(wallpaper)
  → src/lib/selection.js: applySelection(...)
      → invoke("hide_gallery")          # begin the exit animation now
      → invoke("apply_wallpaper", { id, monitor })
  → src-tauri/src/wallpaper.rs: apply_wallpaper(...)
      → capture the display and run the work on a blocking worker
      → apply(...)
          → validate the wallpaper and display, read saved settings
          → audio_transition::protect(...)
              → open_playlist(...)      # only if synchronization is needed
              → run_display_control(..., ["-control", "openWallpaper", "-file", file])
                  → display_args(...)   # append "-location", "MonitorN"
                  → run_control(...)
                      → engine_command(executable).args(args).spawn()
              → restore_properties(...) # apply saved per-wallpaper settings
```

`engine_command` creates a `std::process::Command` for the installed Wallpaper Engine executable. `args` supplies separate arguments; `spawn` is the point where Windows actually launches the CLI process. `run_control` then checks its exit code and timeout. The command reaches the already-running Wallpaper Engine renderer.

The functions separate the frontend command, validation, temporary audio protection, monitor arguments, and process execution. You can read them in the order above instead of reading the whole Rust file from top to bottom.

The audio guard continues after the selector's exit animation. Finishing a change never hides a selector the user has reopened. Errors are logged and retained for the next explicit opening.

## How it works

1. `src-tauri/src/wallpaper.rs` locates Steam from `HKCU\Software\Valve\Steam`, the default install locations, and `steamapps/libraryfolders.vdf`.
2. It reads `steamapps/workshop/content/431960/*/project.json` and local project metadata without altering files.
3. The JavaScript/Svelte frontend uses separate priority queues for quick 224 × 496 author previews and 560 × 1240 source covers, with two requests per queue and nearest cards first. The preview stays blurred under a spinner until the high-resolution cover finishes decoding, then the source image fades in. If source loading fails, the preview remains visible without a spinner. Cached source covers reopen directly. For scenes, Rust reads original artwork from the installed project or PKGV archive and decodes TEXV textures (RGBA, BC1/BC2/BC3, embedded images and LZ4 blocks). Candidate artwork is compared with the author's preview to avoid isolated layers and masks. Only the portrait crop is resized. Images use their original file; videos use a frame from an existing FFmpeg installation. Unsupported scenes fall back to the author's preview. Source path, size and modification time invalidate cached images. Wallpaper scripts and effects are not executed for covers.
4. Every display uses the same named playlist membership, including unsaved additions to that playlist on any display. Each opening resolves unfamiliar playlist entries from their installed project metadata, so wallpapers installed after Wallflow started appear without a full library rescan. Each opening queries Wallpaper Engine through an invisible Windows pseudoconsole and a fixed system-console bridge (its CLI attaches to its parent's console), with saved configuration as fallback. The current wallpaper is centered on opening only when it belongs to the selected playlist. Right-click applies the wallpaper currently centered in the carousel, including after scrolling or dragging.
5. On Apply, the backend invokes the **installed** `wallpaper64.exe` or `wallpaper32.exe` directly with:

```text
-control openWallpaper -file <validated-wallpaper-path> -location Monitor0
```

Apply and playback use `std::process::Command` with separated arguments. Display commands and current-wallpaper queries use Wallpaper Engine's persistent location identifier (`Monitor0`, `Monitor1`, etc.), matched from its monitor map. In Clone/Stretch mode, commands, active-wallpaper lookup, properties and playlist synchronization all target the shared `Monitor0` renderer. Dormant secondary entries in config.json are not independent wallpaper targets in these modes. The read-only query uses the Windows system `cmd.exe` as an invisible console owner with a fixed command, a checked executable name and a location built from a numeric ID; user text and wallpaper paths never become shell commands. Failed commands report the operation, location and exit code. Official CLI documentation: https://help.wallpaperengine.io/en/functionality/cli.html

Scene projects launch their validated `file` target using an ordinary Windows path so it matches Wallpaper Engine's saved settings identity. Saved `wproperties` for the exact file and monitor are restored using `applyProperties`. Playlist changes use `openPlaylist`, and applying an item checks that the selected playlist is active for that monitor first. Selecting **all installed wallpapers** only changes the library view; it does not replace Wallpaper Engine's playlist. A named playlist must be saved in Wallpaper Engine before it can be synchronized.

If a wallpaper's media filename contains Unicode characters, Wallflow launches through its installed ASCII-path `project.json` when available. Wallpaper Engine then reads the original filename from UTF-8 metadata, avoiding the CLI replacing filename characters with `?`. Saved properties still use the original media file identity. Wallpaper Engine's files are not renamed or modified.

## Security model

- No external assets, cloud access, telemetry, auto-updates or administrator rights during normal use. Sign-in launch is opt-in and uses only Wallflow's own value in the current user's Windows Run registry key; disabling it or uninstalling removes that value.
- No third-party executable bundled in this source ZIP.
- Native access uses local wallpaper assets, Wallpaper Engine's configuration, Wallflow settings/cache/log files, and your installed Wallpaper Engine executable. Wallflow does not edit Wallpaper Engine's config.json.
- Wallpaper project `file` paths are canonicalized and checked to remain inside their project directory.
- Arbitrary user-entered executable paths are **not** accepted for wallpaper application.
- Browser preview cannot access the filesystem or Wallpaper Engine.
- NPM and Cargo fetch ordinary **development dependencies** when you build; inspect dependency changes before distributing binaries.

## Limitations in v0.1

- Detects wallpapers only in discoverable Steam libraries and expected project folders, not every manually imported video/image.
- Original source covers are generated once in the background, then reused from disk and memory. Scene covers show static source artwork, so shader effects and the final arrangement of complex animated layers can differ. Unsupported texture formats fall back to the author's preview. FFmpeg is optional for video frames; the app never installs it automatically. Installation dates use local folder creation times, with modification time as a fallback; copied/restored libraries may have different dates.
- Restores saved per-wallpaper properties for the target monitor, including alignment, zoom, volume and user properties. Changes not yet flushed to Wallpaper Engine's config.json may not be available. Selecting named presets separately is not implemented.
- Wallpaper Engine's documented CLI has no playback/mute state query. Preserving playback means leaving its runtime pause/mute controls alone; Wallflow does not infer state from performance rules. Disabling this setting explicitly sends Play and Unmute after a wallpaper change.
- With preservation enabled, Wallflow temporarily mutes only audio-rendering sessions owned by executables in the installed Wallpaper Engine folder, on all active output devices. Session-created notifications also cover new video streams. Original Windows mixer mute states are restored after commands and a 500 ms startup window (extended to at most 1.5 s if streams keep appearing), including on command failures. The guard covers playlist synchronization too; Quit waits for restoration. It never changes system master volume or other applications. This can briefly interrupt audible wallpapers on other displays during a switch. If audio protection cannot initialize, the change fails before sending a wallpaper command. Forced termination during a switch can leave a session muted in the Windows mixer.
- No click-through desktop effect, audio previews or file drag/drop yet. If the active wallpaper is outside the selected playlist, the carousel starts at its first item instead of including unrelated wallpapers.
- The toggle requires the Wallflow process to remain running. Optional sign-in launch is available in Settings. When using the standalone executable, keep it in a permanent location before enabling this option; moving it requires saving the option again from the new location.
- Popup transparency can vary with Windows compositor, GPU driver and WebView2.

## Key files

- `src/App.svelte` - wallpaper selector
- `src/Settings.svelte` - standalone settings window and controls reference
- `src/lib/WallpaperCard.svelte` - image-only animated cards styled with Tailwind
- Adjust `CARD_GAP_PX` near the top of `src/App.svelte` to change spacing: `0` touches, positive values add space, negative values overlap. The default is `4` CSS pixels.
- `src/app.css` - Tailwind import and transparent document base styles
- `src-tauri/src/wallpaper.rs` - Steam scan, preview, Wallpaper Engine CLI
- `src-tauri/capabilities/default.json` - Tauri capabilities
- `src/lib/previews.js` - bounded preview queue and memory cache
- `src-tauri/src/engine_config.rs` - named playlists and Wallpaper Engine display mapping
- `src-tauri/src/engine_query.rs` - invisible console capture for the live wallpaper query
- `src-tauri/src/covers.rs` - bounded read-only PKG/TEX artwork decoding; format reference: [RePKG](https://github.com/notscuffed/repkg)
- `src-tauri/src/focus.rs` - previous app focus and monitor detection
- `src-tauri/src/lifecycle.rs` - animation lifecycle and stale-close protection
- `src-tauri/src/startup.rs` - per-user sign-in launch and single-instance protection
- `.github/workflows/windows-release.yml` - tested Windows installer builds and draft releases
- `src-tauri/src/diagnostics.rs` - bounded native/frontend logging and debugger window
- `src/Logs.svelte` - live searchable debugger
