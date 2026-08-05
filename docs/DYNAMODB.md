# Itinera DynamoDB Design

Status: living design · 2026-08-02 · physical persistence contract

[`DESIGN.md`](DESIGN.md) defines the product's logical data model. This document
defines how repositories persist that model in Amazon DynamoDB without leaking
AWS concepts into the domain core.

## 1. Why DynamoDB

Itinera runs as one Rust Lambda and serves a very small, bursty group. DynamoDB
keeps the whole production data path inside the existing AWS account:

- Lambda authenticates with its execution role, so there is no database
  password to store or rotate;
- requests do not need a VPC, a connection pool, or a permanently running
  server;
- conditional writes and ACID transactions support the application's
  uniqueness, governance, versioning, and audit invariants; and
- provisioned Standard tables can fit the ongoing DynamoDB free tier at this
  scale. Point-in-time recovery and other optional features are billed
  separately.

This is an access-pattern-led design, not a translation of relational tables.
Repository traits remain the boundary: domain services ask for users, trips, or
polls and never construct DynamoDB keys or import the AWS SDK.

## 2. One-table shape

The deployment creates one DynamoDB Standard table with these string keys:

| Resource      | Partition key | Sort key |
| ------------- | ------------- | -------- |
| Table         | `pk`          | `sk`     |
| Sparse `gsi1` | `gsi1pk`      | `gsi1sk` |

Every item also has an `entity_type` and numeric `schema_version`. Repositories
validate both before constructing a domain object; malformed or unknown records
fail closed as internal storage errors.

`gsi1` supports reverse lookups and user-facing lists. It is intentionally
sparse: only items with both index keys enter it. Because global secondary
indexes are eventually consistent, an index result is never trusted as the
authorization decision.

Production uses a deployment-specific table name passed as
`ITINERA_DYNAMODB_TABLE`. `AWS_REGION` is supplied by Lambda. The standard SDK
credential chain is used, and the production deployment must supply only the
Lambda execution role—never static AWS keys in environment variables.

## 3. Current user records

The implemented `/me` flow separates a person's stable identity from the email
address used to find it. A user profile is stored at:

| Attribute        | Value                       |
| ---------------- | --------------------------- |
| `pk`             | `USER#<user_id>`            |
| `sk`             | `PROFILE`                   |
| `entity_type`    | `USER_PROFILE`              |
| `schema_version` | `1`                         |
| `user_id`        | generated opaque ID         |
| `email`          | current canonical email     |
| `display_name`   | optional user-authored name |

A separate uniqueness and lookup claim points to that profile:

| Attribute        | Value                                     |
| ---------------- | ----------------------------------------- |
| `pk`             | `USER_EMAIL#<SHA-256 of canonical email>` |
| `sk`             | `CLAIM`                                   |
| `entity_type`    | `USER_EMAIL_CLAIM`                        |
| `schema_version` | `1`                                       |
| `user_id`        | owning user's opaque ID                   |

First-login provisioning commits both items in one `TransactWriteItems` call.
Each put has an `attribute_not_exists` condition. The claim action comes first,
so its cancellation reason can be translated precisely into `DuplicateEmail`;
a profile-key collision is treated as a storage failure. If two Lambda
instances provision the same email concurrently, one complete transaction wins
and the loser performs the normal two-step lookup to return that user. No
partial profile or claim can be committed.

Login performs two strongly consistent `GetItem` calls: email claim, then user
profile. A missing claim means an unknown user. A claim whose profile is missing
or inconsistent is corrupt data and fails closed rather than provisioning a
second identity.

Hashing keeps the raw email out of keys that are commonly repeated in traces
and operational tooling. It is only exposure reduction: email addresses have a
small enough search space that an SHA-256 digest is not encryption. The email
attribute is still personal data and is protected by IAM, TLS, encryption at
rest, log redaction, retention rules, and restricted backups.

The profile key never changes when the user's email changes, so memberships,
votes, expenses, and audit events continue to reference the same `user_id`.
A future verified email-change command will transactionally create the new
claim if unused, update the profile while checking the old email, and delete the
old claim while checking its owner. Merely logging in with an unknown email must
never be interpreted as an account change: the new address needs a separate
proof and an authenticated account-linking flow. That HTTP flow is not part of
the current `/me` contract.

## 4. Trip aggregate and key vocabulary

Future trip repositories place a trip's related records under
`pk = TRIP#<trip_id>`. The sort key names both the entity type and its identity.
Representative keys are:

