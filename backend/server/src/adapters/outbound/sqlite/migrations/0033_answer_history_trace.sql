ALTER TABLE answer_history_entries ADD COLUMN trace_id BLOB
    CHECK(trace_id IS NULL OR (typeof(trace_id) = 'blob' AND length(trace_id) = 16));
CREATE UNIQUE INDEX answer_history_trace ON answer_history_entries(trace_id)
    WHERE trace_id IS NOT NULL;
