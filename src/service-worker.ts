/// <reference lib="webworker" />
import { assets, immutable } from '$app/manifest';
import { version } from '$app/env';

const sw = self as unknown as ServiceWorkerGlobalScope;
const family = 'financial-gps-app-';
const cacheName = family + version;
const shell = '/';
const paths = new Set([
  ...immutable.map(item => new URL(item.path, sw.location.origin).pathname),
  ...assets.filter(item => !item.path.endsWith('/_headers') && item.path !== '_headers')
    .map(item => new URL(item.path, sw.location.origin).pathname),
  shell
]);

sw.addEventListener('install', event => {
  event.waitUntil((async () => {
    const cache = await caches.open(cacheName);
    await cache.addAll([...paths]);
    await sw.skipWaiting();
  })());
});

sw.addEventListener('activate', event => {
  event.waitUntil((async () => {
    for (const name of await caches.keys()) {
      if (name.startsWith(family) && name !== cacheName) await caches.delete(name);
    }
    await sw.clients.claim();
  })());
});

sw.addEventListener('fetch', event => {
  const request = event.request;
  const url = new URL(request.url);
  if (request.method !== 'GET' || url.origin !== sw.location.origin) return;
  if (request.mode !== 'navigate' && !paths.has(url.pathname)) return;
  event.respondWith((async () => {
    const cache = await caches.open(cacheName);
    const cached = await cache.match(request.mode === 'navigate' ? shell : url.pathname);
    return cached ?? fetch(request);
  })());
});
