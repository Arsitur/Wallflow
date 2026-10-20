import test from 'node:test';
import assert from 'node:assert/strict';
import { coverPosition } from '../src/lib/coverMotion.js';

test('covers travel continuously from left through center to right and clamp at the edges', () => {
  assert.equal(coverPosition(-12), 0);
  assert.equal(coverPosition(0), 50);
  assert.equal(coverPosition(12), 100);
  let previous = 0;
  for (let offset = -12; offset <= 12; offset += 0.01) {
    const position = coverPosition(offset);
    assert.ok(position >= previous && position - previous < 0.14);
    assert.ok(Math.abs(position + coverPosition(-offset) - 100) < 1e-10);
    previous = position;
  }
});

test('disabled or invalid motion always retains the centered crop', () => {
  for (const offset of [-12, -1, 0, 1, 12, NaN, Infinity]) {
    assert.equal(coverPosition(offset, false), 50);
  }
  assert.equal(coverPosition(NaN), 50);
});
