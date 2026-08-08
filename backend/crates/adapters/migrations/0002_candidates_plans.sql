-- Candidate snapshots and the versioned itinerary graph. This migration owns
-- its transaction because adding the trip's exact current-plan foreign key
-- requires a stopped-application table rebuild with foreign keys disabled on
-- the migration connection before BEGIN IMMEDIATE.

PRAGMA foreign_keys = OFF;
BEGIN IMMEDIATE;

CREATE TABLE trip_places (
    trip_id TEXT NOT NULL,
    id TEXT NOT NULL
        CHECK (length(id) BETWEEN 1 AND 200),
    name TEXT NOT NULL
        CHECK (length(name) BETWEEN 1 AND 200),
    kind TEXT NOT NULL
        CHECK (kind IN ('sight', 'food', 'lodging', 'activity', 'transport_hub')),
    lat REAL NOT NULL
        CHECK (lat BETWEEN -90.0 AND 90.0),
    lng REAL NOT NULL
        CHECK (lng BETWEEN -180.0 AND 180.0),
    tz TEXT NOT NULL
        CHECK (length(tz) <= 100),
    country_code TEXT NOT NULL
        CHECK (length(country_code) <= 2),
    admin_area TEXT NOT NULL
        CHECK (length(admin_area) <= 200),
    city TEXT NOT NULL
        CHECK (length(city) BETWEEN 1 AND 120),
    address TEXT NOT NULL
        CHECK (length(address) <= 500),
    external_ref_json TEXT
        CHECK (
            external_ref_json IS NULL
            OR (
                json_valid(external_ref_json)
                AND json_type(external_ref_json) = 'object'
                AND length(external_ref_json) <= 1024
            )
        ),
    website TEXT
        CHECK (website IS NULL OR length(website) BETWEEN 1 AND 2048),
    phone TEXT
        CHECK (phone IS NULL OR length(phone) BETWEEN 1 AND 80),
    rating REAL
        CHECK (rating IS NULL OR rating BETWEEN 0.0 AND 5.0),
    price_level INTEGER
        CHECK (price_level IS NULL OR price_level BETWEEN 1 AND 4),
    opening_hours_json TEXT
        CHECK (
            opening_hours_json IS NULL
            OR (
                json_valid(opening_hours_json)
                AND json_type(opening_hours_json) = 'object'
                AND length(opening_hours_json) <= 4096
            )
        ),
    photo_urls_json TEXT NOT NULL
        CHECK (
            json_valid(photo_urls_json)
            AND json_type(photo_urls_json) = 'array'
            AND length(photo_urls_json) <= 65536
        ),
    guide_json TEXT
        CHECK (
            guide_json IS NULL
            OR (
                json_valid(guide_json)
                AND json_type(guide_json) = 'object'
                AND length(guide_json) <= 65536
            )
        ),
    revision INTEGER NOT NULL
        CHECK (revision BETWEEN 1 AND 9223372036854775807),
    PRIMARY KEY (trip_id, id),
    FOREIGN KEY (trip_id) REFERENCES trips(id) ON DELETE RESTRICT
) STRICT;

