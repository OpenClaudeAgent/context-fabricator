//! Read one captured Actions attempt from its exact branch, including retained data.

use rusqlite::{OptionalExtension, params};

use super::SqliteStore;
use crate::application::answers::{
    AnswerError,
    actions::{ActionsStore, AttemptTarget, LocatedRun, Observation, Selection},
};
use crate::application::{CodeScope, SearchQuery};

const OBSERVATIONS: &str = "SELECT json_extract(s.snapshot, '$.repository'),
                        coalesce(r.actions AND r.verified AND r.actions_access AND r.contents_access
                                 AND NOT c.reconnect AND c.credential_ref IS NOT NULL, 0),
                        item.value, s.repository_id, s.branch,
                        json_extract(workflow.value, '$.name'), json_extract(workflow.value, '$.path')
                 FROM github_actions_snapshots s
                 LEFT JOIN github_repositories r ON r.id = s.repository_id AND r.branch = s.branch
                 CROSS JOIN github_connection c
                 JOIN json_each(s.snapshot, '$.runs') item
                 LEFT JOIN json_each(s.snapshot, '$.workflows') workflow
                   ON json_extract(workflow.value, '$.id') = json_extract(item.value, '$.workflowId')
                 WHERE c.singleton = 1 AND json_extract(s.snapshot, '$.repositoryId') = s.repository_id AND json_extract(s.snapshot, '$.branch') = s.branch";

impl ActionsStore for SqliteStore {
    fn latest(
        &self,
        scope: Option<&CodeScope>,
        repository: Option<&str>,
    ) -> Result<Option<LocatedRun>, AnswerError> {
        let database = self.read().map_err(|_| AnswerError::Storage)?;
        database
            .query_row(
                &format!("{OBSERVATIONS}
                    AND s.repository_id = (
                        SELECT CASE WHEN count(*) = 1 THEN max(id) END
                        FROM github_repositories
                        WHERE (?1 IS NULL OR (id = ?1 AND branch = ?2))
                          AND (?3 IS NULL OR full_name = ?3 COLLATE NOCASE
                               OR substr(full_name, instr(full_name, '/') + 1) = ?3 COLLATE NOCASE)
                          AND actions = 1
                    )
                    AND r.actions = 1 AND r.verified = 1 AND r.actions_access = 1 AND r.contents_access = 1
                    AND NOT c.reconnect AND c.credential_ref IS NOT NULL
                    ORDER BY json_extract(item.value, '$.createdAt') DESC,
                             json_extract(item.value, '$.id') DESC,
                             json_extract(item.value, '$.attempt') DESC
                    LIMIT 1"),
                params![
                    scope.map(|scope| scope.repository_id),
                    scope.map(|scope| scope.branch.as_str()),
                    repository,
                ],
                read_run,
            )
            .optional()
            .map_err(|_| AnswerError::Storage)
    }

    fn discover(
        &self,
        query: &SearchQuery,
        target: Option<&AttemptTarget>,
    ) -> Result<Vec<LocatedRun>, AnswerError> {
        let database = self.read().map_err(|_| AnswerError::Storage)?;
        let scope = query.scope();
        if let Some(target) = target {
            let mut statement = database
                .prepare(&format!(
                    "{OBSERVATIONS}
                AND (?1 IS NULL OR (s.repository_id = ?1 AND s.branch = ?2))
                AND (?3 IS NULL OR json_extract(s.snapshot, '$.repository') = ?3 COLLATE NOCASE)
                AND json_extract(item.value, '$.id') = ?4
                AND json_extract(item.value, '$.attempt') = ?5
                LIMIT 2"
                ))
                .map_err(|_| AnswerError::Storage)?;
            let mut rows = statement
                .query(params![
                    scope.map(|scope| scope.repository_id),
                    scope.map(|scope| scope.branch.as_str()),
                    target.repository.as_deref(),
                    target.run_id,
                    target.attempt,
                ])
                .map_err(|_| AnswerError::Storage)?;
            let run = rows
                .next()
                .map_err(|_| AnswerError::Storage)?
                .map(read_run)
                .transpose()
                .map_err(|_| AnswerError::Storage)?;
            if rows.next().map_err(|_| AnswerError::Storage)?.is_some() {
                return Ok(Vec::new());
            }
            return Ok(run.into_iter().collect());
        }
        let mut statement = database
            .prepare(&format!(
                "{OBSERVATIONS}
                AND (?1 IS NULL OR (s.repository_id = ?1 AND s.branch = ?2))"
            ))
            .map_err(|_| AnswerError::Storage)?;
        let mut rows = statement
            .query(params![
                scope.map(|scope| scope.repository_id),
                scope.map(|scope| scope.branch.as_str()),
            ])
            .map_err(|_| AnswerError::Storage)?;
        let terms: std::collections::BTreeSet<_> =
            query.terms().iter().map(|s| s.to_lowercase()).collect();
        let mut ranked = Vec::new();
        while let Some(row) = rows.next().map_err(|_| AnswerError::Storage)? {
            let located = read_run(row).map_err(|_| AnswerError::Storage)?;
            let run = &located.observation.run;
            let fields = format!(
                "GitHub Actions workflow {} {} {}",
                located.observation.repository,
                located.scope.branch,
                located.observation.fields()
            )
            .to_lowercase();
            let words: std::collections::BTreeSet<_> =
                fields.split(|c: char| !c.is_alphanumeric()).collect();
            let matches = terms
                .iter()
                .filter(|term| words.contains(term.as_str()))
                .count();
            if matches > 0 {
                let rank = (
                    terms.contains(&run.id.to_string()),
                    matches,
                    run.updated_at.clone(),
                    run.id,
                    run.attempt,
                    located.scope.repository_id,
                    located.scope.branch.clone(),
                );
                ranked.push((rank, located));
                ranked.sort_by(|a, b| b.0.cmp(&a.0));
                ranked.truncate(2);
            }
        }
        // A numeric run identity in the question disambiguates otherwise identical
        // recorded conclusions. Keep all matching attempts eligible.
        if ranked.first().is_some_and(|(rank, _)| rank.0) {
            ranked.retain(|(rank, _)| rank.0);
        }
        Ok(ranked
            .into_iter()
            .map(|(_, observation)| observation)
            .collect())
    }

    fn run(&self, selection: &Selection<'_>) -> Result<Option<Observation>, AnswerError> {
        let database = self.read().map_err(|_| AnswerError::Storage)?;
        database
            .query_row(
                &format!("{OBSERVATIONS} AND s.repository_id = ?1 AND s.branch = ?2 AND json_extract(item.value, '$.id') = ?3 AND json_extract(item.value, '$.attempt') = ?4"),
                params![
                    selection.scope.repository_id,
                    selection.scope.branch,
                    selection.run_id,
                    selection.attempt
                ],
                read_run,
            )
            .optional()
            .map(|located| located.map(|l| l.observation))
            .map_err(|_| AnswerError::Storage)
    }
}

fn read_run(row: &rusqlite::Row<'_>) -> rusqlite::Result<LocatedRun> {
    let payload: String = row.get(2)?;
    let run = serde_json::from_str(&payload).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(2, rusqlite::types::Type::Text, Box::new(error))
    })?;
    Ok(LocatedRun {
        scope: CodeScope {
            repository_id: row.get(3)?,
            branch: row.get(4)?,
        },
        observation: Observation {
            repository: row.get(0)?,
            retained: !row.get::<_, bool>(1)?,
            run,
            workflow_name: row.get(5)?,
            workflow_path: row.get(6)?,
        },
    })
}
