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

| Attribute          | Value                                                         |
| ------------------ | ------------------------------------------------------------- |
| `pk`               | `USER#<user_id>`                                              |
| `sk`               | `PROFILE`                                                     |
| `entity_type`      | `USER_PROFILE`                                                |
| `schema_version`   | `1`                                                           |
| `user_id`          | generated opaque ID                                           |
| `email`            | current canonical email                                       |
| `display_name`     | optional user-authored name                                   |
| `membership_count` | app-wide trip count maintained for login-revocation decisions |

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

The implemented trip repository places a trip's related records under
`pk = TRIP#<trip_id>`. The sort key names both the entity type and its identity.
Current and reserved keys are:

| Entity                        | `sk`                                                   | Secondary access path          |
| ----------------------------- | ------------------------------------------------------ | ------------------------------ |
| Trip metadata                 | `META`                                                 | —                              |
| Membership                    | `MEMBER#<user_id>`                                     | `gsi1` lists a user's trips    |
| Pending/accepted invite       | `INVITE#<SHA-256 of canonical email>`                  | hashed base-table invitee copy |
| Candidate                     | `CANDIDATE#<candidate_id>`                             | —                              |
| Candidate/plan place snapshot | `PLACE#<place_id>`                                     | —                              |
| Plan metadata                 | `PLAN#<version>#META`                                  | —                              |
| Day                           | `PLAN#<version>#DAY#<date>#<day_id>`                   | —                              |
| Stop                          | `PLAN#<version>#DAY#<date>#STOP#<sequence>#<stop_id>`  | —                              |
| Proposal                      | `PROPOSAL#<proposal_id>`                               | optional status queue          |
| Poll                          | `POLL#<poll_id>`                                       | optional status queue          |
| Vote                          | `POLL#<poll_id>#VOTE#<user_id>`                        | —                              |
| Expense                       | `EXPENSE#<expense_id>`                                 | —                              |
| Comment                       | `THREAD#<thread_id>#COMMENT#<created_at>#<comment_id>` | —                              |
| Audit event                   | `AUDIT#<created_at>#<event_id>`                        | —                              |

Fixed-width plan versions and sequence numbers preserve numeric ordering when
sorted as strings. Timestamps are UTC ISO-8601 values followed by a unique ID,
so events created in the same instant cannot overwrite one another.

Content audit payloads use the API's `Edit` shape. Schema-version 1 rows written
before safe revert have no provenance members; readers default those members to
null. A successful revert updates the original row to `status = reverted` with
`revertedBy`, `revertedAt`, and `revertEditId`, then creates a second `applied`
row whose `revertsEditId` points to the original. Neither row is deleted.

Pending invite discovery is the one deliberate mirrored access path. A small
pointer lives at `pk = INVITEE#<SHA-256 of canonical email>`,
`sk = TRIP#<trip_id>`. `/me` queries that exact partition with strong
consistency, then one transaction creates `MEMBER#<user_id>`, increments the
trip and user membership counts, marks the trip invite accepted, and deletes
the pointer. Raw email never appears in either key. This avoids trusting an
eventually consistent GSI at the moment login becomes authorization.
An accepted invite remains as the current invite state until a later invite for
the same address replaces it with a new pending revision, so a removed member
can be invited back. Query helpers follow DynamoDB continuation keys rather than
treating a full page as an outage, preventing a large invite set from blocking
`/me` or silently truncating a trip collection. Invite acceptance retries
conditional contention and treats an invite another concurrent `/me` request
already accepted as success, so account bootstrap remains idempotent.

Each implemented record stores its typed payload as JSON in `data`, alongside
the explicit keys, `entity_type`, `schema_version`, and numeric `revision`.
Fields needed by conditions or indexes—such as membership role, counts, current
plan version, and `gsi1` keys—are also top-level attributes. Readers validate
the key, entity type, schema version, revision, payload, and embedded ownership
before returning a domain object. Mutations replace the typed payload only with
an expected-revision condition, so the JSON envelope does not weaken optimistic
concurrency.

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
7. Read audit events newest-first by the `AUDIT#` prefix in strongly consistent
   pages. The current HTTP contract accumulates at most 1,000 records into one
   array and returns a conflict beyond that hard memory/RCU ceiling. Cursor
   pagination will expose the same continuation key, and an edit-ID lookup row
   will make reverts a bounded direct read, before larger histories are
   supported. Strong consistency applies to each page rather than to a
   multi-page snapshot, so reciprocal-provenance validation retries the entire
   bounded query once when the first complete graph alone is inconsistent. A
   second inconsistent graph fails closed as corrupt data.
