-- History has its own BM25 statistics and one row per paired hunk. A hit cannot
-- select one side independently or alter the rank of current-code evidence.
CREATE TABLE github_history_hunk_keys (
    id INTEGER PRIMARY KEY,
    evidence_id INTEGER NOT NULL,
    change_index INTEGER NOT NULL,
    hunk_index INTEGER NOT NULL,
    UNIQUE (evidence_id, change_index, hunk_index)
) STRICT;

CREATE VIRTUAL TABLE github_history_hunks_fts USING fts5(path, before_text, after_text,
    content='', contentless_delete=1);

CREATE VIEW github_history_hunk_projection AS
SELECT
    evidence.rowid AS evidence_id,
    change.key AS change_index,
    hunk.key AS hunk_index,
    json_extract(hunk.value, '$.after.path') AS path,
    json_extract(hunk.value, '$.before.text') AS before_text,
    json_extract(hunk.value, '$.after.text') AS after_text
FROM github_history_evidence evidence
JOIN json_each(evidence.document, '$.changes') change
JOIN json_each(change.value, '$.projection.hunks') hunk
WHERE json_extract(change.value, '$.projection.coverage') = 'complete'
  AND json_type(hunk.value, '$.before') = 'object'
  AND json_type(hunk.value, '$.after') = 'object'
  AND length(cast(json_extract(hunk.value, '$.before.text') AS BLOB)) <= 1800
  AND length(cast(json_extract(hunk.value, '$.after.text') AS BLOB)) <= 1800
  AND json_extract(hunk.value, '$.before.endLine') - json_extract(hunk.value, '$.before.startLine') < 12
  AND json_extract(hunk.value, '$.after.endLine') - json_extract(hunk.value, '$.after.startLine') < 12;

CREATE TRIGGER github_history_hunks_index AFTER INSERT ON github_history_evidence BEGIN
    INSERT INTO github_history_hunk_keys (evidence_id, change_index, hunk_index)
    SELECT evidence_id, change_index, hunk_index FROM github_history_hunk_projection
    WHERE evidence_id = new.rowid;

    INSERT INTO github_history_hunks_fts (rowid, path, before_text, after_text)
    SELECT keys.id, parts.path, parts.before_text, parts.after_text
    FROM github_history_hunk_keys keys JOIN github_history_hunk_projection parts
        USING (evidence_id, change_index, hunk_index)
    WHERE keys.evidence_id = new.rowid;
END;

CREATE TRIGGER github_history_hunks_remove AFTER DELETE ON github_history_evidence BEGIN
    DELETE FROM github_history_hunks_fts WHERE rowid IN (
        SELECT id FROM github_history_hunk_keys WHERE evidence_id = old.rowid
    );
    DELETE FROM github_history_hunk_keys WHERE evidence_id = old.rowid;
END;

INSERT INTO github_history_hunk_keys (evidence_id, change_index, hunk_index)
SELECT evidence_id, change_index, hunk_index FROM github_history_hunk_projection;

INSERT INTO github_history_hunks_fts (rowid, path, before_text, after_text)
SELECT keys.id, parts.path, parts.before_text, parts.after_text
FROM github_history_hunk_keys keys JOIN github_history_hunk_projection parts
    USING (evidence_id, change_index, hunk_index);
