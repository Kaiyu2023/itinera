-- Structural proposals, polls, ballots, and reciprocal plan/audit provenance.
-- Rebuilding the referenced plans table requires the stopped application to
-- disable foreign keys on this one migration connection before BEGIN.

PRAGMA foreign_keys = OFF;
BEGIN IMMEDIATE;

-- Duplicate the immutable candidate/place binding onto the authoritative audit
-- row. Strict readers compare it with proposal_content_edits so a valid-looking
-- link cannot be rebound to another candidate's adopted place.
ALTER TABLE content_edits
ADD COLUMN proposal_candidate_place_id TEXT
    CHECK (
        (
            proposal_candidate_place_id IS NULL
            AND NOT (
                entity = 'candidate'
                AND field = 'status'
                AND (
                    old_value_json = '"in_plan"'
                    OR new_value_json = '"in_plan"'
                )
            )
        )
        OR (
            proposal_candidate_place_id IS NOT NULL
            AND length(proposal_candidate_place_id) BETWEEN 1 AND 200
            AND entity = 'candidate'
            AND field = 'status'
            AND (
                old_value_json = '"in_plan"'
                OR new_value_json = '"in_plan"'
            )
        )
    );

CREATE TABLE proposals (
    trip_id TEXT NOT NULL,
    id TEXT NOT NULL
        CHECK (length(id) BETWEEN 1 AND 200),
    created_by TEXT NOT NULL,
    source_kind TEXT NOT NULL
        CHECK (source_kind IN ('web', 'service')),
    source_service_id TEXT
        CHECK (source_service_id IS NULL OR length(source_service_id) BETWEEN 1 AND 200),
    source_service_name TEXT
        CHECK (source_service_name IS NULL OR length(source_service_name) BETWEEN 1 AND 200),
    title TEXT NOT NULL
        CHECK (length(title) BETWEEN 1 AND 200 AND trim(title) = title),
    rationale TEXT NOT NULL
        CHECK (length(rationale) <= 4000 AND trim(rationale) = rationale),
    change_set_json TEXT NOT NULL
        CHECK (
            json_valid(change_set_json)
            AND json_type(change_set_json) = 'object'
            AND length(CAST(change_set_json AS BLOB)) <= 262144
        ),
    route TEXT NOT NULL
        CHECK (route IN ('leader_approval', 'poll')),
    status TEXT NOT NULL
        CHECK (status IN ('pending', 'rejected', 'applied', 'stale')),
    decision_kind TEXT
        CHECK (decision_kind IS NULL OR decision_kind IN ('leader', 'poll')),
    decision_user_id TEXT,
    decision_poll_id TEXT
        CHECK (decision_poll_id IS NULL OR length(decision_poll_id) BETWEEN 1 AND 200),
    rejection_reason TEXT
        CHECK (
            rejection_reason IS NULL
            OR (
                length(rejection_reason) BETWEEN 1 AND 2000
                AND trim(rejection_reason) = rejection_reason
            )
        ),
    created_at TEXT NOT NULL
        CHECK (
            length(created_at) BETWEEN 20 AND 64
            AND substr(created_at, -1) = 'Z'
            AND datetime(created_at) IS NOT NULL
        ),
    revision INTEGER NOT NULL
        CHECK (revision BETWEEN 1 AND 9223372036854775807),
    PRIMARY KEY (trip_id, id),
    CHECK (
        (source_kind = 'web' AND source_service_id IS NULL AND source_service_name IS NULL)
        OR (
            source_kind = 'service'
            AND source_service_id IS NOT NULL
            AND source_service_name IS NOT NULL
        )
    ),
    CHECK (
        (decision_kind IS NULL AND decision_user_id IS NULL AND decision_poll_id IS NULL)
        OR (
            decision_kind = 'leader'
            AND decision_user_id IS NOT NULL
            AND decision_poll_id IS NULL
        )
        OR (
            decision_kind = 'poll'
            AND decision_user_id IS NULL
            AND decision_poll_id IS NOT NULL
        )
    ),
    CHECK (
        (status = 'rejected' AND rejection_reason IS NOT NULL)
        OR (status <> 'rejected' AND rejection_reason IS NULL)
    ),
    CHECK (
        (
            route = 'leader_approval'
            AND (
                (status = 'pending' AND decision_kind IS NULL)
                OR (status IN ('applied', 'rejected') AND decision_kind = 'leader')
                OR (status = 'stale' AND (decision_kind IS NULL OR decision_kind = 'leader'))
            )
        )
        OR (route = 'poll' AND decision_kind = 'poll')
    ),
    FOREIGN KEY (trip_id) REFERENCES trips(id) ON DELETE RESTRICT,
    FOREIGN KEY (created_by) REFERENCES users(id) ON DELETE RESTRICT,
    FOREIGN KEY (decision_user_id) REFERENCES users(id) ON DELETE RESTRICT,
    FOREIGN KEY (trip_id, decision_poll_id)
        REFERENCES polls(trip_id, id)
        DEFERRABLE INITIALLY DEFERRED
) STRICT;

