# Itinera AWS module

This directory is the public, reusable Terraform **child module** for the
Itinera API. It creates the application resources whose shape is safe to review
in public:

- one Rust Lambda on `provided.al2023`, a `live` alias, and an `AWS_IAM`
  Function URL;
- one no-cache CloudFront distribution whose viewer Function validates the
  Cloudflare Worker proof and whose Origin Access Control signs Lambda requests;
- one provisioned DynamoDB Standard table with the documented `pk` / `sk` keys,
  sparse `gsi1`, and asynchronous cleanup through the `ttl` attribute;
- a Lambda execution role limited to application logs and exact table/index
  data operations;
- a retained CloudWatch log group and optional actionable alarms for Lambda
  errors, throttling, concurrency, Function URL 5xx responses, and table/index
  throttling; and
- deletion protection, point-in-time recovery, reserved concurrency, and
  conservative capacity defaults.

It intentionally creates no provider configuration, Terraform backend, GitHub
deployment role, state bucket, budget, DNS record, Cloudflare resource, SNS
topic, or plaintext secret. Those identify or operate a real environment and
therefore belong to the private `itinera-deploy` root module. The reviewed
Worker source and its deployment contract live in [`../edge`](../edge).

This wiring does not add or change an HTTP route, request, or response. The
OpenAPI contract therefore needs no infrastructure-specific change.

## Use from the private root

Pin this module to a reviewed commit or release. The private workflow builds the
Lambda artifact first, then supplies its path and digest together with the real
environment values:

```hcl
module "itinera" {
  source = "git::https://github.com/Kaiyu2023/itinera.git//infra?ref=<reviewed-commit>"

  name_prefix                    = var.name_prefix
  lambda_package_path            = var.lambda_package_path
  lambda_source_code_hash        = filebase64sha256(var.lambda_package_path)
  cloudflare_access_team_domain  = var.cloudflare_access_team_domain
  cloudflare_access_audience     = var.cloudflare_access_audience
  edge_proof_sha256_hashes       = var.edge_proof_sha256_hashes

  # Optional. Empty (the default) creates no CloudWatch alarms.
  alarm_action_arns = [aws_sns_topic.operations.arn]

  tags = var.tags
}
```

The package must be compiled for the same architecture as
`lambda_architecture` (Arm64 by default). From `backend/`, Cargo Lambda can
produce the expected zip with:

```sh
cargo lambda build --release --arm64 --output-format zip --package itinera-api
```

The deployment build must use the default Cargo features. It must not compile
or set development authentication.

## State and deployment boundary

The private root configures an encrypted S3 backend with `use_lockfile = true`.
The state bucket is bootstrapped separately, has versioning and all S3 public
access blocks enabled, and grants the deployment role access only to the
specific state and lock objects. The public module has no `backend` or
`provider` block, so it cannot select an account or deploy by itself.

No real value belongs in this directory, a checked-in `*.tfvars` file, or public
CI. Inputs marked `sensitive` are hidden from normal Terraform output, but they
still exist in state. `edge_proof_sha256_hashes` contains only one-way digests;
the private workflow installs the plaintext directly as the encrypted
`ITINERA_EDGE_PROOF` Worker secret. Plaintext runtime secrets must never be
passed as Terraform literals.

The module's `.terraform.lock.hcl` makes public validation reproducible. A child
module's lock file is not inherited by callers, so `itinera-deploy` maintains
its own authoritative provider lock file.

## Safe defaults and deliberate overrides

The default table plus GSI allocation is 15 RCU and 10 WCU in total. The module
rejects totals above 25 RCU or 25 WCU while
`enforce_free_tier_capacity_limit = true`; that check cannot know about other
tables in the same payer account, so the private deployment still has to check
account-wide usage and configure a budget alert.

DynamoDB and log deletion protection default to on. For an intentional
teardown, change those settings in a reviewed apply before destroying the
resources. Higher capacity can likewise be enabled only with an explicit
override after reviewing cost and alarm coverage.

The table enables DynamoDB TTL on the numeric `ttl` attribute. Application code
still enforces every expiry synchronously; TTL is cleanup only for bounded
service-usage buckets and idempotency claims, never an authorization boundary.

The Function URL uses `AWS_IAM`. Its resource policy grants
`lambda:InvokeFunctionUrl` and `lambda:InvokeFunction` only to the exact
CloudFront distribution. CloudFront OAC performs the SigV4 signing, so an
unsigned direct request fails at AWS before Lambda starts.

CloudFront itself is also public, so a viewer Function checks the high-entropy
proof that the Access-protected Worker overwrites on every request. It stores
only one or two SHA-256 digests, rejects missing, duplicate, oversized, or
incorrect proof before origin work, and strips the proof before OAC forwards
the request. The API cache policy has zero TTL, forwards no cookies, and allows
only the signed Access assertion and required HTTP metadata to reach Lambda.
Rust independently validates that assertion; the proof is not application
authentication. The allowlist includes the two standard browser preflight
headers so restrictive CORS can be enforced by the API rather than accidentally
broken at the origin boundary.

The private root passes `cloudflare_worker_origin_url` into the Worker's
`ITINERA_CLOUDFRONT_URL` binding. Never point the Worker at the
`lambda_function_url` output. It must also disable the public `workers.dev` and
preview URLs, leaving only the Access-protected custom API hostname. See the
[edge deployment and rollover steps](../edge/README.md).

`alarm_action_arns` defaults to an empty list. In that mode the module creates
no alarms and needs no SNS topic. Supplying one or more topic ARNs creates all
eight alarms with those actions. The production root should normally opt in:
an alarm without a reachable operator is not useful, but omitting monitoring is
an explicit operational trade-off.

## Local validation

Normal frontend/backend development does not require Terraform or AWS
credentials. When changing `infra/`, run:

```sh
terraform fmt -check -recursive
terraform init -backend=false
terraform validate
terraform test
```

`terraform test` uses a mocked AWS provider. It plans no real account resources
and verifies the important security and cost invariants. Public CI runs only
these non-deployment checks; it never runs `terraform plan` against an
environment or `terraform apply`. The static CI job also generates Cloudflare
runtime types, type-checks and bundles the TypeScript Worker, and exercises
proof replacement, credential stripping, payload hashes, rollover, and
fail-closed behavior against the deployable JavaScript artifact.
