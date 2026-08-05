/**
 * Access-protected bridge from Cloudflare to the Itinera CloudFront origin.
 *
 * The private deployment supplies ITINERA_CLOUDFRONT_URL as a normal binding
 * and ITINERA_EDGE_PROOF as an encrypted Worker secret. The plaintext proof is
 * never sent to Lambda or stored in Terraform state.
 */

// Bindings are untrusted configuration until the handler validates them. Using
// unknown and optional values keeps that fail-closed check visible to TypeScript.
interface WorkerEnvironment {
  readonly ITINERA_CLOUDFRONT_URL?: unknown;
  readonly ITINERA_EDGE_PROOF?: unknown;
}

const ACCESS_ASSERTION_HEADER = "cf-access-jwt-assertion";
const EDGE_PROOF_HEADER = "x-itinera-edge-proof";
const PAYLOAD_HASH_HEADER = "x-amz-content-sha256";
const CLOUDFRONT_HOST = /^[a-z0-9]+\.cloudfront\.net$/;
const URL_SAFE_PROOF = /^[A-Za-z0-9_-]{43,128}$/;
const MAX_ASSERTION_LENGTH = 16 * 1024;
const MAX_BODY_BYTES = 1024 * 1024;

const STRIPPED_HEADERS = [
  "authorization",
  "cf-access-client-id",
  "cf-access-client-secret",
  "cf-access-token",
  "cf-authorization",
  "connection",
  "content-length",
  "cookie",
  "host",
  PAYLOAD_HASH_HEADER,
  "proxy-authorization",
  "transfer-encoding",
];

function privateResponse(message: string, status: number): Response {
  return new Response(message, {
    status,
    headers: {
      "cache-control": "private, no-store",
      "content-type": "text/plain; charset=utf-8",
    },
  });
}

function unavailable(): Response {
  return privateResponse("Service unavailable", 503);
}

function validOrigin(value: unknown): value is string {
  if (typeof value !== "string") {
    return false;
  }

  try {
    const origin = new URL(value);
    return (
      origin.protocol === "https:" &&
      CLOUDFRONT_HOST.test(origin.hostname) &&
      origin.username === "" &&
      origin.password === "" &&
      origin.port === "" &&
      origin.pathname === "/" &&
      origin.search === "" &&
      origin.hash === ""
    );
  } catch {
    return false;
  }
}

function toHex(bytes: ArrayBuffer): string {
  return Array.from(new Uint8Array(bytes), (byte) =>
    byte.toString(16).padStart(2, "0"),
  ).join("");
}

async function payloadFor(
  request: Request,
  headers: Headers,
): Promise<ArrayBuffer | null | undefined> {
  const methodRequiresHash =
    request.method === "POST" || request.method === "PUT";
  const hasBody = request.body !== null;
  if (!methodRequiresHash && !hasBody) {
    return undefined;
  }

  const body = await request.arrayBuffer();
  if (body.byteLength > MAX_BODY_BYTES) {
    return null;
  }
  const digest = await crypto.subtle.digest("SHA-256", body);
  headers.set(PAYLOAD_HASH_HEADER, toHex(digest));
  return body;
}

export default {
  async fetch(request, env?: WorkerEnvironment): Promise<Response> {
    const originUrl = env?.ITINERA_CLOUDFRONT_URL;
    const edgeProof = env?.ITINERA_EDGE_PROOF;
    if (
      !validOrigin(originUrl) ||
      typeof edgeProof !== "string" ||
      !URL_SAFE_PROOF.test(edgeProof)
    ) {
      return unavailable();
    }

    const assertion = request.headers.get(ACCESS_ASSERTION_HEADER);
    if (
      assertion === null ||
      assertion.length === 0 ||
      assertion.length > MAX_ASSERTION_LENGTH
    ) {
      return privateResponse("Unauthorized", 401);
    }

    const contentEncoding = request.headers.get("content-encoding");
    const declaredLength = request.headers.get("content-length");
    if (
      (contentEncoding && contentEncoding.toLowerCase() !== "identity") ||
      (declaredLength &&
        (!/^\d+$/.test(declaredLength) ||
          Number(declaredLength) > MAX_BODY_BYTES))
    ) {
      return privateResponse("Payload not supported", 413);
    }

    const incomingUrl = new URL(request.url);
    const targetUrl = new URL(originUrl);
    targetUrl.pathname = incomingUrl.pathname;
    targetUrl.search = incomingUrl.search;

    const headers = new Headers(request.headers);
    for (const name of STRIPPED_HEADERS) {
      headers.delete(name);
    }
    headers.set(EDGE_PROOF_HEADER, edgeProof);

    try {
      const body = await payloadFor(request, headers);
      if (body === null) {
        return privateResponse("Payload not supported", 413);
      }
      const originRequest = new Request(targetUrl, {
        method: request.method,
        headers,
        body,
      });
      const response = await fetch(originRequest, { redirect: "manual" });

      // The API contract does not use redirects. Refusing them prevents a raw
      // origin address from escaping through Location and prevents forwarding
      // the proof to another host.
      if (response.status >= 300 && response.status < 400) {
        return unavailable();
      }

      const responseHeaders = new Headers(response.headers);
      responseHeaders.set("cache-control", "private, no-store");
      return new Response(response.body, {
        status: response.status,
        statusText: response.statusText,
        headers: responseHeaders,
      });
    } catch {
      return unavailable();
    }
  },
} satisfies ExportedHandler<WorkerEnvironment>;