CREATE INDEX proposals_by_trip_created
    ON proposals(trip_id, created_at DESC, id DESC);

CREATE TABLE polls (
    trip_id TEXT NOT NULL,
    id TEXT NOT NULL
        CHECK (length(id) BETWEEN 1 AND 200),
    created_by TEXT NOT NULL,
    kind TEXT NOT NULL
        CHECK (kind IN ('decision', 'plan_change')),
    replaces_poll_id TEXT
        CHECK (replaces_poll_id IS NULL OR length(replaces_poll_id) BETWEEN 1 AND 200),
    title TEXT NOT NULL
        CHECK (length(title) BETWEEN 1 AND 200 AND trim(title) = title),
    description TEXT NOT NULL
        CHECK (length(description) <= 4000 AND trim(description) = description),
    created_at TEXT NOT NULL
        CHECK (
            length(created_at) BETWEEN 20 AND 64
            AND substr(created_at, -1) = 'Z'
            AND datetime(created_at) IS NOT NULL
        ),
    opens_at TEXT
        CHECK (
            opens_at IS NULL
            OR (
                length(opens_at) BETWEEN 20 AND 64
                AND substr(opens_at, -1) = 'Z'
                AND datetime(opens_at) IS NOT NULL
            )
        ),
    closes_at TEXT NOT NULL
        CHECK (
            length(closes_at) BETWEEN 20 AND 64
            AND substr(closes_at, -1) = 'Z'
            AND datetime(closes_at) IS NOT NULL
        ),
    decided_at TEXT
        CHECK (
            decided_at IS NULL
            OR (
                length(decided_at) BETWEEN 20 AND 64
                AND substr(decided_at, -1) = 'Z'
                AND datetime(decided_at) IS NOT NULL
            )
        ),
    quorum INTEGER NOT NULL
        CHECK (quorum BETWEEN 1 AND 1000),
    allow_multi INTEGER NOT NULL
        CHECK (allow_multi IN (0, 1)),
    status TEXT NOT NULL
        CHECK (status IN ('draft', 'scheduled', 'open', 'passed', 'failed', 'expired')),
    resolution_note TEXT
        CHECK (
            resolution_note IS NULL
            OR (
                length(resolution_note) BETWEEN 1 AND 2000
                AND trim(resolution_note) = resolution_note
            )
        ),
    revision INTEGER NOT NULL
        CHECK (revision BETWEEN 1 AND 9223372036854775807),
    PRIMARY KEY (trip_id, id),
    UNIQUE (trip_id, replaces_poll_id),
    CHECK (replaces_poll_id IS NULL OR (kind = 'plan_change' AND replaces_poll_id <> id)),
    CHECK (
        (status = 'draft' AND opens_at IS NULL AND decided_at IS NULL AND resolution_note IS NULL)
        OR (status = 'scheduled' AND opens_at IS NOT NULL AND decided_at IS NULL AND resolution_note IS NULL)
        OR (status = 'open' AND decided_at IS NULL AND resolution_note IS NULL)
        OR (status = 'passed' AND decided_at IS NOT NULL AND resolution_note IS NULL)
        OR (status IN ('failed', 'expired') AND decided_at IS NOT NULL AND resolution_note IS NOT NULL)
    ),
    FOREIGN KEY (trip_id) REFERENCES trips(id) ON DELETE RESTRICT,
    FOREIGN KEY (created_by) REFERENCES users(id) ON DELETE RESTRICT,
    FOREIGN KEY (trip_id, replaces_poll_id)
        REFERENCES polls(trip_id, id)
        DEFERRABLE INITIALLY DEFERRED
) STRICT;

