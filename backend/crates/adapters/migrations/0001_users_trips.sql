-- First SQLite capability slice: stable users plus trips, memberships, and
-- invitations. Later capability migrations add their own tables and the
-- deferred current-plan foreign key once plans exist.

CREATE TABLE users (
    id TEXT PRIMARY KEY NOT NULL
        CHECK (length(id) BETWEEN 1 AND 200),
    email TEXT NOT NULL UNIQUE
        CHECK (
            length(email) BETWEEN 3 AND 320
            AND email = lower(trim(email))
            AND instr(email, '@') > 1
            AND instr(substr(email, instr(email, '@') + 1), '@') = 0
            AND instr(email, '@') < length(email)
        ),
    display_name TEXT
        CHECK (display_name IS NULL OR length(display_name) BETWEEN 1 AND 200),
    revision INTEGER NOT NULL
        CHECK (revision BETWEEN 1 AND 9223372036854775807)
) STRICT;

CREATE TABLE user_email_claims (
    email_digest TEXT PRIMARY KEY NOT NULL
        CHECK (
            length(email_digest) = 64
            AND email_digest NOT GLOB '*[^0-9a-f]*'
        ),
    user_id TEXT NOT NULL UNIQUE,
    FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE RESTRICT
) STRICT;

CREATE TABLE trips (
    id TEXT PRIMARY KEY NOT NULL
        CHECK (length(id) BETWEEN 1 AND 200),
    name TEXT NOT NULL
        CHECK (length(name) BETWEEN 1 AND 120),
    cover_photo_url TEXT
        CHECK (cover_photo_url IS NULL OR length(cover_photo_url) <= 2048),
    accent_color TEXT
        CHECK (accent_color IS NULL OR length(accent_color) BETWEEN 1 AND 128),
    stop_kind_labels_json TEXT
        CHECK (
            stop_kind_labels_json IS NULL
            OR (
                json_valid(stop_kind_labels_json)
                AND json_type(stop_kind_labels_json) = 'object'
                AND length(stop_kind_labels_json) <= 4096
            )
        ),
    status TEXT NOT NULL
        CHECK (status IN ('dreaming', 'planning', 'booked', 'ongoing', 'done')),
    start_date TEXT NOT NULL
        CHECK (
            length(start_date) = 10
            AND start_date GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]'
            AND date(start_date) = start_date
        ),
    end_date TEXT NOT NULL
        CHECK (
            length(end_date) = 10
            AND end_date GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]'
            AND date(end_date) = end_date
            AND CAST(julianday(end_date) - julianday(start_date) AS INTEGER)
                BETWEEN 0 AND 89
        ),
    base_currency TEXT NOT NULL
        CHECK (base_currency GLOB '[A-Z][A-Z][A-Z]'),
    soft_budget_json TEXT
        CHECK (
            soft_budget_json IS NULL
            OR (
                json_valid(soft_budget_json)
                AND json_type(soft_budget_json) = 'object'
                AND length(soft_budget_json) <= 4096
            )
        ),
    current_plan_id TEXT
        CHECK (current_plan_id IS NULL OR length(current_plan_id) BETWEEN 1 AND 200),
    current_plan_version INTEGER
        CHECK (
            current_plan_version IS NULL
            OR current_plan_version BETWEEN 1 AND 9223372036854775807
        ),
    created_at TEXT NOT NULL
        CHECK (
            length(created_at) BETWEEN 20 AND 35
            AND substr(created_at, -1) = 'Z'
            AND datetime(created_at) IS NOT NULL
        ),
    revision INTEGER NOT NULL
        CHECK (revision BETWEEN 1 AND 9223372036854775807),
    CHECK (
        (current_plan_id IS NULL AND current_plan_version IS NULL)
        OR (current_plan_id IS NOT NULL AND current_plan_version IS NOT NULL)
    )
) STRICT;

CREATE TABLE trip_memberships (
    trip_id TEXT NOT NULL,
    user_id TEXT NOT NULL,
    role TEXT NOT NULL
        CHECK (role IN ('leader', 'member', 'viewer')),
    joined_at TEXT NOT NULL
        CHECK (
            length(joined_at) BETWEEN 20 AND 35
            AND substr(joined_at, -1) = 'Z'
            AND datetime(joined_at) IS NOT NULL
        ),
    revision INTEGER NOT NULL
        CHECK (revision BETWEEN 1 AND 9223372036854775807),
    PRIMARY KEY (trip_id, user_id),
    FOREIGN KEY (trip_id) REFERENCES trips(id) ON DELETE RESTRICT,
    FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE RESTRICT
) STRICT;

CREATE INDEX trip_memberships_by_user
    ON trip_memberships(user_id, trip_id);

CREATE INDEX trip_memberships_by_role
    ON trip_memberships(trip_id, role);

CREATE TABLE trip_invites (
    trip_id TEXT NOT NULL,
    email_digest TEXT NOT NULL
        CHECK (
            length(email_digest) = 64
            AND email_digest NOT GLOB '*[^0-9a-f]*'
        ),
    id TEXT NOT NULL
        CHECK (length(id) BETWEEN 1 AND 200),
    email TEXT NOT NULL
        CHECK (
            length(email) BETWEEN 3 AND 320
            AND email = lower(trim(email))
            AND instr(email, '@') > 1
            AND instr(substr(email, instr(email, '@') + 1), '@') = 0
            AND instr(email, '@') < length(email)
        ),
    invited_by TEXT NOT NULL,
    status TEXT NOT NULL
        CHECK (status IN ('pending', 'accepted')),
    created_at TEXT NOT NULL
        CHECK (
            length(created_at) BETWEEN 20 AND 35
            AND substr(created_at, -1) = 'Z'
            AND datetime(created_at) IS NOT NULL
        ),
    revision INTEGER NOT NULL
        CHECK (revision BETWEEN 1 AND 9223372036854775807),
    PRIMARY KEY (trip_id, email_digest),
    UNIQUE (trip_id, id),
    FOREIGN KEY (trip_id) REFERENCES trips(id) ON DELETE RESTRICT,
    FOREIGN KEY (invited_by) REFERENCES users(id) ON DELETE RESTRICT
) STRICT;

CREATE INDEX trip_invites_by_email_status
    ON trip_invites(email_digest, status, trip_id);

-- Accepted invitations are retained for deterministic re-invites, so the
-- trip-side pending scan must not walk that unbounded history.
CREATE INDEX trip_invites_pending_by_trip
    ON trip_invites(trip_id, email_digest)
    WHERE status = 'pending';

PRAGMA user_version = 1;
PRAGMA foreign_key_check;
PRAGMA integrity_check;
