// Match the card's carousel geometry so the crop reaches each image edge
// when a card approaches the left or right edge of the visible gallery.
// This runs on the existing animation frames and reuses the loaded image.
export function imagePanPosition(offset, viewportWidth, gap = 4) {
  const viewport = Math.max(320, Number.isFinite(viewportWidth) ? viewportWidth : 1360);
  const cardWidth = Math.max(160, Math.min(280, viewport * 0.12));
  const distance = Number.isFinite(offset) ? offset : 0;
  const abs = Math.abs(distance);
  const direction = Math.sign(distance);
  const translated = direction * (
    ((1 - Math.pow(0.86, abs)) / 0.14) * 0.93 * cardWidth + abs * gap
  );
  const usableHalfWidth = Math.max(1, (viewport - cardWidth) / 2);
  const fraction = Math.max(-1, Math.min(1, translated / usableHalfWidth));
  return 50 + fraction * 50;
}