CREATE INDEX polls_by_trip_created
    ON polls(trip_id, created_at DESC, id DESC);

CREATE TABLE poll_options (
    trip_id TEXT NOT NULL,
    poll_id TEXT NOT NULL
        CHECK (length(poll_id) BETWEEN 1 AND 200),
    id TEXT NOT NULL
        CHECK (length(id) BETWEEN 1 AND 200),
    position INTEGER NOT NULL
        CHECK (position BETWEEN 0 AND 5),
    label TEXT NOT NULL
        CHECK (length(label) BETWEEN 1 AND 200 AND trim(label) = label),
    proposal_id TEXT
        CHECK (proposal_id IS NULL OR length(proposal_id) BETWEEN 1 AND 200),
    PRIMARY KEY (trip_id, poll_id, id),
    UNIQUE (trip_id, poll_id, position),
    UNIQUE (trip_id, poll_id, label),
    UNIQUE (trip_id, poll_id, proposal_id),
    FOREIGN KEY (trip_id, poll_id)
        REFERENCES polls(trip_id, id) ON DELETE RESTRICT,
    FOREIGN KEY (trip_id, proposal_id)
        REFERENCES proposals(trip_id, id)
        DEFERRABLE INITIALLY DEFERRED
) STRICT;

CREATE TABLE poll_ballots (
    trip_id TEXT NOT NULL,
    poll_id TEXT NOT NULL
        CHECK (length(poll_id) BETWEEN 1 AND 200),
    user_id TEXT NOT NULL,
    voted_at TEXT NOT NULL
        CHECK (
            length(voted_at) BETWEEN 20 AND 64
            AND substr(voted_at, -1) = 'Z'
            AND datetime(voted_at) IS NOT NULL
        ),
    revision INTEGER NOT NULL
        CHECK (revision BETWEEN 1 AND 9223372036854775807),
    PRIMARY KEY (trip_id, poll_id, user_id),
    FOREIGN KEY (trip_id, poll_id)
        REFERENCES polls(trip_id, id) ON DELETE RESTRICT,
    FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE RESTRICT
) STRICT;

CREATE TABLE poll_ballot_options (
    trip_id TEXT NOT NULL,
    poll_id TEXT NOT NULL
        CHECK (length(poll_id) BETWEEN 1 AND 200),
    user_id TEXT NOT NULL,
    option_id TEXT NOT NULL
        CHECK (length(option_id) BETWEEN 1 AND 200),
    PRIMARY KEY (trip_id, poll_id, user_id, option_id),
    FOREIGN KEY (trip_id, poll_id, user_id)
        REFERENCES poll_ballots(trip_id, poll_id, user_id) ON DELETE RESTRICT,
    FOREIGN KEY (trip_id, poll_id, option_id)
        REFERENCES poll_options(trip_id, poll_id, id) ON DELETE RESTRICT
) STRICT;

-- Every proposal-owned candidate status audit row is tied to the applied
-- proposal and the immutable candidate place snapshot whose adoption changed.
CREATE TABLE proposal_content_edits (
    trip_id TEXT NOT NULL,
    edit_id TEXT NOT NULL
        CHECK (length(edit_id) BETWEEN 1 AND 200),
    proposal_id TEXT NOT NULL
        CHECK (length(proposal_id) BETWEEN 1 AND 200),
    candidate_id TEXT NOT NULL
        CHECK (length(candidate_id) BETWEEN 1 AND 200),
    candidate_place_id TEXT NOT NULL
        CHECK (length(candidate_place_id) BETWEEN 1 AND 200),
    PRIMARY KEY (trip_id, edit_id),
    UNIQUE (trip_id, proposal_id, candidate_id),
    FOREIGN KEY (trip_id, edit_id)
        REFERENCES content_edits(trip_id, id)
        DEFERRABLE INITIALLY DEFERRED,
    FOREIGN KEY (trip_id, proposal_id)
        REFERENCES proposals(trip_id, id) ON DELETE RESTRICT,
    FOREIGN KEY (trip_id, candidate_id)
        REFERENCES candidates(trip_id, id) ON DELETE RESTRICT,
    FOREIGN KEY (trip_id, candidate_place_id)
        REFERENCES trip_places(trip_id, id) ON DELETE RESTRICT
) STRICT;

