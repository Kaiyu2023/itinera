/**
 * Itinera service worker — hand-rolled, no build plugin (DESIGN.md §11.7:
 * "read your plan with no roaming data").
 *
 * Strategy:
 * - Navigations: network-first, falling back to the cached app shell ('/')
 *   so the SPA still boots offline; the router + react-query cache render
 *   whatever data was seen before.
 * - Hashed build assets (/assets/*): cache-first — immutable by construction.
 * - Photos & fonts: stale-while-revalidate, capped LRU-ish by cache clear on
 *   version bump.
 * - Everything else (future API calls): pass through untouched. The mock app
 *   has no network data; when HttpApiClient lands, API caching stays OFF —
 *   correctness beats offline for collaborative state.
 *
 * Bump VERSION on any strategy change; old caches are dropped on activate.
 */
const VERSION = 'itinera-v1';
const SHELL = ['/', '/manifest.webmanifest', '/favicon.svg'];

self.addEventListener('install', (event) => {
  event.waitUntil(
    caches
      .open(VERSION)
      .then((cache) => cache.addAll(SHELL))
      .then(() => self.skipWaiting()),
  );
});

self.addEventListener('activate', (event) => {
  event.waitUntil(
    caches
      .keys()
      .then((keys) => Promise.all(keys.filter((k) => k !== VERSION).map((k) => caches.delete(k))))
      .then(() => self.clients.claim()),
  );
});

self.addEventListener('fetch', (event) => {
  const { request } = event;
  if (request.method !== 'GET') return;
  const url = new URL(request.url);
  if (url.origin !== self.location.origin) return;

  // SPA navigations: try the network, fall back to the cached shell.
  if (request.mode === 'navigate') {
    event.respondWith(
      fetch(request)
        .then((res) => {
          const copy = res.clone();
          caches.open(VERSION).then((cache) => cache.put('/', copy));
          return res;
        })
        .catch(() => caches.match('/')),
    );
    return;
  }

  // Immutable hashed assets: cache-first.
  if (url.pathname.startsWith('/assets/')) {
    event.respondWith(
      caches.match(request).then(
        (hit) =>
          hit ??
          fetch(request).then((res) => {
            const copy = res.clone();
            caches.open(VERSION).then((cache) => cache.put(request, copy));
            return res;
          }),
      ),
    );
    return;
  }

  // Photos, fonts, icons: stale-while-revalidate.
  if (/^\/(photos|fonts)\//.test(url.pathname) || /\.(png|svg|webp|woff2?)$/.test(url.pathname)) {
    event.respondWith(
      caches.match(request).then((hit) => {
        const refresh = fetch(request)
          .then((res) => {
            const copy = res.clone();
            caches.open(VERSION).then((cache) => cache.put(request, copy));
            return res;
          })
          .catch(() => hit);
        return hit ?? refresh;
      }),
    );
  }
});
