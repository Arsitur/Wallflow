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
        if (data != null) {
          cache.set(job.key, data);
          if (cache.size > capacity) cache.delete(cache.keys().next().value);
        }
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

export function createProgressiveCoverLoader(fetchPreview, concurrency = 2, capacity = 96) {
  // Cheap author previews have their own queue so scene extraction cannot hold them up.
  const thumbnails = createPreviewLoader(id => fetchPreview(id, 'low'), concurrency, capacity);
  const covers = createPreviewLoader(id => fetchPreview(id, 'high'), concurrency, capacity);
  return {
    request(id, priority, revision, update) {
      let active = true;
      const cover = covers.peek(id, revision) || null;
      let state = { thumbnail: cover ? null : thumbnails.peek(id, revision) || null, cover, failed: false };
      const publish = changes => {
        if (!active) return;
        state = { ...state, ...changes };
        update(state);
      };
      update(state);
      if (!cover) {
        if (!state.thumbnail) {
          thumbnails(id, priority, revision).then(thumbnail => publish({ thumbnail }), () => {});
        }
        covers(id, priority, revision).then(
          data => publish({ cover: data, failed: !data }),
          () => publish({ failed: true }),
        );
      }
      // Requests still populate the shared caches after a card scrolls out of view.
      return () => { active = false; };
    },
    prioritize(id, priority, revision) {
      thumbnails.prioritize(id, priority, revision);
      covers.prioritize(id, priority, revision);
    },
  };
}

export const coverLoader = createProgressiveCoverLoader((id, quality) => invoke('wallpaper_preview', { id, quality }));
