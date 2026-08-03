/**
 * The only supported bridge from Cloudflare Access to the Lambda Function URL.
 *
 * ITINERA_ORIGIN_URL and ITINERA_ORIGIN_SECRET are encrypted Worker secrets,
 * configured by the private deployment workflow. The browser sees neither.
 */

const ORIGIN_HEADER = "x-itinera-origin-verification";
const LAMBDA_URL_HOST = /^[a-z0-9-]+\.lambda-url\.[a-z0-9-]+\.on\.aws$/;
const URL_SAFE_SECRET = /^[A-Za-z0-9_-]{43,128}$/;

function unavailable() {
  return new Response("Service unavailable", {
    status: 503,
    headers: { "cache-control": "private, no-store" },
  });
}

function validOrigin(value) {
  try {
    const origin = new URL(value);
    return (
      origin.protocol === "https:" &&
      LAMBDA_URL_HOST.test(origin.hostname) &&
      origin.username === "" &&
      origin.password === "" &&
      origin.pathname === "/" &&
      origin.search === "" &&
      origin.hash === ""
    );
  } catch {
    return false;
  }
}

export default {
  async fetch(request, env) {
    if (
      !validOrigin(env.ITINERA_ORIGIN_URL) ||
      typeof env.ITINERA_ORIGIN_SECRET !== "string" ||
      !URL_SAFE_SECRET.test(env.ITINERA_ORIGIN_SECRET)
    ) {
      return unavailable();
    }

    const incomingUrl = new URL(request.url);
    const targetUrl = new URL(env.ITINERA_ORIGIN_URL);
    targetUrl.pathname = incomingUrl.pathname;
    targetUrl.search = incomingUrl.search;

    // `new Request(target, request)` clones the method, body and Access JWT.
    // `set` replaces any attacker-supplied copy rather than appending another.
    const originRequest = new Request(targetUrl, request);
    originRequest.headers.set(ORIGIN_HEADER, env.ITINERA_ORIGIN_SECRET);

    try {
      const response = await fetch(originRequest, { redirect: "manual" });
      // Never follow a redirect with the origin credential or reveal the raw
      // Function URL to a browser through a Location response header.
      if (response.status >= 300 && response.status < 400) {
        return unavailable();
      }
      return response;
    } catch {
      return unavailable();
    }
  },
};
