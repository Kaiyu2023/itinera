-- Trip-scoped discussion threads, comments, and caller-owned reactions.
-- Every authoritative resource check remains in the repository transaction;
-- these strict tables preserve local shape, stable ownership, and reciprocity.

CREATE TABLE discussion_threads (
    trip_id TEXT NOT NULL,
    id TEXT NOT NULL
        CHECK (length(id) BETWEEN 1 AND 200),
    anchor_kind TEXT NOT NULL
        CHECK (anchor_kind IN ('trip', 'day', 'stop', 'poll', 'candidate')),
    anchor_id TEXT
        CHECK (anchor_id IS NULL OR length(anchor_id) BETWEEN 1 AND 200),
    anchor_key TEXT NOT NULL
        CHECK (length(anchor_key) BETWEEN 1 AND 210),
    title TEXT NOT NULL
        CHECK (
            length(title) BETWEEN 1 AND 200
            AND trim(title) = title
        ),
    created_at TEXT NOT NULL
        CHECK (
            length(created_at) = 30
            AND created_at GLOB
                '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9][0-9][0-9][0-9][0-9][0-9][0-9]Z'
            AND datetime(created_at) IS NOT NULL
        ),
    last_activity_at TEXT NOT NULL
        CHECK (
            length(last_activity_at) = 30
            AND last_activity_at GLOB
                '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9][0-9][0-9][0-9][0-9][0-9][0-9]Z'
            AND datetime(last_activity_at) IS NOT NULL
        ),
    revision INTEGER NOT NULL
        CHECK (revision BETWEEN 1 AND 9223372036854775807),
    PRIMARY KEY (trip_id, id),
    UNIQUE (trip_id, anchor_key),
    CHECK (
        (anchor_kind = 'trip' AND anchor_id IS NULL AND anchor_key = 'trip')
        OR (
            anchor_kind <> 'trip'
            AND anchor_id IS NOT NULL
            AND anchor_key = anchor_kind || ':' || anchor_id
        )
    ),
    CHECK (last_activity_at >= created_at),
    FOREIGN KEY (trip_id) REFERENCES trips(id) ON DELETE RESTRICT
) STRICT;

CREATE INDEX discussion_threads_by_activity
    ON discussion_threads(trip_id, last_activity_at DESC, id DESC);

CREATE TABLE discussion_comments (
    trip_id TEXT NOT NULL,
    thread_id TEXT NOT NULL,
    id TEXT NOT NULL
        CHECK (length(id) BETWEEN 1 AND 200),
    author_id TEXT NOT NULL,
    body TEXT NOT NULL
        CHECK (
            length(body) BETWEEN 1 AND 10000
            AND trim(body) = body
        ),
    created_at TEXT NOT NULL
        CHECK (
            length(created_at) = 30
            AND created_at GLOB
                '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9].[0-9][0-9][0-9][0-9][0-9][0-9][0-9][0-9][0-9]Z'
            AND datetime(created_at) IS NOT NULL
        ),
    revision INTEGER NOT NULL
        CHECK (revision BETWEEN 1 AND 9223372036854775807),
    PRIMARY KEY (trip_id, thread_id, id),
    FOREIGN KEY (trip_id, thread_id)
        REFERENCES discussion_threads(trip_id, id) ON DELETE RESTRICT,
    FOREIGN KEY (author_id) REFERENCES users(id) ON DELETE RESTRICT
) STRICT;

CREATE INDEX discussion_comments_by_thread_time
    ON discussion_comments(trip_id, thread_id, created_at, id);

CREATE TABLE comment_reactions (
    trip_id TEXT NOT NULL,
    thread_id TEXT NOT NULL,
    comment_id TEXT NOT NULL,
    emoji TEXT NOT NULL
        CHECK (
            length(emoji) BETWEEN 1 AND 16
            AND trim(emoji) = emoji
        ),
    user_id TEXT NOT NULL,
    PRIMARY KEY (trip_id, thread_id, comment_id, emoji, user_id),
    FOREIGN KEY (trip_id, thread_id, comment_id)
        REFERENCES discussion_comments(trip_id, thread_id, id) ON DELETE RESTRICT,
    FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE RESTRICT
) STRICT;

CREATE INDEX comment_reactions_by_comment
    ON comment_reactions(trip_id, thread_id, comment_id, emoji, user_id);

PRAGMA user_version = 5;
PRAGMA foreign_key_check;
PRAGMA integrity_check;
