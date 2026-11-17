// Minimal hand-written service worker (phase 4, batch 5a). No workbox.
//
// Scope: cache the application SHELL (the page + its hashed JS/CSS) so the
// installed Web app opens when offline and loads fast when served locally.
// It NEVER caches photos or API responses:
//   * `/api/*` (commands, SSE, signed media) is bypassed entirely — signed
//     media URLs expire and the library is server state, not shell;
//   * navigation requests are network-first, so a new build on lapd is used
//     as soon as the device is online (the cache is only the offline fallback).
//
// Hashed asset filenames are immutable, so they are served cache-first. The
// version constant only needs a bump if the caching *strategy* changes.
const CACHE = 'lap-shell-v1';
const SHELL = ['/', '/index.html', '/manifest.webmanifest', '/icon-192.png', '/icon-512.png', '/favicon.ico'];

self.addEventListener('install', (event) => {
  event.waitUntil(
    caches.open(CACHE).then((cache) => cache.addAll(SHELL)).then(() => self.skipWaiting())
  );
});

self.addEventListener('activate', (event) => {
  event.waitUntil(
    caches.keys()
      .then((keys) => Promise.all(keys.filter((k) => k !== CACHE).map((k) => caches.delete(k))))
      .then(() => self.clients.claim())
  );
});

self.addEventListener('fetch', (event) => {
  const request = event.request;
  if (request.method !== 'GET') return;
  const url = new URL(request.url);
  // Only same-origin shell; never touch the API or cross-origin.
  if (url.origin !== self.location.origin) return;
  if (url.pathname.startsWith('/api/')) return;

  // Navigation: network-first (fresh build wins), fall back to the cached shell.
  if (request.mode === 'navigate') {
    event.respondWith(
      fetch(request).catch(() => caches.match('/index.html'))
    );
    return;
  }

  // Static assets (hashed js/css, icons): cache-first, then fill the cache.
  if (/\.[0-9a-z]+$/i.test(url.pathname) || url.pathname.startsWith('/assets/')) {
    event.respondWith(
      caches.match(request).then((cached) => cached || fetch(request).then((response) => {
        if (response.ok) {
          const copy = response.clone();
          caches.open(CACHE).then((cache) => cache.put(request, copy));
        }
        return response;
      }).catch(() => cached))
    );
  }
});
