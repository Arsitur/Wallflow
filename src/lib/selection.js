// Dismiss immediately; the command can keep running (and protecting audio)
// after the gallery has finished its exit animation. Never hide on completion:
// the user may have explicitly reopened a new gallery in the meantime.
export async function applySelection(invoke, selection) {
  await invoke('hide_gallery');
  return invoke('apply_wallpaper', selection);
}
