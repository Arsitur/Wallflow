<script>
  import { onMount } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import { captureShortcut, formatShortcut } from "./lib/shortcuts.js";
  let native = false;
  let loaded = false;
  let disposed = false;
  let settingsBusy = true;
  let settingsError = "";
  let playlists = [];
  let draftPlaylist = "";
  let draftShortcut = "";
  let recordingShortcut = false;
  let recordingText = "";
  let shortcutBeforeRecording = "";
  let shortcutError = "";
  let shortcutInput;
  let draftPreservePlayback = true;
  let draftSyncPlaylist = true;
  let draftStartWithWindows = false;
  let draftAmazingAnimations = true;
  let rebuildingCovers = false;
  let coverStatus = "";
  let displayMode = "per-monitor";
  const controls = [
    ["Left click a card", "Apply that wallpaper"],
    ["Right click the selector", "Apply the centered wallpaper"],
    ["Mouse wheel / trackpad", "Browse wallpapers"],
    ["Drag left / right", "Browse wallpapers"],
    ["Left / Right arrow or A / D", "Move one wallpaper"],
    ["Enter / Space", "Apply the centered wallpaper"],
    ["R", "Rescan the installed library"],
    ["Esc", "Close Settings or hide the selector"],
    ["Alt + F4", "Close Settings or hide the selector"],
    ["Switch to another app", "Hide the selector"],
    ["Left click the tray icon", "Open the selector"],
    ["Right click the tray icon", "Open Settings, Logs / Debugger, or Quit"],
  ];

  async function loadSettings() {
    settingsBusy = true;
    settingsError = "";
    try {
      if (!native) throw new Error("Open the desktop app to change settings.");
      const info = await invoke("get_settings");
      if (disposed) return;
      playlists = info.playlists;
      draftPlaylist = info.playlist || "";
      draftShortcut = formatShortcut(info.shortcut);
      draftPreservePlayback = info.preserve_playback;
      draftSyncPlaylist = info.sync_playlist;
      draftStartWithWindows = info.start_with_windows;
      draftAmazingAnimations = info.amazing_animations ?? true;
      displayMode = info.display_mode;
      loaded = true;
    } catch (error) {
      if (!disposed) settingsError = String(error);
    } finally {
      if (!disposed) settingsBusy = false;
    }
  }
  async function closeSettings() {
    if (!native) return;
    try { await getCurrentWindow().close(); }
    catch (error) { settingsError = String(error); }
  }
  async function saveSettings() {
    if (!loaded || settingsBusy || rebuildingCovers) return;
    settingsBusy = true;
    settingsError = "";
    try {
      await invoke("save_settings", {
        playlist: draftPlaylist || null,
        shortcut: draftShortcut,
        preservePlayback: draftPreservePlayback,
        syncPlaylist: draftSyncPlaylist,
        startWithWindows: draftStartWithWindows,
        amazingAnimations: draftAmazingAnimations,
      });
      if (!disposed) await closeSettings();
    } catch (error) {
      console.error("Saving settings failed", error);
      if (!disposed) settingsError = String(error);
    } finally {
      if (!disposed) settingsBusy = false;
    }
  }
  async function rebuildCovers() {
    if (!native || rebuildingCovers || settingsBusy) return;
    rebuildingCovers = true;
    coverStatus = "";
    try {
      await invoke("rebuild_cover_cache");
      coverStatus = "Covers will regenerate the next time you open the selector.";
    } catch (error) { settingsError = String(error); }
    finally { rebuildingCovers = false; }
  }
  function handleKey(event) {
    if (event.key === "Escape" && !event.defaultPrevented) {
      event.preventDefault();
      void closeSettings();
    }
  }
  function stopRecording() {
    recordingShortcut = false;
    shortcutError = "";
    if (native) void invoke("set_shortcut_recording", { recording: false }).catch(console.error);
  }
  function startRecording() {
    if (recordingShortcut) return;
    shortcutBeforeRecording = draftShortcut;
    recordingText = "";
    shortcutError = "";
    recordingShortcut = true;
    if (native) void invoke("set_shortcut_recording", { recording: true }).catch(error => {
      settingsError = String(error);
      shortcutInput?.blur();
    });
  }
  function recordShortcut(event) {
    if (!recordingShortcut) return;
    if (event.key === "Tab" && !event.ctrlKey && !event.altKey && !event.metaKey) return;
    event.preventDefault();
    event.stopPropagation();
    if (event.key === "Escape" && !event.ctrlKey && !event.altKey && !event.shiftKey && !event.metaKey) {
      draftShortcut = shortcutBeforeRecording;
      shortcutInput.blur();
      return;
    }
    const result = captureShortcut(event);
    if (!result) return;
    shortcutError = result.error || "";
    recordingText = result.shortcut || result.preview || "";
    if (result.shortcut) draftShortcut = result.shortcut;
  }
  onMount(() => {
    native = "__TAURI_INTERNALS__" in window;
    let unlisten;
    if (native) void getCurrentWindow().listen("shortcut-recorded", ({ payload }) => {
      if (!recordingShortcut) return;
      draftShortcut = formatShortcut(payload);
      recordingText = draftShortcut;
      shortcutError = "";
    }).then(cleanup => { if (disposed) cleanup(); else unlisten = cleanup; }).catch(console.error);
    void loadSettings();
    return () => { disposed = true; unlisten?.(); };
  });
