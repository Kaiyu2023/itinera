-- Immutable, field-level content history and reciprocal safe-revert
-- provenance. Service-source columns are staged now, but their composite
-- owner/service foreign key is added only after service identities exist.

CREATE TABLE content_edits (
    trip_id TEXT NOT NULL,
    id TEXT NOT NULL
        CHECK (length(id) BETWEEN 1 AND 200),
    entity TEXT NOT NULL
        CHECK (entity IN ('stop', 'day', 'candidate', 'notice', 'trip')),
    entity_id TEXT NOT NULL
        CHECK (length(entity_id) BETWEEN 1 AND 200),
    field TEXT NOT NULL
        CHECK (
            (entity = 'trip' AND field = 'status')
            OR (entity = 'candidate' AND field IN ('place', 'pitch', 'tags', 'status'))
            OR (entity = 'day' AND field IN ('windowStart', 'windowEnd', 'cityHint'))
            OR (entity = 'stop' AND field IN ('plannedArrival', 'durationMin', 'notes', 'booking'))
            OR (entity = 'notice' AND field IN ('title', 'body', 'pinned', 'sourceUrl', 'status', 'audience'))
        ),
    old_value_json TEXT NOT NULL
        CHECK (
            json_valid(old_value_json)
            AND length(CAST(old_value_json AS BLOB)) <= 4194304
        ),
    new_value_json TEXT NOT NULL
        CHECK (
            json_valid(new_value_json)
            AND length(CAST(new_value_json AS BLOB)) <= 4194304
        ),
    author_id TEXT NOT NULL,
    source_kind TEXT NOT NULL
        CHECK (source_kind IN ('web', 'service')),
    source_service_id TEXT
        CHECK (source_service_id IS NULL OR length(source_service_id) BETWEEN 1 AND 200),
    source_service_name TEXT
        CHECK (source_service_name IS NULL OR length(source_service_name) BETWEEN 1 AND 200),
    status TEXT NOT NULL
        CHECK (status IN ('applied', 'reverted')),
    created_at TEXT NOT NULL
        CHECK (
            length(created_at) BETWEEN 20 AND 64
            AND substr(created_at, -1) = 'Z'
            AND datetime(created_at) IS NOT NULL
        ),
    reverted_by TEXT,
    reverted_at TEXT
        CHECK (
            reverted_at IS NULL
            OR (
                length(reverted_at) BETWEEN 20 AND 64
                AND substr(reverted_at, -1) = 'Z'
                AND datetime(reverted_at) IS NOT NULL
            )
        ),
    revert_edit_id TEXT
        CHECK (revert_edit_id IS NULL OR length(revert_edit_id) BETWEEN 1 AND 200),
    reverts_edit_id TEXT
        CHECK (reverts_edit_id IS NULL OR length(reverts_edit_id) BETWEEN 1 AND 200),
    revision INTEGER NOT NULL
        CHECK (revision BETWEEN 1 AND 9223372036854775807),
    PRIMARY KEY (trip_id, id),
    CHECK (old_value_json <> new_value_json),
    CHECK (
        (source_kind = 'web' AND source_service_id IS NULL AND source_service_name IS NULL)
        OR (
            source_kind = 'service'
            AND source_service_id IS NOT NULL
            AND source_service_name IS NOT NULL
        )
    ),
    CHECK (
        (status = 'applied' AND reverted_by IS NULL AND reverted_at IS NULL AND revert_edit_id IS NULL)
        OR (
            status = 'reverted'
            AND reverted_by IS NOT NULL
            AND reverted_at IS NOT NULL
            AND revert_edit_id IS NOT NULL
        )
    ),
    CHECK (revert_edit_id IS NULL OR revert_edit_id <> id),
    CHECK (reverts_edit_id IS NULL OR reverts_edit_id <> id),
    FOREIGN KEY (trip_id) REFERENCES trips(id) ON DELETE RESTRICT,
    FOREIGN KEY (author_id) REFERENCES users(id) ON DELETE RESTRICT,
    FOREIGN KEY (reverted_by) REFERENCES users(id) ON DELETE RESTRICT,
    FOREIGN KEY (trip_id, revert_edit_id)
        REFERENCES content_edits(trip_id, id)
        DEFERRABLE INITIALLY DEFERRED,
    FOREIGN KEY (trip_id, reverts_edit_id)
        REFERENCES content_edits(trip_id, id)
        DEFERRABLE INITIALLY DEFERRED
) STRICT;

CREATE INDEX content_edits_by_trip_created
    ON content_edits(trip_id, created_at DESC, id DESC);

CREATE UNIQUE INDEX content_edits_unique_revert_target
    ON content_edits(trip_id, revert_edit_id)
    WHERE revert_edit_id IS NOT NULL;

CREATE UNIQUE INDEX content_edits_unique_original_target
    ON content_edits(trip_id, reverts_edit_id)
    WHERE reverts_edit_id IS NOT NULL;

PRAGMA user_version = 3;
PRAGMA foreign_key_check;
PRAGMA integrity_check;
