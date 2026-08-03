import assert from "node:assert/strict";
import test from "node:test";

import worker from "./cloudflare-origin-proxy.mjs";

const ORIGIN_URL = "https://abc123.lambda-url.eu-west-2.on.aws/";
const ORIGIN_SECRET = "0123456789abcdefghijklmnopqrstuvwxyzABCDEFG";

test("proxies the request while replacing an untrusted origin header", async (t) => {
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

  const request = new Request("https://api.itinera.example/me?view=full", {
    method: "POST",
    headers: {
      "cf-access-jwt-assertion": "signed-access-token",
      "content-type": "application/json",
      "x-itinera-origin-verification": "attacker-controlled",
    },
    body: "{}",
  });
  const response = await worker.fetch(request, {
    ITINERA_ORIGIN_URL: ORIGIN_URL,
    ITINERA_ORIGIN_SECRET: ORIGIN_SECRET,
  });

  assert.equal(response.status, 200);
  assert.equal(forwarded.url, `${ORIGIN_URL}me?view=full`);
  assert.equal(forwarded.method, "POST");
  assert.equal(
    forwarded.headers.get("cf-access-jwt-assertion"),
    "signed-access-token",
  );
  assert.equal(
    forwarded.headers.get("x-itinera-origin-verification"),
    ORIGIN_SECRET,
  );
  assert.equal(options.redirect, "manual");
});

test("fails closed instead of sending credentials to a non-Lambda origin", async (t) => {
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
    {
      ITINERA_ORIGIN_URL: "https://attacker.example/",
      ITINERA_ORIGIN_SECRET: ORIGIN_SECRET,
    },
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
    new Request("https://api.itinera.example/me"),
    {
      ITINERA_ORIGIN_URL: ORIGIN_URL,
      ITINERA_ORIGIN_SECRET: ORIGIN_SECRET,
    },
  );

  assert.equal(response.status, 503);
  assert.equal(response.headers.get("location"), null);
});
