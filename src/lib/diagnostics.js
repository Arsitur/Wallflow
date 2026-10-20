import { invoke } from "@tauri-apps/api/core";

export function installDiagnostics() {
  if (!("__TAURI_INTERNALS__" in window)) return;
  let queued = 0;
  const send = (level, source, values) => {
    if (queued >= 30) return;
    queued++;
    const message = values.map((value) => {
      if (value instanceof Error) return value.stack || value.message;
      try { return typeof value === "string" ? value : JSON.stringify(value); }
      catch { return String(value); }
    }).join(" ").slice(0, 6000);
    invoke("frontend_log", { level, source, message }).catch(() => {}).finally(() => queued--);
  };
  for (const level of ["log", "info", "warn", "error", "debug"]) {
    const original = console[level].bind(console);
    console[level] = (...values) => { original(...values); send(level, "frontend", values); };
  }
  window.addEventListener("error", (event) => send("error", "window.error", [event.error || event.message]));
  window.addEventListener("unhandledrejection", (event) => send("error", "unhandledrejection", [event.reason]));
  console.info("Frontend ready", location.search || "gallery");
}
