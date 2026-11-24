ALTER TABLE settings ADD COLUMN inference_artifact_limit_gb INTEGER NOT NULL DEFAULT 16
    CHECK (inference_artifact_limit_gb BETWEEN 1 AND 1000);
