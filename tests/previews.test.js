import test from 'node:test';
import assert from 'node:assert/strict';
import { createPreviewLoader } from '../src/lib/previews.js';

const nextTurn = () => new Promise(resolve => setImmediate(resolve));

test('loads center previews first, limits parallel requests and shares duplicate requests', async () => {
  const started = [];
  const finish = new Map();
  let active = 0;
  let peak = 0;
  const load = createPreviewLoader(id => {
    started.push(id);
    peak = Math.max(peak, ++active);
    return new Promise(resolve => finish.set(id, () => { active--; resolve(`image:${id}`); }));
  }, 2);
  const far = load('far', 10);
  const center = load('center', 0);
  const near = load('near', 1);
  assert.equal(load('center', 0), center);
  await nextTurn();
  assert.deepEqual(started, ['center', 'near']);
  finish.get('center')();
  finish.get('near')();
  await nextTurn();
  assert.deepEqual(started, ['center', 'near', 'far']);
  finish.get('far')();
  assert.deepEqual(await Promise.all([far, center, near]), ['image:far', 'image:center', 'image:near']);
  assert.equal(peak, 2);
  assert.equal(await load('center'), 'image:center');
  assert.equal(started.length, 3);
});

test('failed previews can retry and the least recently used images are evicted', async () => {
  const calls = [];
  let fail = true;
  const load = createPreviewLoader(async id => {
    calls.push(id);
    if (id === 'retry' && fail) { fail = false; throw new Error('decode failed'); }
    return id;
  }, 2, 2);
  await assert.rejects(load('retry'), /decode failed/);
  await nextTurn();
  assert.equal(await load('retry'), 'retry');
  await load('second');
  await load('retry');
  await load('third');
  await load('second');
  assert.deepEqual(calls, ['retry', 'retry', 'second', 'third', 'second']);
});

test('a changed source revision fetches a fresh preview', async () => {
  let calls = 0;
  const load = createPreviewLoader(async () => `image-${++calls}`);
  assert.equal(await load('same-id', 0, 'old'), 'image-1');
  assert.equal(await load('same-id', 0, 'old'), 'image-1');
  assert.equal(await load('same-id', 0, 'new'), 'image-2');
});

test('cached covers are available synchronously for remounted cards and respect source revisions', async () => {
  const load = createPreviewLoader(async id => `image:${id}`, 1, 2);
  assert.equal(load.peek('a', 'v1'), undefined);
  await load('a', 0, 'v1');
  await load('b', 0, 'v1');
  assert.equal(load.peek('a', 'v1'), 'image:a');
  assert.equal(load.peek('a', 'v2'), undefined);
  await load('c', 0, 'v1');
  assert.equal(load.peek('a', 'v1'), 'image:a');
  assert.equal(load.peek('b', 'v1'), undefined);
});

test('scrolling reprioritizes queued source covers without restarting requests', async () => {
  const started = [];
  const finish = new Map();
  const load = createPreviewLoader(id => {
    started.push(id);
    return new Promise(resolve => finish.set(id, () => resolve(id)));
  }, 1);
  const busy = load('busy');
  await nextTurn();
  const left = load('left', 1);
  const right = load('right', 8, 'v1');
  load.prioritize('right', 0, 'v1');
  finish.get('busy')();
  await busy;
  await nextTurn();
  assert.deepEqual(started, ['busy', 'right']);
  finish.get('right')(); await right; await nextTurn();
  finish.get('left')(); await left;
});
