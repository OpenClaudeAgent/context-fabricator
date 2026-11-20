CREATE TABLE outbound_buckets (
    bucket TEXT PRIMARY KEY,
    next_at INTEGER NOT NULL DEFAULT 0,
    cooldown_until INTEGER NOT NULL DEFAULT 0,
    attempts INTEGER NOT NULL DEFAULT 0,
    pending INTEGER NOT NULL DEFAULT 0,
    uncertain INTEGER NOT NULL DEFAULT 0,
    failures INTEGER NOT NULL DEFAULT 0,
    window_start INTEGER NOT NULL DEFAULT 0,
    window_attempts INTEGER NOT NULL DEFAULT 0
) STRICT;

CREATE TABLE sync_jobs (
    collection TEXT PRIMARY KEY,
    phase TEXT NOT NULL,
    due_at INTEGER,
    failures INTEGER NOT NULL
) STRICT;
