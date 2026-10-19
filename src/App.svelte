<script>
  import { onMount, tick } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import WallpaperCard from "./lib/WallpaperCard.svelte";

  // Gap in CSS pixels between adjacent cards. Smaller = closer; negative values overlap.
  const CARD_GAP_PX = 4;

  let wallpapers = [];
  let native = false;
  let loading = false;
  let switching = false;
  let notify = "";
  let target = 0;
  let position = 0;
  let scrollMotion = 0;
  let width = 1360;
  let dragging = false;
  let dragged = false;
  let pointerStart = 0;
  let dragStart = 0;
  let pointerId = null;
  let frame = 0;
  let toastTimer;
  let wheelTimer;
  let reducedMotion = false;
  let galleryEpoch = 0;
  let phase = "closed";
  let monitorIndex = 0;
  let playlistName = null;
  let closeTimer;
  let revealFrame;
  let revealFrameNext;
  let settingsDialog;
  let showSettings = false;
  let settingsBusy = false;
  let settingsError = "";
  let playlists = [];
  let draftPlaylist = "";
  let draftShortcut = "";

  async function openSettings() {
    if (!native || settingsBusy) return;
    pointerUp();
    dragged = false;
    settingsError = "";
    settingsBusy = true;
    const epoch = galleryEpoch;
    try {
      const info = await invoke("get_settings");
      if (epoch !== galleryEpoch) return;
      playlists = info.playlists;
      draftPlaylist = info.playlist || "";
      draftShortcut = info.shortcut;
      showSettings = true;
      await tick();
      if (!settingsDialog.open) settingsDialog.showModal();
    } catch (error) {
      toast(String(error));
    } finally {
      settingsBusy = false;
    }
  }
  function closeSettings() {
    showSettings = false;
    settingsDialog?.close();
  }
  async function saveSettings() {
    settingsBusy = true;
    settingsError = "";
    try {
      await invoke("save_settings", {
        playlist: draftPlaylist || null,
        shortcut: draftShortcut,
      });
      if ((draftPlaylist || null) !== playlistName) {
        const snapshot = await invoke("gallery_snapshot");
        wallpapers = snapshot.items;
        playlistName = snapshot.playlist;
        monitorIndex = snapshot.monitor;
        target = Math.max(
          0,
          wallpapers.findIndex((item) => item.id === snapshot.active_id),
        );
        position = target;
      }
      closeSettings();
    } catch (error) {
      settingsError = String(error);
    } finally {
      settingsBusy = false;
    }
  }

  async function openGallery(epoch) {
    galleryEpoch = epoch;
    clearTimeout(closeTimer);
    clearTimeout(wheelTimer);
    cancelAnimationFrame(revealFrame);
    cancelAnimationFrame(revealFrameNext);
    pointerId = null;
    dragging = dragged = false;
    scrollMotion = 0;
    phase = "closed";
    loading = true;
    try {
      const snapshot = await invoke("gallery_snapshot");
      if (epoch !== galleryEpoch) return;
      wallpapers = snapshot.items;
      playlistName = snapshot.playlist;
      monitorIndex = snapshot.monitor;
      // Every opening starts at the actual wallpaper, never the previous scroll position.
      target = Math.max(
        0,
        wallpapers.findIndex((item) => item.id === snapshot.active_id),
      );
      position = target;
    } catch (error) {
      if (epoch !== galleryEpoch) return;
      wallpapers = [];
      toast(String(error));
    } finally {
      if (epoch === galleryEpoch) {
        loading = false;
        await tick();
        // Native positioning and WebView layout complete while the window is hidden.
        await new Promise((resolve) => setTimeout(resolve, 40));
        if (
          epoch !== galleryEpoch ||
          !(await invoke("present_gallery", { epoch }))
        )
          return;
        revealFrame = requestAnimationFrame(() => {
          revealFrameNext = requestAnimationFrame(() => {
            if (epoch === galleryEpoch) phase = "open";
          });
        });
      }
    }
  }
  function closeGallery(epoch) {
    closeSettings();
    galleryEpoch = epoch;
    phase = "closed";
    loading = false;
    clearTimeout(closeTimer);
    cancelAnimationFrame(revealFrame);
    cancelAnimationFrame(revealFrameNext);
    closeTimer = setTimeout(
      () =>
        invoke("finish_hide", { epoch }).catch((error) => toast(String(error))),
      reducedMotion ? 0 : 180,
    );
  }
  $: selectedIndex = Math.round(target);
  $: current = wallpapers[selectedIndex];
  $: firstVisible = Math.max(0, Math.floor(position) - 12);
  $: visible = wallpapers.slice(firstVisible, Math.ceil(position) + 13);
  function clamp(value) {
    return Math.max(0, Math.min(wallpapers.length - 1, value));
  }
  function toast(message) {
    notify = message;
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => (notify = ""), 3800);
  }
  function wheel(event) {
    if (showSettings) return;
    event.preventDefault();
    if (dragging || loading || phase !== "open") return;
    const delta =
      Math.abs(event.deltaX) > Math.abs(event.deltaY)
        ? event.deltaX
        : event.deltaY;
    const unit = event.deltaMode === 1 ? 16 : event.deltaMode === 2 ? width : 1;
    target = clamp(target + Math.max(-2, Math.min(2, (delta * unit) / 150)));
    clearTimeout(wheelTimer);
    wheelTimer = setTimeout(() => (target = Math.round(target)), 160);
  }
  function pointerDown(event) {
    if (
      showSettings ||
      (event.target instanceof Element &&
        event.target.closest("[data-settings]"))
    )
      return;
    if (event.button !== 0 || phase !== "open") return;
    clearTimeout(wheelTimer);
    pointerId = event.pointerId;
    pointerStart = event.clientX;
    dragStart = target;
    dragged = false;
  }
  function pointerMove(event) {
    if (event.pointerId !== pointerId) return;
    const distance = event.clientX - pointerStart;
    if (Math.abs(distance) > 5) {
      dragging = true;
      dragged = true;
    }
    if (dragging)
      target = clamp(dragStart - distance / Math.max(65, width * 0.075));
  }
  function pointerUp() {
    if (pointerId === null) return;
    pointerId = null;
    if (dragging) target = Math.round(target);
    dragging = false;
  }
  function cardClick(event) {
    if (dragged) {
      event.preventDefault();
      event.stopPropagation();
      dragged = false;
    }
  }
  async function hide() {
    if (native) await invoke("hide_gallery");
  }
  async function refresh() {
    if (!native || loading) return;
    loading = true;
    try {
      await invoke("list_wallpapers");
      await invoke("show_gallery");
    } catch (error) {
      toast(`Scan error: ${String(error)}`);
    } finally {
      loading = false;
    }
  }
  async function apply(wallpaper) {
    if (dragged || switching || loading || phase !== "open") return;
    if (!native) {
      toast("Preview only. Open the desktop app to apply wallpapers.");
      return;
    }
    switching = true;
    try {
      await invoke("apply_wallpaper", {
        id: wallpaper.id,
        monitor: monitorIndex,
      });
      await hide();
      await invoke("resume_wallpapers");
    } catch (error) {
      toast(`Wallpaper Engine: ${String(error)}`);
    } finally {
      switching = false;
    }
  }
  function handleKey(event) {
    if (showSettings) return;
    if (event.key === "Escape") {
      event.preventDefault();
      void hide();
    }
    if (loading || phase !== "open") return;
    if (event.key === "ArrowLeft" || event.key === "a") {
      event.preventDefault();
      target = clamp(Math.round(target) - 1);
    }
    if (event.key === "ArrowRight" || event.key === "d") {
      event.preventDefault();
      target = clamp(Math.round(target) + 1);
    }
    if (event.key === "Enter" || event.key === " ") {
      event.preventDefault();
      if (current) void apply(current);
    }
    if (event.key.toLowerCase() === "r" && !event.ctrlKey && !event.metaKey)
      void refresh();
  }
  onMount(() => {
    native = "__TAURI_INTERNALS__" in window;
    const motion = window.matchMedia("(prefers-reduced-motion: reduce)");
    const updateMotion = () => (reducedMotion = motion.matches);
    updateMotion();
    motion.addEventListener("change", updateMotion);
    let last = performance.now();
    const animate = (now) => {
      const dt = Math.min(now - last, 50);
      last = now;
      const previous = position;
      position +=
        (target - position) * (reducedMotion ? 1 : 1 - Math.exp(-dt / 85));
      if (Math.abs(position - target) < 0.001) position = target;
      const velocity = Math.max(
        -1,
        Math.min(1, ((position - previous) * 80) / Math.max(1, dt)),
      );
      scrollMotion =
        reducedMotion || phase !== "open"
          ? 0
          : scrollMotion + (velocity - scrollMotion) * (1 - Math.exp(-dt / 90));
      if (Math.abs(scrollMotion) < 0.001) scrollMotion = 0;
      frame = requestAnimationFrame(animate);
    };
    frame = requestAnimationFrame(animate);
    let disposed = false;
    const unlisteners = [];
    const listen = (promise) =>
      promise
        .then((unlisten) => {
          if (disposed) unlisten();
          else unlisteners.push(unlisten);
        })
        .catch((error) => toast(String(error)));
    if (native) {
      const win = getCurrentWindow();
      Promise.all([
        listen(
          win.listen("gallery-opening", ({ payload }) => {
            void openGallery(payload);
          }),
        ),
        listen(
          win.listen("gallery-closing", ({ payload }) => closeGallery(payload)),
        ),
      ])
        .then(() => {
          if (!disposed) return invoke("gallery_ready");
        })
        .catch((error) => toast(String(error)));
    } else {
      phase = "open";
    }
    return () => {
      disposed = true;
      cancelAnimationFrame(frame);
      clearTimeout(toastTimer);
      clearTimeout(wheelTimer);
      clearTimeout(closeTimer);
      cancelAnimationFrame(revealFrame);
      cancelAnimationFrame(revealFrameNext);
      galleryEpoch++;
      unlisteners.forEach((unlisten) => unlisten());
      motion.removeEventListener("change", updateMotion);
    };
  });
