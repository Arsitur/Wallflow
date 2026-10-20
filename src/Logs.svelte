<script>
  import { onMount, tick } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  let entries = [];
  let filter = "";
  let level = "all";
  let follow = true;
  let path = "";
  let error = "";
  let output;
  let after = 0;
  $: visible = entries.filter((entry) => (level === "all" || entry.level === level) && `${entry.source} ${entry.message}`.toLowerCase().includes(filter.toLowerCase()));
  onMount(() => {
    let disposed = false;
    let timer;
    invoke("log_path").then((value) => path = value || "Log file unavailable").catch((reason) => error = String(reason));
    const poll = async () => {
      try {
        const incoming = await invoke("get_logs", { after });
        if (disposed) return;
        if (incoming.length) {
          after = incoming[incoming.length - 1].id;
          entries = [...entries, ...incoming].slice(-2000);
          await tick();
          if (follow && output) output.scrollTop = output.scrollHeight;
        }
        error = "";
      } catch (reason) { error = String(reason); }
      if (!disposed) timer = setTimeout(poll, 1000);
    };
    void poll();
    return () => { disposed = true; clearTimeout(timer); };
  });
</script>

<main class="flex h-full flex-col gap-4 bg-zinc-950 p-5 text-sm text-zinc-100">
  <div class="flex items-center justify-between gap-4">
    <div><h1 class="text-xl font-semibold">Logs / Debugger</h1><p class="mt-1 text-xs text-zinc-400">Monitors, playlists, Wallpaper Engine commands, previews and errors.</p></div>
    <span class="text-xs text-zinc-400">{entries.length} / 2000 events</span>
  </div>
  <div class="flex flex-wrap items-center gap-3">
    <input aria-label="Search logs" placeholder="Search logs…" bind:value={filter} class="min-w-40 flex-1 rounded-lg border border-white/15 bg-zinc-900 px-3 py-2" />
    <select aria-label="Log level" bind:value={level} class="rounded-lg border border-white/15 bg-zinc-900 px-3 py-2"><option value="all">All levels</option>{#each ["debug", "info", "warn", "error", "log"] as name}<option value={name}>{name}</option>{/each}</select>
    <label class="flex items-center gap-2"><input type="checkbox" bind:checked={follow} /> Auto-scroll</label>
    <button class="rounded-lg border border-white/15 px-3 py-2" on:click={() => entries = []}>Clear view</button>
  </div>
  {#if error}<p role="alert" class="text-red-300">{error}</p>{/if}
  <div bind:this={output} class="min-h-0 flex-1 select-text overflow-auto rounded-lg border border-white/10 bg-black/40 p-3 font-mono text-xs" role="log" aria-label="Wallflow events">
    {#each visible as entry (entry.id)}
      <div class="mb-2 break-words whitespace-pre-wrap leading-5"><span class="text-zinc-500">{new Date(entry.time).toLocaleTimeString()} </span><span class:text-red-300={entry.level === "error"} class:text-amber-300={entry.level === "warn"} class:text-sky-300={entry.level === "info"}>[{entry.level}]</span> <span class="text-zinc-400">{entry.source}</span> {entry.message}</div>
    {/each}
    {#if !visible.length}<p class="text-zinc-500">No events match the selected filters.</p>{/if}
  </div>
  <p class="select-text break-all text-xs text-zinc-500">Local file: {path}<br />Logs rotate at 2 MB. The previous file is kept in the same folder.</p>
</main>