CREATE TABLE candidates (
    trip_id TEXT NOT NULL,
    id TEXT NOT NULL
        CHECK (length(id) BETWEEN 1 AND 200),
    place_id TEXT NOT NULL
        CHECK (length(place_id) BETWEEN 1 AND 200),
    source_catalog_place_id TEXT
        CHECK (
            source_catalog_place_id IS NULL
            OR length(source_catalog_place_id) BETWEEN 1 AND 200
        ),
    source_trip_place_id TEXT
        CHECK (
            source_trip_place_id IS NULL
            OR length(source_trip_place_id) BETWEEN 1 AND 200
        ),
    proposed_by TEXT NOT NULL,
    created_at TEXT NOT NULL
        CHECK (
            length(created_at) BETWEEN 20 AND 35
            AND substr(created_at, -1) = 'Z'
            AND datetime(created_at) IS NOT NULL
        ),
    pitch TEXT NOT NULL
        CHECK (length(pitch) BETWEEN 1 AND 2000),
    tags_json TEXT NOT NULL
        CHECK (
            json_valid(tags_json)
            AND json_type(tags_json) = 'array'
            AND length(tags_json) <= 4096
        ),
    status TEXT NOT NULL
        CHECK (status IN ('shortlisted', 'in_plan', 'rejected')),
    revision INTEGER NOT NULL
        CHECK (revision BETWEEN 1 AND 9223372036854775807),
    PRIMARY KEY (trip_id, id),
    UNIQUE (trip_id, place_id),
    CHECK (
        source_catalog_place_id IS NULL
        OR source_trip_place_id IS NULL
    ),
    FOREIGN KEY (trip_id) REFERENCES trips(id) ON DELETE RESTRICT,
    FOREIGN KEY (trip_id, place_id)
        REFERENCES trip_places(trip_id, id) ON DELETE RESTRICT,
    FOREIGN KEY (trip_id, source_trip_place_id)
        REFERENCES trip_places(trip_id, id) ON DELETE RESTRICT,
    FOREIGN KEY (proposed_by) REFERENCES users(id) ON DELETE RESTRICT
) STRICT;

CREATE INDEX candidates_by_trip_created
    ON candidates(trip_id, created_at, id);

CREATE TABLE plans (
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
            length(created_at) BETWEEN 20 AND 35
            AND substr(created_at, -1) = 'Z'
            AND datetime(created_at) IS NOT NULL
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
    FOREIGN KEY (trip_id) REFERENCES trips(id) ON DELETE RESTRICT
) STRICT;

-- The proposals capability adds the deferred same-trip proposal foreign key
-- by rebuilding this table after the proposals parent table exists. SQLite
-- rejects even a NULL child insert when its declared parent table is absent.
CREATE UNIQUE INDEX plans_by_creating_proposal
    ON plans(trip_id, created_from_proposal_id)
    WHERE created_from_proposal_id IS NOT NULL;

CREATE TABLE plan_days (
    trip_id TEXT NOT NULL,
    plan_version INTEGER NOT NULL,
    id TEXT NOT NULL
        CHECK (length(id) BETWEEN 1 AND 200),
    plan_id TEXT NOT NULL
        CHECK (length(plan_id) BETWEEN 1 AND 200),
    date TEXT NOT NULL
        CHECK (
            length(date) = 10
            AND date GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]'
            AND date(date) = date
        ),
    city_hint TEXT NOT NULL
        CHECK (length(city_hint) BETWEEN 1 AND 120),
    tz TEXT NOT NULL
        CHECK (length(tz) BETWEEN 1 AND 100),
    window_start TEXT NOT NULL
        CHECK (
            length(window_start) = 5
            AND window_start GLOB '[0-2][0-9]:[0-5][0-9]'
            AND time(window_start) IS NOT NULL
        ),
    window_end TEXT NOT NULL
        CHECK (
            length(window_end) = 5
            AND window_end GLOB '[0-2][0-9]:[0-5][0-9]'
            AND time(window_end) IS NOT NULL
            AND window_end >= window_start
        ),
    revision INTEGER NOT NULL
        CHECK (revision BETWEEN 1 AND 9223372036854775807),
    PRIMARY KEY (trip_id, plan_version, id),
    UNIQUE (trip_id, plan_version, date),
    FOREIGN KEY (trip_id, plan_id, plan_version)
        REFERENCES plans(trip_id, id, version) ON DELETE RESTRICT
) STRICT;

CREATE TABLE stop_identities (
    trip_id TEXT NOT NULL,
    id TEXT NOT NULL
        CHECK (length(id) BETWEEN 1 AND 200),
    PRIMARY KEY (trip_id, id),
    FOREIGN KEY (trip_id) REFERENCES trips(id) ON DELETE RESTRICT
) STRICT;

