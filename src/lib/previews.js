import { invoke } from '@tauri-apps/api/core';

// Keep decode work bounded and let the closest visible cards load first.
export function createPreviewLoader(fetchPreview, concurrency = 2, capacity = 100) {
  const cache = new Map();
  const pending = new Map();
  const queue = [];
  let running = 0;
  function pump() {
    queue.sort((a, b) => a.priority - b.priority);
    while (running < concurrency && queue.length) {
      const job = queue.shift();
      running++;
      Promise.resolve().then(() => fetchPreview(job.id)).then(data => {
        cache.set(job.key, data);
        if (cache.size > capacity) cache.delete(cache.keys().next().value);
        job.resolve(data);
      }, job.reject).finally(() => {
        pending.delete(job.key);
        running--;
        pump();
      });
    }
  }
  const load = (id, priority = 0, revision = '') => {
    const key = JSON.stringify([id, revision]);
    if (cache.has(key)) {
      const data = cache.get(key);
      cache.delete(key);
      cache.set(key, data);
      return Promise.resolve(data);
    }
    if (pending.has(key)) {
      const job = pending.get(key);
      job.priority = priority;
      return job.promise;
    }
    const job = { id, key, priority };
    job.promise = new Promise((resolve, reject) => { job.resolve = resolve; job.reject = reject; });
    pending.set(key, job);
    queue.push(job);
    queueMicrotask(pump);
    return job.promise;
  };
  // Re-mounted cards start with their best cached image instead of flashing a thumbnail.
  load.peek = (id, revision = '') => {
    const key = JSON.stringify([id, revision]);
    const data = cache.get(key);
    if (data) { cache.delete(key); cache.set(key, data); }
    return data;
  };
  load.prioritize = (id, priority, revision = '') => {
    const job = pending.get(JSON.stringify([id, revision]));
    if (job) job.priority = priority;
  };
  return load;
}

// A single source-quality cover: no blurry thumbnail or different image while scrolling.
export const loadCover = createPreviewLoader(id => invoke('wallpaper_preview', { id, quality: 'high' }), 2, 96);
