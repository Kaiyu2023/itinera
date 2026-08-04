import assert from "node:assert/strict";
import { createHash, webcrypto } from "node:crypto";
import test from "node:test";

// Exercise Wrangler's deployable JavaScript bundle, not a test-only
// TypeScript transpilation path.
import worker from "./dist/cloudflare-origin-proxy.js";

globalThis.crypto ??= webcrypto;

const ORIGIN_URL = "https://d111111abcdef8.cloudfront.net/";
const EDGE_PROOF = "0123456789abcdefghijklmnopqrstuvwxyzABCDEFG";
const ACCESS_ASSERTION = "signed-access-application-token";

function environment(overrides = {}) {
  return {
    ITINERA_CLOUDFRONT_URL: ORIGIN_URL,
    ITINERA_EDGE_PROOF: EDGE_PROOF,
    ...overrides,
  };
}

test("replaces proof and payload hash while stripping login credentials", async (t) => {
  const originalFetch = globalThis.fetch;
  t.after(() => {
    globalThis.fetch = originalFetch;
  });

  let forwarded;
  let options;
  globalThis.fetch = async (request, init) => {
    forwarded = request;
    options = init;
    return new Response(JSON.stringify({ ok: true }), { status: 200 });
  };

  const body = JSON.stringify({ hello: "world" });
  const request = new Request("https://api.itinera.example/me?view=full", {
    method: "POST",
    headers: {
      authorization: "Bearer must-not-reach-aws",
      "cf-access-client-id": "service-client-id",
      "cf-access-client-secret": "service-client-secret",
      "cf-access-jwt-assertion": ACCESS_ASSERTION,
      "cf-access-token": "upstream-token",
      "content-type": "application/json",
      cookie: "CF_Authorization=session; other=value",
      "x-amz-content-sha256": "attacker-hash",
      "x-itinera-edge-proof": "attacker-proof",
    },
    body,
  });
  const response = await worker.fetch(request, environment());

  assert.equal(response.status, 200);
  assert.equal(response.headers.get("cache-control"), "private, no-store");
  assert.equal(forwarded.url, `${ORIGIN_URL}me?view=full`);
  assert.equal(forwarded.method, "POST");
  assert.equal(
    forwarded.headers.get("cf-access-jwt-assertion"),
    ACCESS_ASSERTION,
  );
  assert.equal(forwarded.headers.get("x-itinera-edge-proof"), EDGE_PROOF);
  assert.equal(
    forwarded.headers.get("x-amz-content-sha256"),
    createHash("sha256").update(body).digest("hex"),
  );
  assert.equal(forwarded.headers.get("authorization"), null);
  assert.equal(forwarded.headers.get("cf-access-client-id"), null);
  assert.equal(forwarded.headers.get("cf-access-client-secret"), null);
  assert.equal(forwarded.headers.get("cf-access-token"), null);
  assert.equal(forwarded.headers.get("cookie"), null);
  assert.equal(await forwarded.text(), body);
  assert.equal(options.redirect, "manual");
});

test("hashes PATCH bodies used by the API", async (t) => {
  const originalFetch = globalThis.fetch;
  t.after(() => {
    globalThis.fetch = originalFetch;
  });

  let forwarded;
  globalThis.fetch = async (request) => {
    forwarded = request;
    return new Response(null, { status: 204 });
  };

  const body = "patch-body";
  const response = await worker.fetch(
    new Request("https://api.itinera.example/trips/t1", {
      method: "PATCH",
      headers: { "cf-access-jwt-assertion": ACCESS_ASSERTION },
      body,
    }),
    environment(),
  );

  assert.equal(response.status, 204);
  assert.equal(
    forwarded.headers.get("x-amz-content-sha256"),
    createHash("sha256").update(body).digest("hex"),
  );
});

test("rejects a request without an Access assertion before CloudFront", async (t) => {
  const originalFetch = globalThis.fetch;
  t.after(() => {
    globalThis.fetch = originalFetch;
  });

  let called = false;
  globalThis.fetch = async () => {
    called = true;
    return new Response();
  };

  const response = await worker.fetch(
    new Request("https://api.itinera.example/me"),
    environment(),
  );

  assert.equal(response.status, 401);
  assert.equal(response.headers.get("cache-control"), "private, no-store");
  assert.equal(called, false);
});

test("rejects oversized or encoded bodies before CloudFront", async (t) => {
  const originalFetch = globalThis.fetch;
  t.after(() => {
    globalThis.fetch = originalFetch;
  });

  let called = false;
  globalThis.fetch = async () => {
    called = true;
    return new Response();
  };

  const oversized = await worker.fetch(
    new Request("https://api.itinera.example/me", {
      method: "POST",
      headers: {
        "cf-access-jwt-assertion": ACCESS_ASSERTION,
        "content-length": "1048577",
      },
      body: "small",
    }),
    environment(),
  );
  const encoded = await worker.fetch(
    new Request("https://api.itinera.example/me", {
      method: "POST",
      headers: {
        "cf-access-jwt-assertion": ACCESS_ASSERTION,
        "content-encoding": "gzip",
      },
      body: "compressed-looking",
    }),
    environment(),
  );
  const actuallyOversized = await worker.fetch(
    new Request("https://api.itinera.example/me", {
      method: "POST",
      headers: { "cf-access-jwt-assertion": ACCESS_ASSERTION },
      body: "x".repeat(1024 * 1024 + 1),
    }),
    environment(),
  );

  assert.equal(oversized.status, 413);
  assert.equal(encoded.status, 413);
  assert.equal(actuallyOversized.status, 413);
  assert.equal(called, false);
});

test("fails closed when Worker bindings are absent", async (t) => {
  const originalFetch = globalThis.fetch;
  t.after(() => {
    globalThis.fetch = originalFetch;
  });

  let called = false;
  globalThis.fetch = async () => {
    called = true;
    return new Response();
  };

  const response = await worker.fetch(
    new Request("https://api.itinera.example/me", {
      headers: { "cf-access-jwt-assertion": ACCESS_ASSERTION },
    }),
    undefined,
  );

  assert.equal(response.status, 503);
  assert.equal(called, false);
});

test("fails closed instead of sending proof to a non-CloudFront origin", async (t) => {
  const originalFetch = globalThis.fetch;
  t.after(() => {
    globalThis.fetch = originalFetch;
  });

  let called = false;
  globalThis.fetch = async () => {
    called = true;
    return new Response();
  };

  const response = await worker.fetch(
    new Request("https://api.itinera.example/me", {
      headers: { "cf-access-jwt-assertion": ACCESS_ASSERTION },
    }),
    environment({ ITINERA_CLOUDFRONT_URL: "https://attacker.example/" }),
  );

  assert.equal(response.status, 503);
  assert.equal(called, false);
});

test("does not expose or follow an origin redirect", async (t) => {
  const originalFetch = globalThis.fetch;
  t.after(() => {
    globalThis.fetch = originalFetch;
  });

  globalThis.fetch = async () =>
    new Response(null, {
      status: 302,
      headers: { location: ORIGIN_URL },
    });

  const response = await worker.fetch(
    new Request("https://api.itinera.example/me", {
      headers: { "cf-access-jwt-assertion": ACCESS_ASSERTION },
    }),
    environment(),
  );

  assert.equal(response.status, 503);
  assert.equal(response.headers.get("location"), null);
});
