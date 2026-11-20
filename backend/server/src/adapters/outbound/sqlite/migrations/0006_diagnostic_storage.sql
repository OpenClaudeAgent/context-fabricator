ALTER TABLE settings ADD COLUMN diagnostic_storage_limit_mb INTEGER NOT NULL DEFAULT 10
    CHECK (diagnostic_storage_limit_mb BETWEEN 2 AND 1000);
