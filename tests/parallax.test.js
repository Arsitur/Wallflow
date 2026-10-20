import test from 'node:test';
import assert from 'node:assert/strict';
import { imagePanPosition } from '../src/lib/parallax.js';

test('centered card shows the center of its image', () => {
  assert.equal(imagePanPosition(0, 1360), 50);
});

test('left and right cards reveal corresponding image edges', () => {
  assert.equal(imagePanPosition(-20, 1360), 0);
  assert.equal(imagePanPosition(20, 1360), 100);
});

test('panning is continuous and monotonic through the center', () => {
  const steps = [-3, -2, -1, -0.5, 0, 0.5, 1, 2, 3]
    .map(offset => imagePanPosition(offset, 1360));
  for (let i = 1; i < steps.length; i++) {
    assert.ok(steps[i] > steps[i - 1], `not increasing at step ${i}`);
  }
});

test('the movement is symmetric and does not overshoot at different viewport sizes', () => {
  for (const width of [320, 800, 1360, 2560]) {
    for (const offset of [-100, -5, -2, -0.2, 0, 0.2, 2, 5, 100]) {
      const pan = imagePanPosition(offset, width);
      assert.ok(pan >= 0 && pan <= 100);
      assert.ok(Math.abs(pan + imagePanPosition(-offset, width) - 100) < 1e-8);
    }
  }
});
