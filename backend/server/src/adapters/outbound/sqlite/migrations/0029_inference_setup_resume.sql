ALTER TABLE inference_setups ADD COLUMN lease_revision INTEGER NOT NULL DEFAULT 0 CHECK (lease_revision >= 0);
ALTER TABLE inference_setups ADD COLUMN intent TEXT;
ALTER TABLE inference_setups ADD COLUMN waiting_archive INTEGER NOT NULL DEFAULT 0 CHECK (waiting_archive IN (0, 1));
CREATE TABLE inference_setup_model_receipts (
    operation_id TEXT NOT NULL REFERENCES inference_setups(operation_id),
    profile TEXT NOT NULL,
    PRIMARY KEY (operation_id, profile)
) STRICT;
