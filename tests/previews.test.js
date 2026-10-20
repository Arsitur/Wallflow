import test from 'node:test';
import assert from 'node:assert/strict';
import { createPreviewLoader, createProgressiveCoverLoader } from '../src/lib/previews.js';

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

test('progressive covers publish the thumbnail first and reopen cached high resolution directly', async () => {
  const finish = new Map();
  const calls = [];
  const loader = createProgressiveCoverLoader((id, quality) => {
    calls.push(`${quality}:${id}`);
    return new Promise(resolve => finish.set(quality, resolve));
  });
  const states = [];
  loader.request('scene', 0, 'v1', state => states.push(state));
  await nextTurn();
  assert.deepEqual(calls, ['low:scene', 'high:scene']);
  finish.get('low')('thumbnail');
  await nextTurn();
  assert.deepEqual(states.at(-1), { thumbnail: 'thumbnail', cover: null, failed: false });
  finish.get('high')('original');
  await nextTurn();
  assert.deepEqual(states.at(-1), { thumbnail: 'thumbnail', cover: 'original', failed: false });
  const reopened = [];
  loader.request('scene', 0, 'v1', state => reopened.push(state));
  assert.deepEqual(reopened, [{ thumbnail: null, cover: 'original', failed: false }]);
  assert.equal(calls.length, 2);
});

test('slow source extraction does not block the next quick preview', async () => {
  const started = [];
  const finish = new Map();
  const loader = createProgressiveCoverLoader((id, quality) => {
    const key = `${quality}:${id}`;
    started.push(key);
    return new Promise(resolve => finish.set(key, resolve));
  }, 1);
  loader.request('first', 0, 'v1', () => {});
  const second = [];
  loader.request('second', 1, 'v1', state => second.push(state));
  await nextTurn();
  finish.get('low:first')('first-preview');
  await nextTurn();
  assert.ok(started.includes('low:second'));
  assert.ok(!started.includes('high:second'));
  finish.get('low:second')('second-preview');
  await nextTurn();
  assert.equal(second.at(-1).thumbnail, 'second-preview');
  assert.equal(second.at(-1).cover, null);
  finish.get('high:first')('first-cover');
  await nextTurn();
  finish.get('high:second')('second-cover');
  await nextTurn();
  assert.equal(second.at(-1).cover, 'second-cover');
});

test('cancelled revisions cannot update a card and late thumbnails cannot replace its cover', async () => {
  const jobs = [];
  const loader = createProgressiveCoverLoader((id, quality) => new Promise(resolve => jobs.push({ quality, resolve })));
  const oldStates = [];
  const cancel = loader.request('same', 0, 'old', state => oldStates.push(state));
  await nextTurn();
  cancel();
  const currentStates = [];
  loader.request('same', 0, 'new', state => currentStates.push(state));
  await nextTurn();
  jobs[0].resolve('old-preview');
  jobs[1].resolve('old-cover');
  await nextTurn();
  assert.equal(oldStates.length, 1);
  assert.equal(currentStates.at(-1).cover, null);
  jobs[3].resolve('new-cover');
  await nextTurn();
  jobs[2].resolve('late-preview');
  await nextTurn();
  assert.deepEqual(currentStates.at(-1), { thumbnail: 'late-preview', cover: 'new-cover', failed: false });
});

test('unavailable high resolution keeps the preview and can retry without caching a missing cover', async () => {
  for (const failure of ['missing', 'error']) {
    let highCalls = 0;
    let lowCalls = 0;
    const loader = createProgressiveCoverLoader(async (id, quality) => {
      if (quality === 'low') { lowCalls++; return 'preview'; }
      if (++highCalls === 1) {
        if (failure === 'error') throw new Error('extraction failed');
        return null;
      }
      return 'recovered-cover';
    });
    const states = [];
    loader.request('scene', 0, 'v1', state => states.push(state));
    await nextTurn();
    assert.deepEqual(states.at(-1), { thumbnail: 'preview', cover: null, failed: true });
    loader.request('scene', 0, 'v1', state => states.push(state));
    await nextTurn();
    assert.equal(states.at(-1).cover, 'recovered-cover');
    assert.equal(states.at(-1).failed, false);
    assert.equal(lowCalls, 1);
    assert.equal(highCalls, 2);
  }
});
