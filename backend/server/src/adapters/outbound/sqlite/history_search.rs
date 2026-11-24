//! Paired history search uses its own FTS5 population and active-lineage scope.

use rusqlite::named_params;

use super::SqliteStore;
use crate::{
    application::{
        CodeScope, SearchQuery,
        answers::{
            AnswerError,
            history::{HistoryStore, HunkKey, LocatedPair, MAX_PAIRS, Pair, Selection},
        },
        github::history::{Reopen, Reopened, Store},
    },
    domain::github::history::hunks::Hunk,
};

impl HistoryStore for SqliteStore {
    fn pairs(
        &self,
        query: &SearchQuery,
        selection: Selection<'_>,
    ) -> Result<Vec<Pair>, AnswerError> {
        Ok(search_pairs(self, query, Some(selection))?
            .into_iter()
            .map(|located| located.pair)
            .collect())
    }

    fn discover(&self, query: &SearchQuery) -> Result<Vec<LocatedPair>, AnswerError> {
        search_pairs(self, query, None)
    }

    fn reopen(
        &self,
        selection: Selection<'_>,
        pair: &Pair,
    ) -> Result<Option<Reopened>, AnswerError> {
        self.reopen_hunk(&Reopen {
            repository_id: selection.scope.repository_id,
            branch: &selection.scope.branch,
            child: selection.child,
            parent: Some(&pair.parent),
            change: usize::try_from(pair.key.change).map_err(|_| AnswerError::Storage)?,
            hunk: usize::try_from(pair.key.hunk).map_err(|_| AnswerError::Storage)?,
        })
        .map_err(|_| AnswerError::Storage)
    }
}

fn search_pairs(
    store: &SqliteStore,
    query: &SearchQuery,
    selection: Option<Selection<'_>>,
) -> Result<Vec<LocatedPair>, AnswerError> {
    let expression = super::search::fts_terms(query);
    let scope = selection.map(|s| s.scope).or_else(|| query.scope());
    let db = store.read().map_err(|_| AnswerError::Storage)?;
    let mut statement = db.prepare(
            "SELECT json_extract(evidence.document, '$.repository'), evidence.parent,
                keys.change_index, keys.hunk_index,
                json_extract(evidence.document, format('$.changes[%d].projection.hunks[%d]',
                                                        keys.change_index, keys.hunk_index)), evidence.repository_id, evidence.branch, evidence.child
             FROM github_history_hunks_fts
             JOIN github_history_hunk_keys keys ON keys.id = github_history_hunks_fts.rowid
             JOIN github_history_evidence evidence ON evidence.rowid = keys.evidence_id
             JOIN github_history_lineage lineage
                  ON (lineage.repository_id, lineage.branch) = (evidence.repository_id, evidence.branch)
             JOIN json_each(lineage.snapshot, '$.commits') commit_row
                  ON json_extract(commit_row.value, '$.id') = evidence.child
                  AND json_extract(commit_row.value, '$.parents[0]') = evidence.parent
             WHERE github_history_hunks_fts MATCH :expression
               AND (:id IS NULL OR (evidence.repository_id = :id AND evidence.branch = :branch))
               AND (:child IS NULL OR evidence.child = :child)
               AND evidence.projection_version = 1
               AND NOT EXISTS (SELECT 1 FROM github_data_paused p WHERE p.repository_id = evidence.repository_id)
             ORDER BY bm25(github_history_hunks_fts, 2.0, 1.0, 1.0), evidence.repository_id, evidence.branch, evidence.child, keys.change_index, keys.hunk_index
             LIMIT :limit",
        ).map_err(|_| AnswerError::Storage)?;
    let rows = statement
        .query_map(
            named_params! {
                ":expression": expression, ":limit": MAX_PAIRS,
                ":id": scope.map(|s| s.repository_id), ":branch": scope.map(|s| &s.branch),
                ":child": selection.map(|s| s.child),
            },
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    HunkKey {
                        change: row.get(2)?,
                        hunk: row.get(3)?,
                    },
                    row.get::<_, String>(4)?,
                    CodeScope {
                        repository_id: row.get(5)?,
                        branch: row.get(6)?,
                    },
                    row.get::<_, String>(7)?,
                ))
            },
        )
        .map_err(|_| AnswerError::Storage)?;
    rows.map(|row| {
        let (repository, parent, key, json, scope, child) =
            row.map_err(|_| AnswerError::Storage)?;
        let Hunk {
            before: Some(before),
            after: Some(after),
        } = serde_json::from_str(&json).map_err(|_| AnswerError::Storage)?
        else {
            return Err(AnswerError::Storage);
        };
        Ok(LocatedPair {
            scope,
            child,
            pair: Pair {
                key,
                repository,
                parent,
                before,
                after,
            },
        })
    })
    .collect()
}
