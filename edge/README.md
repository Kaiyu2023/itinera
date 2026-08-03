# Cloudflare origin proxy

`cloudflare-origin-proxy.mjs` is the reviewed Worker that sits on the
Access-protected API hostname and forwards requests to the Lambda Function URL.
It preserves Cloudflare's signed Access assertion, replaces any client-supplied
`X-Itinera-Origin-Verification` header, and never follows origin redirects.

The private deployment workflow supplies two encrypted Worker secrets:

- `ITINERA_ORIGIN_URL`: the Terraform module's sensitive Function URL output;
- `ITINERA_ORIGIN_SECRET`: a newly generated, cryptographically random value of
  at least 32 bytes, encoded as 43 URL-safe base64 characters.

Only the SHA-256 hash of `ITINERA_ORIGIN_SECRET` is passed to Terraform through
`origin_secret_sha256_hashes`. The plaintext must never be a Terraform input,
GitHub secret printed by a command, checked-in file, URL, or browser value.

The deployment order is:

1. deploy Lambda with the current secret hash;
2. configure the Worker secrets and deploy this exact reviewed Worker;
3. attach it to the Access-protected API hostname; and
4. smoke-test the hostname, then verify the raw Function URL returns `403` for
   the same request without the Worker-injected header.

For zero-downtime rotation, deploy `[old_hash, new_hash]`, change the Worker
secret to the new value, verify traffic, and finally deploy `[new_hash]`.

Run the dependency-free tests with Node 22 or later:

```sh
node --test cloudflare-origin-proxy.test.mjs
```