</script>

<svelte:window
  bind:innerWidth={width}
  on:keydown={handleKey}
  on:pointerdown={pointerDown}
  on:pointermove={pointerMove}
  on:pointerup={pointerUp}
  on:pointercancel={pointerUp}
  on:blur={pointerUp}
  on:click|capture={cardClick}
/>
<main
  class={`group h-full w-full touch-none select-none overflow-hidden bg-transparent transition-[opacity,scale,translate] duration-180 ease-out motion-reduce:transition-none ${phase === "open" ? "translate-y-0 scale-100 opacity-100" : "pointer-events-none translate-y-2 scale-95 opacity-0"} ${dragging ? "cursor-grabbing [&_button]:cursor-grabbing" : ""}`}
  on:wheel|nonpassive={wheel}
  on:contextmenu|preventDefault={openSettings}
>
  <section
    class="relative h-full w-full"
    aria-label={playlistName
      ? `Wallpaper carousel: ${playlistName}`
      : "Wallpaper carousel"}
    aria-busy={loading || switching}
  >
    {#each visible as wallpaper, index (wallpaper.id)}
      <WallpaperCard
        {wallpaper}
        gap={CARD_GAP_PX}
        motion={scrollMotion}
        offset={firstVisible + index - position}
        active={firstVisible + index === selectedIndex}
        onApply={apply}
      />
    {/each}
    {#if !wallpapers.length}
      <div
        class="absolute inset-0 flex flex-col items-center justify-center gap-3 text-sm text-white drop-shadow-md"
        role="status"
      >
        <p>
          {loading
            ? "Loading wallpapers…"
            : native
              ? playlistName
                ? `No installed wallpapers in ${playlistName}. Right-click to choose another playlist.`
                : "No installed wallpapers found."
              : "Open the desktop app to view your installed wallpapers."}
        </p>
        {#if native && !loading}<button
            class="cursor-pointer rounded border border-white/25 bg-zinc-900/80 px-4 py-2"
            on:click={refresh}>Rescan library</button
          >{/if}
      </div>
    {/if}
  </section>
  {#if notify}<div
      class="absolute bottom-6 left-1/2 z-150 max-w-4/5 -translate-x-1/2 rounded-md bg-zinc-900/90 px-4 py-2.5 text-xs text-white"
      role="status"
    >
      {notify}
    </div>{/if}
</main>

<dialog
  bind:this={settingsDialog}
  on:cancel|preventDefault={closeSettings}
  on:close={() => (showSettings = false)}
  class="fixed inset-0 m-auto w-[min(440px,90vw)] max-h-[85vh] overflow-auto rounded-2xl border border-white/10 bg-zinc-950 p-6 text-sm text-zinc-100 shadow-2xl backdrop:bg-transparent"
>
  <form
    on:submit|preventDefault={saveSettings}
    class="flex flex-col gap-5"
    data-settings
  >
    <div class="flex items-center justify-between">
      <h2 class="text-lg font-semibold">Setări</h2>
      <button
        type="button"
        class="cursor-pointer px-2 py-1 text-zinc-400 hover:text-white"
        aria-label="Close settings"
        on:click={closeSettings}>✕</button
      >
    </div>
    <label class="flex flex-col gap-2"
      >Playlist<select
        bind:value={draftPlaylist}
        disabled={settingsBusy}
        class="rounded-lg border border-white/15 bg-zinc-900 px-3 py-2.5 outline-none focus:border-white/50"
        ><option value="">Toate wallpaper-urile instalate</option
        >{#each playlists as name}<option value={name}>{name}</option
          >{/each}</select
      ></label
    >
    <label class="flex flex-col gap-2"
      >Shortcut<input
        bind:value={draftShortcut}
        disabled={settingsBusy}
        class="rounded-lg border border-white/15 bg-zinc-900 px-3 py-2.5 outline-none focus:border-white/50"
        placeholder="Ctrl+Alt+G"
        autocomplete="off"
        spellcheck="false"
      /></label
    >
    <p class="-mt-3 text-xs leading-5 text-zinc-400">
      Scrie combinația dorită, de exemplu Ctrl+Alt+G sau Ctrl+Shift+Space.
      Setările se păstrează între porniri.
    </p>
    {#if settingsError}<p role="alert" class="text-xs leading-5 text-red-300">
        {settingsError}
      </p>{/if}
    <div class="flex justify-end gap-2">
      <button
        type="button"
        class="cursor-pointer rounded-lg px-4 py-2 text-zinc-400 hover:text-white"
        on:click={closeSettings}>Anulează</button
      ><button
        type="submit"
        disabled={settingsBusy}
        class="cursor-pointer rounded-lg bg-white px-4 py-2 font-medium text-zinc-950 disabled:opacity-50"
        >{settingsBusy ? "Se salvează…" : "Salvează"}</button
      >
    </div>
  </form>
</dialog>
