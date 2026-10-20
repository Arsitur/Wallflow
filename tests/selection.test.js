import test from 'node:test';
import assert from 'node:assert/strict';
import { applySelection } from '../src/lib/selection.js';

test('dismisses before a slow wallpaper command and never hides a reopened gallery on completion', async () => {
  const calls = [];
  let finish;
  let hidden = false;
  const selection = { id: 'workshop:1269334610', monitor: 1 };
  const result = applySelection(async (command, args) => {
    calls.push(command);
    if (command === 'hide_gallery') { hidden = true; return; }
    assert.ok(hidden, 'The gallery must already be dismissing before audio setup starts');
    assert.deepEqual(args, selection);
    return new Promise(resolve => { finish = resolve; });
  }, selection);
  await new Promise(resolve => setImmediate(resolve));
  assert.deepEqual(calls, ['hide_gallery', 'apply_wallpaper']);
  hidden = false; // The user explicitly reopens while the slow command runs.
  finish();
  await result;
  assert.equal(hidden, false);
  assert.equal(calls.filter(command => command === 'hide_gallery').length, 1);
});

test('a command failure is still reported after the gallery has been dismissed', async () => {
  let hidden = false;
  await assert.rejects(applySelection(async command => {
    if (command === 'hide_gallery') { hidden = true; return; }
    throw new Error('Wallpaper Engine unavailable');
  }, { id: 'missing', monitor: 0 }), /Wallpaper Engine unavailable/);
  assert.ok(hidden);
});
