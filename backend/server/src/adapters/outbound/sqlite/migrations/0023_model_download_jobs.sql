CREATE TABLE model_download_jobs (
    profile TEXT PRIMARY KEY,
    fingerprint TEXT NOT NULL CHECK (length(fingerprint) = 64),
    operation_id TEXT NOT NULL CHECK (length(operation_id) = 36),
    phase TEXT NOT NULL CHECK (phase IN ('running', 'paused', 'interrupted', 'failed')),
    error TEXT
) STRICT;
