ALTER TABLE sync_jobs ADD COLUMN generation INTEGER NOT NULL DEFAULT 0;

UPDATE sync_jobs
SET generation = coalesce((SELECT generation FROM collections WHERE name = collection), 0);

DELETE FROM sync_jobs WHERE phase = 'credentials'
    AND NOT EXISTS (SELECT 1 FROM collections WHERE name = collection AND access_lost = 1);
