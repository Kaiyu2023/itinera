# Itinera AWS module

This directory is the public, reusable Terraform **child module** for the
Itinera API. It creates the application resources whose shape is safe to review
in public:

- one Rust Lambda on `provided.al2023`, a `live` alias, and a Function URL;
- one provisioned DynamoDB Standard table with the documented `pk` / `sk` keys
  and sparse `gsi1`;
- a Lambda execution role limited to application logs and exact table/index
  data operations;
- a retained CloudWatch log group and alarms for Lambda errors, throttling,
  concurrency, Function URL 5xx responses, and table/index throttling; and
- deletion protection, point-in-time recovery, reserved concurrency, and
  conservative capacity defaults.

It intentionally creates no provider configuration, Terraform backend, GitHub
deployment role, state bucket, budget, DNS record, Cloudflare resource, SNS
topic, or secret. Those identify or operate a real environment and therefore
belong to the private `itinera-deploy` root module.

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
  alarm_action_arns              = [aws_sns_topic.operations.arn]

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
still exist in state; they are deployment identifiers, not a mechanism for
storing secrets. Runtime secrets must be referenced through a managed secret
service instead of passed as Terraform literals.

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

The Function URL uses `NONE` because Cloudflare cannot sign AWS IAM requests.
AWS therefore treats the URL as public. The API still validates every
Cloudflare Access JWT, and production remains blocked until the separately
documented origin-hardening and browser request protections are implemented.
The URL is not a secret and reserved concurrency only limits—not eliminates—the
cost of direct abuse.

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
environment or `terraform apply`.