CREATE TABLE plan_stops (
    trip_id TEXT NOT NULL,
    plan_version INTEGER NOT NULL,
    id TEXT NOT NULL
        CHECK (length(id) BETWEEN 1 AND 200),
    day_id TEXT NOT NULL
        CHECK (length(day_id) BETWEEN 1 AND 200),
    seq REAL NOT NULL
        CHECK (
            seq BETWEEN 1.0 AND 1000000.0
            AND seq = CAST(seq AS INTEGER)
        ),
    place_id TEXT NOT NULL
        CHECK (length(place_id) BETWEEN 1 AND 200),
    stop_kind TEXT NOT NULL
        CHECK (stop_kind IN ('visit', 'meal', 'lodging', 'activity', 'transit')),
    planned_arrival TEXT NOT NULL
        CHECK (
            length(planned_arrival) = 5
            AND planned_arrival GLOB '[0-2][0-9]:[0-5][0-9]'
            AND time(planned_arrival) IS NOT NULL
        ),
    duration_min INTEGER NOT NULL
        CHECK (duration_min BETWEEN 1 AND 1440),
    booking_ref TEXT
        CHECK (booking_ref IS NULL OR length(booking_ref) BETWEEN 1 AND 200),
    booking_url TEXT
        CHECK (booking_url IS NULL OR length(booking_url) BETWEEN 1 AND 2048),
    booking_cost_amount REAL
        CHECK (booking_cost_amount IS NULL OR booking_cost_amount >= 0.0),
    booking_cost_currency TEXT
        CHECK (
            booking_cost_currency IS NULL
            OR booking_cost_currency GLOB '[A-Z][A-Z][A-Z]'
        ),
    notes TEXT NOT NULL
        CHECK (length(notes) <= 10000),
    revision INTEGER NOT NULL
        CHECK (revision BETWEEN 1 AND 9223372036854775807),
    PRIMARY KEY (trip_id, plan_version, id),
    UNIQUE (trip_id, plan_version, day_id, seq),
    CHECK (
        (booking_ref IS NULL AND booking_url IS NULL
            AND booking_cost_amount IS NULL AND booking_cost_currency IS NULL)
        OR (
            booking_ref IS NOT NULL
            AND (booking_cost_amount IS NULL) = (booking_cost_currency IS NULL)
        )
    ),
    FOREIGN KEY (trip_id, plan_version, day_id)
        REFERENCES plan_days(trip_id, plan_version, id) ON DELETE RESTRICT,
    FOREIGN KEY (trip_id, id)
        REFERENCES stop_identities(trip_id, id) ON DELETE RESTRICT,
    FOREIGN KEY (trip_id, place_id)
        REFERENCES trip_places(trip_id, id) ON DELETE RESTRICT
) STRICT;

CREATE INDEX plan_stops_by_place
    ON plan_stops(trip_id, place_id, plan_version);

-- Rebuild trips to make the ID/version pair an exact, deferred foreign key.
CREATE TABLE trips_v2 (
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
            OR current_plan_version BETWEEN 1 AND 4294967295
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
    ),
    FOREIGN KEY (id, current_plan_id, current_plan_version)
        REFERENCES plans(trip_id, id, version)
        DEFERRABLE INITIALLY DEFERRED
) STRICT;

INSERT INTO trips_v2 (
    id, name, cover_photo_url, accent_color, stop_kind_labels_json, status,
    start_date, end_date, base_currency, soft_budget_json, current_plan_id,
    current_plan_version, created_at, revision
)
SELECT
    id, name, cover_photo_url, accent_color, stop_kind_labels_json, status,
    start_date, end_date, base_currency, soft_budget_json, current_plan_id,
    current_plan_version, created_at, revision
FROM trips;

DROP TABLE trips;
ALTER TABLE trips_v2 RENAME TO trips;

-- Foreign-key enforcement is disabled only for the referenced-table rebuild,
-- so turn its diagnostic into a checked write before committing. Any orphan
-- (including a legacy current-plan pointer) violates this temporary CHECK and
-- rolls the entire migration transaction back.
CREATE TEMP TABLE migration_0002_fk_assertion (
    violation_count INTEGER NOT NULL CHECK (violation_count = 0)
) STRICT;
INSERT INTO migration_0002_fk_assertion (violation_count)
SELECT COUNT(*) FROM pragma_foreign_key_check;
DROP TABLE migration_0002_fk_assertion;

PRAGMA user_version = 2;
COMMIT;
PRAGMA foreign_keys = ON;
PRAGMA foreign_key_check;
PRAGMA integrity_check;
