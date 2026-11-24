ALTER TABLE generation_selection ADD COLUMN revision INTEGER NOT NULL DEFAULT 1 CHECK (revision >= 1);

CREATE TABLE inference_setups (
    operation_id TEXT NOT NULL PRIMARY KEY,
    fingerprint TEXT NOT NULL,
    selection_revision INTEGER NOT NULL CHECK (selection_revision >= 0),
    embedding_epoch INTEGER CHECK (embedding_epoch >= 0),
    phase TEXT NOT NULL CHECK (phase IN (
        'runtime', 'answer_weights', 'activating_answer', 'embedding_weights',
        'completed', 'failed', 'interrupted'
    )),
    answer_ready INTEGER NOT NULL DEFAULT 0 CHECK (answer_ready IN (0, 1)),
    error TEXT
) STRICT;

CREATE UNIQUE INDEX inference_setups_one_active ON inference_setups ((1))
    WHERE phase NOT IN ('completed', 'failed', 'interrupted');