| Entity                       | `sk`                                                   | Optional `gsi1` purpose |
| ---------------------------- | ------------------------------------------------------ | ----------------------- |
| Trip metadata                | `META`                                                 | —                       |
| Membership                   | `MEMBER#<user_id>`                                     | list a user's trips     |
| Candidate and place snapshot | `CANDIDATE#<candidate_id>`                             | —                       |
| Plan metadata                | `PLAN#<version>#META`                                  | —                       |
| Day                          | `PLAN#<version>#DAY#<date>`                            | —                       |
| Stop                         | `PLAN#<version>#DAY#<date>#STOP#<sequence>#<stop_id>`  | —                       |
| Proposal                     | `PROPOSAL#<proposal_id>`                               | optional status queue   |
| Poll                         | `POLL#<poll_id>`                                       | optional status queue   |
| Vote                         | `POLL#<poll_id>#VOTE#<user_id>`                        | —                       |
| Expense                      | `EXPENSE#<expense_id>`                                 | —                       |
| Comment                      | `THREAD#<thread_id>#COMMENT#<created_at>#<comment_id>` | —                       |
| Audit event                  | `AUDIT#<created_at>#<event_id>`                        | —                       |

Fixed-width plan versions and sequence numbers preserve numeric ordering when
sorted as strings. Timestamps are UTC ISO-8601 values followed by a unique ID,
so events created in the same instant cannot overwrite one another.

Large collections remain separate items. A plan is not one embedded document:
that would approach DynamoDB's 400 KiB item limit, amplify every edit into a
large rewrite, and make concurrent changes unnecessarily conflict.

The exact key for a new entity must be recorded here before its repository is
implemented. Renaming a key prefix is a data migration, even though it does not
change the OpenAPI contract.

## 5. Required access patterns

Every user-facing repository operation is a `GetItem`, `Query`, bounded batch,
or transaction with a fully known partition key. `Scan` is excluded from Lambda's
runtime IAM policy and must not appear in an interactive route.

The main patterns are:

1. Resolve a login with strongly consistent direct reads of the canonical-email
   claim and then its stable user profile.
2. Resolve a profile by user ID with a direct `USER#<user_id>` + `PROFILE`
   `GetItem`.
3. List trips for a user by querying membership items under
   `gsi1pk = USER#<user_id>` and `gsi1sk` beginning with `TRIP#`.
4. Authorize a trip request with a strongly consistent direct read of
   `TRIP#<trip_id>` + `MEMBER#<user_id>`.
5. Load the current trip shell or a plan/day slice by querying explicit sort-key
   prefixes.
6. Resolve a route object with both its trip ID and object ID. An unscoped
   object-ID lookup is not provided.
7. Page comments and audit events by sort key with an explicit limit and opaque
   continuation key.

An index can make navigation fast, but the direct membership read is the source
of truth. A removed member therefore loses access immediately after the
successful membership write, even while a stale `gsi1` entry is still visible.

## 6. Replacing relational constraints

DynamoDB has no foreign keys. Correctness moves into keys, conditions, and
transactions rather than into handler convention.

- **Uniqueness:** encode the unique value in a primary key and create it with
  `attribute_not_exists`. Vote keys include the user ID, so one user has at most
  one current vote per poll.
- **Membership:** every trip mutation transaction includes a condition on that
  trip's `MEMBER#<actor_id>` item and the required stored role.
- **At least one leader:** membership changes conditionally update a leader
  count on trip metadata; removing or demoting a leader requires the stored
  count to remain at least one.
- **Plan compare-and-swap:** applying a proposal conditions the trip metadata on
  the expected current plan version, writes the next version, closes the
  proposal or poll, and appends the audit event atomically.
- **Ledger corrections:** updating or deleting an expense checks the current
  membership role, validates the complete resulting ledger row, reconciles any
  stop link, and appends an actor-attributed audit event in one transaction.
  The request cannot write the frozen exchange rate directly.
- **Referential ownership:** related records share the trip partition and every
  command carries the authoritative trip ID from the route. Repositories never
  fetch an arbitrary object first and infer its tenant afterward.
- **Idempotency:** externally retried mutations claim an idempotency item in the
  same transaction and return the previously committed result on replay.
- **Deletion:** trip deletion first marks the aggregate unavailable, then a
  bounded background cleanup removes its items. Authorization treats the
  tombstone as deleted throughout cleanup.

`TransactWriteItems` is the boundary for a governance decision and all effects
it authorizes. A transaction cannot contain more than 100 unique items or 4 MiB,
so commands and change sets are capped below both limits. No transaction writes
two actions against the same item; conditions that govern an update are placed
on that update when necessary.

Conditional and transactional cancellation is an expected domain outcome, not
a generic server crash. Repositories translate it into conflict, stale-version,
duplicate, or forbidden errors without returning AWS response details to the
client.

## 7. Consistency rules

Use strong consistency when a stale value could change identity,
authorization, money, or governance:

- login claims, profiles, provisioning, and email uniqueness;
- direct trip membership and role checks;
- current plan version and proposal application; and
- ledger balances or settlement state used for a write decision.

Eventually consistent reads are suitable for browse-only lists, activity
feeds, and profile decoration where temporary staleness cannot grant access or
apply a mutation. A write response may return the committed object directly so
the UI does not need to wait for an index to catch up.

## 8. Capacity and cost controls

