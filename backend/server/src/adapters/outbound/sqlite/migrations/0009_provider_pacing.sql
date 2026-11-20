ALTER TABLE outbound_buckets ADD COLUMN requests_per_minute INTEGER CHECK (requests_per_minute BETWEEN 1 AND 60);
ALTER TABLE outbound_buckets ADD COLUMN last_handoff INTEGER NOT NULL DEFAULT 0;
UPDATE outbound_buckets SET last_handoff = next_at WHERE attempts > 0;