</script>

<svelte:window on:keydown={handleKey} on:blur={() => shortcutInput?.blur()} />
<main class="h-full overflow-auto bg-zinc-950 p-6 text-sm text-zinc-100">
  <form
    on:submit|preventDefault={saveSettings}
    class="flex flex-col gap-5"
    data-settings
  >
    <h1 class="text-lg font-semibold">Settings</h1>
    {#if !loaded && settingsBusy}<p role="status" class="text-zinc-400">Loading settings…</p>{/if}
    <label class="flex flex-col gap-2"
      >Playlist<select
        bind:value={draftPlaylist}
        disabled={settingsBusy}
        class="rounded-lg border border-white/15 bg-zinc-900 px-3 py-2.5 outline-none focus:border-white/50"
        ><option value="">All installed wallpapers</option
        >{#each playlists as name}<option value={name}>{name}</option
          >{/each}</select
      ></label
    >
    <label class="flex flex-col gap-2"
      >Shortcut<input
        bind:this={shortcutInput}
        value={recordingShortcut ? recordingText : draftShortcut}
        readonly
        on:focus={startRecording}
        on:click={startRecording}
        on:blur={stopRecording}
        on:keydown={recordShortcut}
        aria-describedby="shortcut-hint"
        disabled={settingsBusy}
        class="rounded-lg border border-white/15 bg-zinc-900 px-3 py-2.5 outline-none focus:border-white/50"
        placeholder={recordingShortcut ? "Press a shortcut…" : "Click to record a shortcut"}
        autocomplete="off"
        spellcheck="false"
      /></label
    >
    <p id="shortcut-hint" aria-live="polite" class="-mt-3 text-xs leading-5 text-zinc-400">
      {#if shortcutError}{shortcutError}
      {:else if recordingShortcut}Press a combination with Ctrl, Alt or Super. Esc cancels; Tab finishes recording.
      {:else}Click the field, then press your shortcut. Choose Save to apply it.{/if}
    </p>
    <label class="flex items-start gap-3">
      <input type="checkbox" bind:checked={draftStartWithWindows} disabled={settingsBusy} class="mt-1 accent-white" />
      <span>Start with Windows<span class="mt-1 block text-xs leading-5 text-zinc-400">Start silently in the system tray when you sign in. The selector stays hidden until you use its shortcut or tray controls.</span></span>
    </label>
    <label class="flex items-start gap-3">
      <input type="checkbox" bind:checked={draftPreservePlayback} disabled={settingsBusy} class="mt-1 accent-white" />
      <span>Keep pause and sound settings<span class="mt-1 block text-xs leading-5 text-zinc-400">Paused stays paused. Playing stays playing. Muted stays muted when you switch wallpapers. Turn this off to resume playback and turn sound on for all monitors.</span></span>
    </label>
    <label class="flex items-start gap-3">
      <input type="checkbox" bind:checked={draftSyncPlaylist} disabled={settingsBusy} class="mt-1 accent-white" />
      <span>Sync playlist with Wallpaper Engine<span class="mt-1 block text-xs leading-5 text-zinc-400">Applies to the monitor containing this Settings window. “All installed wallpapers” only changes the library view.</span></span>
    </label>
    <label class="flex items-start gap-3">
      <input type="checkbox" bind:checked={draftAmazingAnimations} disabled={settingsBusy} class="mt-1 accent-white" />
      <span>Amazing animations<span class="mt-1 block text-xs leading-5 text-zinc-400">Smoothly pan across each cover as it moves through the carousel: left edge, center, then right edge. Uses the same images and respects Windows reduced-motion preferences.</span></span>
    </label>
    {#if displayMode === "clone" || displayMode === "stretch"}
      <p class="text-xs leading-5 text-zinc-400">Wallpaper Engine uses {displayMode === "clone" ? "Clone wallpaper" : "Stretch wallpaper"}. Wallpaper and playlist changes apply to all displays. To use separate wallpapers, select “Wallpaper per monitor” in Wallpaper Engine.</p>
    {/if}
    <section aria-labelledby="controls-heading" class="border-t border-white/10 pt-4">
      <div class="mb-5 flex items-center justify-between gap-4">
        <div><h3 class="font-medium">Cover cache</h3><p class="mt-1 text-xs leading-5 text-zinc-400">Regenerate covers from the installed wallpaper sources.</p></div>
        <button type="button" disabled={settingsBusy || rebuildingCovers} class="shrink-0 rounded-lg border border-white/15 px-3 py-2 text-xs disabled:opacity-50" on:click={rebuildCovers}>{rebuildingCovers ? "Rebuilding…" : "Rebuild covers"}</button>
      </div>
      {#if coverStatus}<p role="status" class="mb-4 text-xs leading-5 text-zinc-400">{coverStatus}</p>{/if}
      <h3 id="controls-heading" class="mb-3 font-medium">Controls</h3>
      <dl class="grid grid-cols-[minmax(0,1fr)_minmax(0,1.2fr)] gap-x-4 gap-y-3 text-xs leading-5">
        <dt class="text-zinc-200">{draftShortcut || "Global shortcut"}</dt>
        <dd class="text-zinc-400">Show / hide the selector from any app (configured above)</dd>
        {#each controls as [gesture, action]}
          <dt class="text-zinc-200">{gesture}</dt>
          <dd class="text-zinc-400">{action}</dd>
        {/each}
      </dl>
    </section>
    {#if settingsError}<p role="alert" class="text-xs leading-5 text-red-300">
        {settingsError}
      </p>{/if}
    {#if !loaded && !settingsBusy}<button type="button" class="self-start rounded-lg border border-white/15 px-3 py-2" on:click={loadSettings}>Retry</button>{/if}
    <div class="flex justify-end gap-2">
      <button
        type="button"
        class="cursor-pointer rounded-lg px-4 py-2 text-zinc-400 hover:text-white"
        on:click={closeSettings}>Cancel</button
      ><button
        type="submit"
        disabled={settingsBusy || rebuildingCovers}
        class="cursor-pointer rounded-lg bg-white px-4 py-2 font-medium text-zinc-950 disabled:opacity-50"
        >{settingsBusy ? "Saving…" : "Save"}</button
      >
    </div>
  </form>
</main>