-- Add the deferred same-trip proposal parent now that it exists. Child tables
-- and the trip pointer keep referencing the final `plans` name.
CREATE TABLE plans_v4 (
    trip_id TEXT NOT NULL,
    version INTEGER NOT NULL
        CHECK (version BETWEEN 1 AND 4294967295),
    id TEXT NOT NULL
        CHECK (length(id) BETWEEN 1 AND 200),
    created_from_proposal_id TEXT
        CHECK (
            created_from_proposal_id IS NULL
            OR length(created_from_proposal_id) BETWEEN 1 AND 200
        ),
    created_at TEXT NOT NULL
        CHECK (
            length(created_at) BETWEEN 20 AND 64
            AND substr(created_at, -1) = 'Z'
            AND datetime(created_at) IS NOT NULL
        ),
    applied_change_set_json TEXT
        CHECK (
            applied_change_set_json IS NULL
            OR (
                json_valid(applied_change_set_json)
                AND json_type(applied_change_set_json) = 'object'
                AND length(CAST(applied_change_set_json AS BLOB)) <= 262144
            )
        ),
    application_entity_ids_json TEXT
        CHECK (
            application_entity_ids_json IS NULL
            OR (
                json_valid(application_entity_ids_json)
                AND json_type(application_entity_ids_json) = 'array'
                AND length(CAST(application_entity_ids_json AS BLOB)) <= 16384
            )
        ),
    structural_audits_json TEXT
        CHECK (
            structural_audits_json IS NULL
            OR (
                json_valid(structural_audits_json)
                AND json_type(structural_audits_json) = 'array'
                AND length(CAST(structural_audits_json AS BLOB)) <= 262144
            )
        ),
    base_structure_hash TEXT
        CHECK (
            base_structure_hash IS NULL
            OR (
                length(base_structure_hash) = 64
                AND base_structure_hash NOT GLOB '*[^0-9a-f]*'
            )
        ),
    structure_hash TEXT
        CHECK (
            structure_hash IS NULL
            OR (
                length(structure_hash) = 64
                AND structure_hash NOT GLOB '*[^0-9a-f]*'
            )
        ),
    revision INTEGER NOT NULL
        CHECK (revision BETWEEN 1 AND 9223372036854775807),
    PRIMARY KEY (trip_id, version),
    UNIQUE (trip_id, id),
    UNIQUE (trip_id, id, version),
    CHECK (
        (version = 1 AND created_from_proposal_id IS NULL)
        OR (version > 1 AND created_from_proposal_id IS NOT NULL)
    ),
    CHECK (
        (
            version = 1
            AND applied_change_set_json IS NULL
            AND application_entity_ids_json IS NULL
            AND structural_audits_json IS NULL
            AND base_structure_hash IS NULL
        )
        OR (
            version > 1
            AND applied_change_set_json IS NOT NULL
            AND application_entity_ids_json IS NOT NULL
            AND structural_audits_json IS NOT NULL
            AND base_structure_hash IS NOT NULL
            AND structure_hash IS NOT NULL
        )
    ),
    FOREIGN KEY (trip_id) REFERENCES trips(id) ON DELETE RESTRICT,
    FOREIGN KEY (trip_id, created_from_proposal_id)
        REFERENCES proposals(trip_id, id)
        DEFERRABLE INITIALLY DEFERRED
) STRICT;

INSERT INTO plans_v4 (
    trip_id, version, id, created_from_proposal_id, created_at,
    applied_change_set_json, application_entity_ids_json, structural_audits_json,
    base_structure_hash, structure_hash, revision
)
SELECT trip_id, version, id, created_from_proposal_id, created_at,
       NULL, NULL, NULL, NULL, NULL, revision
FROM plans;

DROP TABLE plans;
ALTER TABLE plans_v4 RENAME TO plans;

CREATE UNIQUE INDEX plans_by_creating_proposal
    ON plans(trip_id, created_from_proposal_id)
    WHERE created_from_proposal_id IS NOT NULL;

CREATE TEMP TABLE migration_0004_fk_assertion (
    violation_count INTEGER NOT NULL CHECK (violation_count = 0)
) STRICT;
INSERT INTO migration_0004_fk_assertion (violation_count)
SELECT COUNT(*) FROM pragma_foreign_key_check;
DROP TABLE migration_0004_fk_assertion;

PRAGMA user_version = 4;
COMMIT;
PRAGMA foreign_keys = ON;
PRAGMA foreign_key_check;
PRAGMA integrity_check;
