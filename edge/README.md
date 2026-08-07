# Cloudflare origin proxy

> **Transitional implementation: do not deploy it.** The accepted
> [single-node SQLite architecture](../docs/adr/0001-single-node-sqlite.md) uses
> Cloudflare Tunnel directly to a loopback-bound API on the host. This Worker,
> its proof protocol, CloudFront, and the Lambda Function URL remain only until
> the runtime cutover and will then be removed. No private environment exists.

This Worker is the only supported bridge from the Cloudflare Access-protected
API hostname to AWS:

```text
Cloudflare Access → Worker → CloudFront proof check → OAC → Lambda AWS_IAM
```

The private deployment supplies two bindings:

- `ITINERA_CLOUDFRONT_URL` — the module's `cloudflare_worker_origin_url`
  output. It must be an HTTPS root URL on `*.cloudfront.net`.
- `ITINERA_EDGE_PROOF` — an encrypted Worker secret containing 32 or more
  random bytes encoded as 43–128 base64url characters.

The Worker requires the Access application assertion, removes the credentials
used to obtain it, replaces any caller-supplied proof, and calculates
`x-amz-content-sha256` before forwarding body-bearing requests. CloudFront
validates the proof at the viewer edge and removes it. OAC then signs the
request for the IAM-protected Lambda Function URL. Rust receives the signed
Access assertion but never receives or validates the edge proof. The Worker
also rejects encoded or larger-than-1-MiB API bodies; future photos use a
separate direct-upload flow rather than passing through Lambda.

The deployment must disable both the public `workers.dev` endpoint and Worker
preview URLs. The only production route is the custom API hostname covered by
the closed Cloudflare Access policy. This matters because the Worker deliberately
leaves cryptographic assertion verification to Rust; a public alternate Worker
URL would let an attacker attach a fake assertion, obtain the Worker proof on an
outbound request, and spend a Lambda invocation even though Rust would reject it.

Only the proof's lowercase SHA-256 digest is passed to Terraform through
`edge_proof_sha256_hashes`. The plaintext must never enter a Terraform variable,
state, Lambda environment variable, repository, log, or command output.

The Worker is authored in TypeScript and bundled to JavaScript by Wrangler.
`wrangler.jsonc` deliberately contains only safe build defaults: the current
runtime compatibility date, the source entry point, and the requirement that
public development and preview URLs stay disabled. Real routes, binding values,
the Access policy, and the encrypted proof remain in the private deployment.
Runtime declarations are generated from that compatibility date rather than
maintained by hand or taken from an unrelated latest-version type package.
The lockfile temporarily overrides Wrangler's local Miniflare HTTP client to
the patched, same-major `undici` 7.29 release; remove the override once Wrangler
itself requires that version or newer.

## Deployment and rollover

The private workflow should:

1. generate the proof without printing it;
2. calculate its lowercase SHA-256 digest;
3. apply the AWS module with that digest;
4. run the edge checks below and build the deployable Worker bundle;
5. install the plaintext as the encrypted Worker secret;
6. set `ITINERA_CLOUDFRONT_URL`, disable `workers.dev` and preview URLs, deploy
   the Worker only on the custom API hostname, and attach the closed Access
   policy;
7. verify that the API hostname works, while direct CloudFront and direct
   Lambda requests return `403` without invoking the application.

For rollover, deploy old and new digests together, replace the Worker secret,
verify traffic, then remove the old digest. Routine calendar rotation is not
needed; rotate after suspected exposure, administrator changes, or an
occasional recovery exercise.

Install the pinned local toolchain, generate the matching Cloudflare runtime
types, type-check and bundle the Worker, then run the edge tests with:

```sh
npm ci
npm run generate-types
npm run typecheck
npm test
```

`npm test` first asks Wrangler for the same deployable JavaScript bundle that
production will use, then runs the Worker and CloudFront gate tests against
that artifact rather than a test-only TypeScript transpilation path.
