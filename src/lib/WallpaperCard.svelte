<script>
  import { onDestroy } from "svelte";
  import { coverLoader } from "./previews.js";
  export let wallpaper;
  export let offset = 0;
  export let active = false;
  export let motion = 0;
  export let gap = 8;
  export let onApply;
  let thumbnail = null;
  let cover = null;
  let failed = false;
  let coverReady = false;
  let thumbnailFailed = false;
  let coverImageFailed = false;
  let cancelCoverRequest = () => {};
  let coverKey = "";
  function fetchPreview(item) {
    const key = JSON.stringify([item.id, item.preview_revision]);
    if (key === coverKey) return;
    cancelCoverRequest();
    coverKey = key;
    thumbnail = cover = null;
    coverReady = thumbnailFailed = coverImageFailed = failed = false;
    if (item.preview_available) {
      cancelCoverRequest = coverLoader.request(item.id, Math.abs(offset), item.preview_revision, (state) => {
        thumbnail = state.thumbnail;
        cover = state.cover;
        failed = state.failed;
      });
    }
  }
  async function coverLoaded(event) {
    const image = event.currentTarget;
    const key = coverKey;
    try {
      await image.decode();
      if (key === coverKey && image.isConnected) coverReady = true;
    } catch {
      if (key === coverKey && image.isConnected) coverImageFailed = true;
    }
  }
  $: fetchPreview(wallpaper);
  $: coverLoader.prioritize(
    wallpaper.id,
    Math.abs(offset),
    wallpaper.preview_revision,
  );
  onDestroy(() => cancelCoverRequest());
  $: coverLoading = wallpaper.preview_available && !coverReady && !failed && !coverImageFailed;
  $: abs = Math.abs(offset);
  $: scale = Math.max(0.22, Math.pow(0.86, abs));
  // Sum half-widths of adjacent scaled cards, then add an explicit pixel gap.
  $: translate =
    ((Math.sign(offset) * (1 - Math.pow(0.86, abs))) / 0.14) * 0.93;
  $: gapOffset = Math.sign(offset) * abs * gap;
  $: opacity = Math.min(1, Math.max(0, 12 - abs));
  function clickCard() {
    onApply(wallpaper);
  }
</script>

<button
  class="group absolute left-1/2 top-1/2 h-[min(84vh,620px)] w-[clamp(160px,12vw,280px)] cursor-pointer rounded-none border-0 bg-transparent p-0 outline-none will-change-transform"
  style={`transform:translate(-50%,-50%) translateX(calc(${translate} * clamp(160px,12vw,280px) + ${gapOffset}px)) scale(${scale}) skewX(${-motion * 2}deg) scale(${1 - Math.abs(motion) * 0.035},${1 + Math.abs(motion) * 0.012});z-index:${Math.round(100 - abs * 5)};opacity:${opacity};pointer-events:${opacity > 0 ? "auto" : "none"}`}
  on:click={clickCard}
  aria-label={`${wallpaper.title}, apply wallpaper${active ? ", selected" : ""}`}
  aria-current={active ? "true" : undefined}
  tabindex={active ? 0 : -1}
>
  <div
    class={`relative h-full w-full overflow-hidden border bg-[#1a222a] transition-colors duration-150  group-focus-visible:outline-4 group-focus-visible:-outline-offset-6  group-focus-visible:outline-white ${active ? "border-white/100 outline-[1.5px]" : "border-white/20 hover:border-white/25 hover:outline-[1.5px]"}`}
  >
    {#key coverKey}
    {#if thumbnail && !thumbnailFailed}
      <img
        class={`pointer-events-none absolute inset-0 h-full w-full object-cover ${!failed && !coverImageFailed ? "scale-110 blur-md" : ""}`}
        alt=""
        src={thumbnail}
        draggable="false"
        decoding="async"
        on:error={() => (thumbnailFailed = true)}
      />
    {/if}
    {#if cover && !coverImageFailed}
      <img
        class={`pointer-events-none absolute inset-0 h-full w-full object-cover transition-opacity duration-150 motion-reduce:transition-none ${coverReady ? "opacity-100" : "opacity-0"}`}
        alt=""
        src={cover}
        draggable="false"
        decoding="async"
        on:load={coverLoaded}
        on:error={() => (coverImageFailed = true)}
      />
    {/if}
    {#if !coverReady && (!thumbnail || thumbnailFailed)}
      <div
        class="grid h-full place-items-center text-white/40"
        aria-hidden="true"
      >
        {failed || coverImageFailed || !wallpaper.preview_available ? "◇" : ""}
      </div>
    {/if}
    {#if coverLoading}
      <div
        class="pointer-events-none absolute inset-0 grid place-items-center"
        role="status"
        aria-label="Loading high-resolution cover"
      >
        <span
          class="size-6 animate-spin rounded-full border-2 border-white/20 border-t-white/90 motion-reduce:animate-none"
          aria-hidden="true"
        ></span>
      </div>
    {/if}
    {/key}
  </div>
</button>