Use provisioned capacity and the Standard table class to use the ongoing free
tier. The public module starts the table at 10 RCU / 5 WCU and `gsi1` at
5 RCU / 5 WCU. It rejects a combined allocation above 25 RCU or 25 WCU unless
the private root explicitly disables the guard. That guard cannot see other
tables, so the deployment still checks account-wide usage before applying.
Measure real item sizes and consumed capacity before enabling autoscaling. Any
autoscaling maximum must be deliberate and paired with a budget alert. The free
allowance is per Region and payer account, not per table.

Strong reads and transactions consume more capacity than eventual reads and
ordinary writes. That is intentional on security-sensitive paths; correctness
is not traded for a small capacity saving. Pagination and command-size limits
keep individual requests bounded.

When the private deployment supplies at least one SNS destination, the public
module creates alarms for base-table and `gsi1` read/write throttle events,
plus Lambda errors, throttles, Function URL 5xx responses, and near-limit
concurrency that expose propagated storage failures. With no destination it
creates no alarms. Before trip storage goes live, the private deployment also
adds an AWS Budget, and operational dashboards and alert rules cover consumed
capacity, transaction conflicts, persistent system errors, recovery status,
and spend.

## 9. Security and IAM

DynamoDB encrypts table, index, stream, and backup data at rest. The SDK uses
TLS in transit. For this small application, the AWS-owned encryption key avoids
another billable customer-managed key; this choice can be revisited if the data
classification changes.

The Lambda execution role is separate from deployment and human administration
roles. Its policy is restricted to the deployed table and `gsi1`, and only to
the operations repositories actually use:

```text
dynamodb:GetItem
dynamodb:PutItem
dynamodb:UpdateItem
dynamodb:DeleteItem
dynamodb:Query
dynamodb:BatchGetItem
dynamodb:TransactGetItems
dynamodb:TransactWriteItems
```

`Scan`, table creation/deletion, backup administration, policy changes, and
wildcard resources are not runtime permissions. The public module derives the
exact table and index ARNs from the resources it creates; the private root's
provider supplies the account and Region without committing either here.

Application logs never include raw items, raw keys, email addresses, AWS
credentials, provider response bodies, or expressions populated with user
data. Metrics use operation and error categories rather than tenant or item
identifiers.

## 10. Recovery, retention, and schema evolution

Before real trip data is accepted, enable deletion protection and point-in-time
recovery for an agreed 1–35 day window. PITR restores into a new table; the
runbook must therefore cover restoring, verifying counts and representative
aggregates, applying tags and protection settings, changing the Lambda table
environment variable, and rolling back the cutover. A restore drill is part of
the production gate.

PITR is a recovery control, not immutable archival, and it has a separate cost.
Longer retention uses deliberate on-demand backups only if the group's data
retention policy requires them.

Every item carries `schema_version`. Readers may support the current and prior
version during a rolling migration; writers emit only the current version.
Migrations are restartable, idempotent, rate-limited jobs run by a separate role.
They never run implicitly during a Lambda request or cold start.

DynamoDB TTL is reserved for truly expiring records such as token metadata or
idempotency claims. Expiry is always checked by application time because TTL
deletion is asynchronous and cannot revoke access by itself.

## 11. Testing and deployment gate

Unit tests use the official AWS SDK mock interceptor to assert exact key,
consistency, item, and condition-expression behaviour without AWS credentials.
Terraform mock-provider tests separately assert the physical key schema,
protection and capacity defaults, production environment variables, exact-table
IAM without `Scan`, and core alarms without contacting AWS.
Repository integration tests additionally run against an isolated DynamoDB
table before trip APIs ship. They cover concurrent claims, atomic profile/claim
creation, transaction cancellation, future email-claim replacement, stale
versions, pagination, malformed records, and cross-trip denial.

Production startup requires a non-empty `ITINERA_DYNAMODB_TABLE` and a valid AWS
region. Development authentication deliberately uses the in-memory repository
so normal frontend work does not require cloud credentials or mutate durable
data.

Changing the persistence provider does not alter an HTTP request or response,
so [`openapi.yaml`](openapi.yaml) needs no DynamoDB-specific fields. Storage
details remain behind repository ports. A future user-facing email-change route
will require its own OpenAPI operation, but this storage refactor does not add
one or alter `/me`.

## 12. AWS references

- [DynamoDB pricing and ongoing free tier](https://aws.amazon.com/dynamodb/pricing/)
- [DynamoDB transactions](https://docs.aws.amazon.com/amazondynamodb/latest/developerguide/transactions.html)
- [DynamoDB service constraints](https://docs.aws.amazon.com/amazondynamodb/latest/developerguide/Constraints.html)
- [DynamoDB security best practices](https://docs.aws.amazon.com/amazondynamodb/latest/developerguide/best-practices-security-preventative.html)
- [DynamoDB backup and restore](https://docs.aws.amazon.com/amazondynamodb/latest/developerguide/Backup-and-Restore.html)