8. Accept pending invitations by strongly querying one hashed invitee
   partition, never by scanning email attributes or trusting an index.

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
- **Current-plan content edits:** a day or stop update conditions both its child
  revision and the trip metadata revision that named that plan as current. An
  in-flight edit therefore conflicts instead of mutating a version that Phase 3
  has just made historical.
- **Content revert:** the caller supplies only a trip-scoped server edit id.
  The repository first reads direct membership strongly, queries audit rows
  only inside `TRIP#<trip_id>`, validates the stored edit and an explicit
  entity/field allowlist, and compares the live field with the audit
  `newValue`. Its transaction condition-checks the editor role, replaces the
  target only when both `revision` and the exact serialized `data` payload still
  match the strong read, protects the current plan metadata for day/stop edits,
  conditionally replaces the original audit revision, and create-only appends
  the compensation. Conditioning the whole payload is deliberately stronger
  than conditioning only the named field and works with existing JSON-envelope
  rows without a migration. Candidate-place reverts repoint to the immutable
  previous candidate-owned snapshot and guard both snapshot records; they do
  not mutate either place. Conditional contention reloads membership and the
  original edit: a concurrent successful revert completes the retry
  idempotently, while any other stale state returns a conflict. Until the
  cursor-and-lookup follow-up lands, both list and edit lookup stop after 1,000
  audit rows rather than allowing an unbounded partition read. Because a
  transaction may commit between strongly consistent query pages, an otherwise
  valid reciprocal-provenance failure causes one complete bounded reread before
  the repository reports corruption.
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
- current plan version and proposal application;
- audit records and live entity revisions used by safe revert; and
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
Mocked-SDK repository tests cover atomic profile/claim/trip creation, strong
membership reads, pagination, conditional mutation authorization, malformed
records, cross-trip denial, the exact safe-revert transaction, stale and
concurrent reverts, idempotent repeats, and unsupported targets without
contacting AWS. They also cover the audit safety ceiling, reciprocal provenance,
invalid timestamps, and day rows interleaved with their nested stop keys. Before the private
environment accepts trip data, the deployment verification step adds live
isolated-table checks for concurrent claims, cancellation behavior, stale
versions, and pagination against DynamoDB itself.

Production startup requires a non-empty `ITINERA_DYNAMODB_TABLE` and a valid AWS
region. The same one-table adapter implements `UserRepo`, `TripRepo`, and the
separate `ContentHistoryRepo`, so one SDK client and connection pool are shared.
Development authentication changes
only identity verification: it does not select another persistence provider.
Until a local DynamoDB environment is added, every runtime therefore requires
an explicitly configured DynamoDB table and AWS SDK configuration. The
development Access-policy adapter is a no-op only inside the explicitly enabled
`dev-auth` build; production invite grants and place-catalog lookups return
service unavailable until the credentialled step 4 adapters replace their
fail-closed placeholders. Stateful fakes exist only inside API integration-test
targets and cannot be linked into or selected by the application binary. Never
point a `dev-auth` build at a shared or production table: that mode deliberately
accepts an asserted email without Cloudflare verification.

The trip adapter is divided by capability under `dynamodb/trip_repo/`:
`records` owns persisted shapes and key codecs; `store` owns generic strongly
consistent reads and transaction primitives; `access` owns direct-membership
authorization; and `trips`, `memberships`, `candidates`, and `plans` each keep
their complete DynamoDB transactions beside the use cases they implement.
`mod.rs` is a small explicit `TripRepo` facade. This separation does not divide
one-table transactions or make GSI results authoritative for access control.

Content history is a separate repository port and adapter under
`dynamodb/history_repo/`; it shares only the table record codec and owns its
direct authorization, strict audit decoder, allowlist, and revert transaction.
It is not another method family added to `TripRepo`.

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
