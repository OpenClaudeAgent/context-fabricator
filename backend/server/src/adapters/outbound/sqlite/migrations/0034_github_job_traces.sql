ALTER TABLE sync_jobs ADD COLUMN github_current_origin BLOB
    CHECK(github_current_origin IS NULL OR
          (typeof(github_current_origin) = 'blob' AND length(github_current_origin) = 24));

CREATE TRIGGER github_current_origin_generation_changed
AFTER UPDATE OF generation ON collections
WHEN NEW.generation <> OLD.generation
BEGIN
    UPDATE sync_jobs SET github_current_origin = NULL WHERE collection = OLD.name;
END;
