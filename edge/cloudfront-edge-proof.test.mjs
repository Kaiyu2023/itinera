import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { readFile } from "node:fs/promises";
import { createRequire } from "node:module";
import test from "node:test";
import vm from "node:vm";

const require = createRequire(import.meta.url);
const PROOF_HEADER = "x-itinera-edge-proof";
const TEMPLATE_URL = new URL(
  "../infra/cloudfront-edge-proof.js.tftpl",
  import.meta.url,
);

async function loadHandler(proofs) {
  const source = await readFile(TEMPLATE_URL, "utf8");
  const digests = proofs.map((proof) =>
    createHash("sha256").update(proof).digest("hex"),
  );
  const rendered = source
    .replace("${proof_header_json}", JSON.stringify(PROOF_HEADER))
    .replace("${allowed_digests_json}", JSON.stringify(digests));

  return vm.runInNewContext(`${rendered}\nhandler;`, { require });
}

function event(proofHeader) {
  const headers = {
    "cf-access-jwt-assertion": { value: "signed-assertion" },
  };
  if (proofHeader) {
    headers[PROOF_HEADER] = proofHeader;
  }
  return {
    request: {
      method: "GET",
      uri: "/me",
      querystring: {},
      headers,
      cookies: {},
    },
  };
}

test("accepts a valid proof and removes it before origin forwarding", async () => {
  const proof = "0123456789abcdefghijklmnopqrstuvwxyzABCDEFG";
  const handler = await loadHandler([proof]);
  const requestEvent = event({ value: proof });

  const result = handler(requestEvent);

  assert.equal(result.uri, "/me");
  assert.equal(result.headers[PROOF_HEADER], undefined);
  assert.equal(
    result.headers["cf-access-jwt-assertion"].value,
    "signed-assertion",
  );
});

test("accepts either digest during a zero-downtime rollover", async () => {
  const oldProof = "0123456789abcdefghijklmnopqrstuvwxyzABCDEFG";
  const newProof = "ABCDEFG0123456789abcdefghijklmnopqrstuvwxyz";
  const handler = await loadHandler([oldProof, newProof]);

  assert.equal(handler(event({ value: oldProof })).uri, "/me");
  assert.equal(handler(event({ value: newProof })).uri, "/me");
});

test("rejects missing, incorrect, duplicate, and oversized proofs", async () => {
  const proof = "0123456789abcdefghijklmnopqrstuvwxyzABCDEFG";
  const handler = await loadHandler([proof]);
  const invalidEvents = [
    event(),
    event({ value: "wrong-proof" }),
    event({ value: proof, multiValue: [{ value: proof }, { value: proof }] }),
    event({ value: "x".repeat(129) }),
  ];

  for (const invalidEvent of invalidEvents) {
    const response = handler(invalidEvent);
    assert.equal(response.statusCode, 403);
    assert.equal(response.headers["cache-control"].value, "private, no-store");
  }
});
