// Use the carousel's existing smoothed position; no extra timers or image copies.
export function coverPosition(offset, enabled = true) {
  if (!enabled || !Number.isFinite(offset)) return 50;
  const progress = Math.max(-1, Math.min(1, offset / 6));
  return 50 + 50 * Math.sin(progress * Math.PI / 2);
}
