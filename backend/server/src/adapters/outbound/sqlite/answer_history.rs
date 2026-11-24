use std::collections::BTreeSet;

use super::{SqliteStore, storage_failure};
use crate::application::{
    Failure,
    answer_history::{Capture, Entry, Journal, Page, Record, Settings, Summary},
};
use rusqlite::{Connection, OptionalExtension, ToSql, TransactionBehavior, params};
use uuid::Uuid;

const PAGE_SIZE: u16 = 25;
const DAY_MS: i64 = 86_400_000;

fn settings(db: &Connection) -> rusqlite::Result<Settings> {
    db.query_row(
        "SELECT revision, enabled, retention_days FROM answer_history_settings WHERE singleton=1",
        [],
        |row| {
            Ok(Settings {
                revision: row.get(0)?,
                enabled: row.get(1)?,
                retention_days: row.get(2)?,
            })
        },
    )
}
// Version 1 is the serialized public answer. Decode its nested fields instead of
// trusting an arbitrary JSON object read from the database. Unknown fields are
// tolerated for forward compatibility, while required fields and types are not.
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredAnswer {
    outcome: StoredOutcome,
    model: String,
    mode: StoredMode,
    text: Option<String>,
    citations: Vec<StoredCitation>,
    evidence_repository_ids: Vec<i64>,
}
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
enum StoredOutcome {
    Answered,
    InsufficientEvidence,
}
#[derive(serde::Deserialize)]
#[serde(rename_all = "lowercase")]
enum StoredMode {
    Bm25,
    Semantic,
    Hybrid,
    Selected,
}
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredCitation {
    evidence: StoredEvidence,
    quote: String,
    line_url: Option<String>,
    comparison: Option<StoredComparison>,
    workflow_run: Option<StoredWorkflowRun>,
}
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredEvidence {
    source: String,
    revision: String,
    kind: StoredKind,
    tracked_ref: Option<String>,
    title: String,
    access: StoredAccess,
    location: Option<crate::domain::corpus::snapshot::Location>,
}
#[derive(serde::Deserialize)]
#[serde(rename_all = "lowercase")]
enum StoredKind {
    Current,
    History,
    Workflow,
}
#[derive(serde::Deserialize)]
#[serde(rename_all = "lowercase")]
enum StoredAccess {
    Available,
    Retained,
}
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredComparison {
    side: StoredSide,
    child: String,
    parent: String,
    change_index: u32,
    hunk_index: u32,
}
#[derive(serde::Deserialize)]
#[serde(rename_all = "lowercase")]
enum StoredSide {
    Before,
    After,
}
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredWorkflowRun {
    repository_id: i64,
    workflow_id: i64,
    run_id: i64,
    attempt: u32,
    run_url: String,
    reported_head: String,
    commit_provenance: String,
}
fn valid_answer(value: &serde_json::Value) -> bool {
    let Ok(answer) = serde_json::from_value::<StoredAnswer>(value.clone()) else {
        return false;
    };
    // These fields are read to validate their types; empty values are not ruled
    // out by the serialized format and may occur in older valid answers.
    let _ = (&answer.outcome, &answer.model, &answer.mode, &answer.text);
    let unique_owners: BTreeSet<_> = answer.evidence_repository_ids.iter().copied().collect();
    answer.citations.len() <= 5
        && unique_owners.len() == answer.evidence_repository_ids.len()
        && unique_owners.iter().all(|id| *id > 0)
        && answer.citations.iter().all(|citation| {
            let evidence = &citation.evidence;
            let _ = (
                &evidence.source,
                &evidence.revision,
                &evidence.kind,
                &evidence.tracked_ref,
                &evidence.title,
                &evidence.access,
                &citation.line_url,
            );
            let comparison_valid = citation.comparison.as_ref().is_none_or(|comparison| {
                let _ = (
                    &comparison.side,
                    &comparison.child,
                    &comparison.parent,
                    comparison.change_index,
                    comparison.hunk_index,
                );
                true
            });
            let workflow_valid = citation.workflow_run.as_ref().is_none_or(|run| {
                let _ = (&run.run_url, &run.reported_head, &run.commit_provenance);
                run.repository_id > 0 && run.workflow_id > 0 && run.run_id > 0 && run.attempt > 0
            });
            let location_valid = evidence.location.as_ref().is_none_or(|location| {
                location.start_byte < location.end_byte
                    && location.start_line > 0
                    && location.start_line <= location.end_line
            });
            !citation.quote.is_empty() && comparison_valid && workflow_valid && location_valid
        })
}
fn cutoff(now: i64, days: u16) -> i64 {
    now.saturating_sub(i64::from(days) * DAY_MS)
}
fn prune(db: &Connection, now: i64, days: u16) -> rusqlite::Result<()> {
    db.execute(
        "DELETE FROM answer_history_entries WHERE completed_at_ms < ?1",
        [cutoff(now, days)],
    )?;
    Ok(())
}

#[derive(Clone, Copy)]
enum EntryKey {
    Id(i64),
    Trace(Uuid),
}

fn load_entry(store: &SqliteStore, key: EntryKey, now_ms: i64) -> Result<Option<Entry>, Failure> {
    let mut db = store.read()?;
    let tx = db.transaction().map_err(storage_failure)?;
    let policy = settings(&tx).map_err(storage_failure)?;
    if !policy.enabled {
        return Ok(None);
    }
    let (predicate, value): (_, &dyn ToSql) = match &key {
        EntryKey::Id(id) => ("id = ?1", id),
        EntryKey::Trace(trace) => ("trace_id = ?1", trace.as_bytes()),
    };
    let sql = format!(
        "SELECT id, question, scope_json, completed_at_ms, answer_version, answer_json, trace_id
         FROM answer_history_entries WHERE {predicate} AND completed_at_ms >= ?2"
    );
    let row = tx
        .query_row(
            &sql,
            params![value, cutoff(now_ms, policy.retention_days)],
            |r| {
                Ok((
                    r.get(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, i64>(3)?,
                    r.get::<_, i64>(4)?,
                    r.get::<_, String>(5)?,
                    r.get::<_, Option<Vec<u8>>>(6)?,
                ))
            },
        )
        .optional()
        .map_err(storage_failure)?;
    tx.commit().map_err(storage_failure)?;
    row.map(
        |(id, question, scope, completed_at_ms, version, answer, trace_id)| {
            if version != 1 {
                return Err(Failure::Storage);
            }
            let scope = serde_json::from_str(&scope).map_err(|_| Failure::Storage)?;
            let answer = serde_json::from_str(&answer).map_err(|_| Failure::Storage)?;
            if !valid_answer(&answer) {
                return Err(Failure::Storage);
            }
            let trace_id = trace_id
                .as_deref()
                .map(Uuid::from_slice)
                .transpose()
                .map_err(|_| Failure::Storage)?
                .map(|id| id.simple().to_string());
            let expires_at_ms =
                completed_at_ms.saturating_add(i64::from(policy.retention_days) * DAY_MS + 1);
            Ok(Entry {
                id,
                question,
                scope,
                completed_at_ms,
                expires_at_ms,
                trace_id,
                answer,
            })
        },
    )
    .transpose()
}

impl Journal for SqliteStore {
    fn settings(&self) -> Result<Settings, Failure> {
        settings(&*self.read()?).map_err(storage_failure)
    }
    fn update(&self, input: Settings, now_ms: i64) -> Result<Settings, Failure> {
        input.validate()?;
        let mut db = self.write()?;
        let tx = db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_failure)?;
        let previous = settings(&tx).map_err(storage_failure)?;
        let changed = tx.execute("UPDATE answer_history_settings SET revision=revision+1, epoch=epoch+CASE WHEN enabled=1 AND ?1=0 THEN 1 ELSE 0 END, enabled=?1, retention_days=?2 WHERE singleton=1 AND revision=?3", params![input.enabled, input.retention_days, input.revision]).map_err(storage_failure)?;
        if changed != 1 {
            return Err(Failure::Conflict);
        }
        if input.enabled {
            prune(
                &tx,
                now_ms,
                previous.retention_days.min(input.retention_days),
            )
            .map_err(storage_failure)?;
        } else {
            tx.execute("DELETE FROM answer_history_entries", [])
                .map_err(storage_failure)?;
        }
        let result = settings(&tx).map_err(storage_failure)?;
        tx.commit().map_err(storage_failure)?;
        Ok(result)
    }
    fn begin(&self) -> Result<Option<Capture>, Failure> {
        self.read()?.query_row("SELECT enabled, epoch, purge_sequence FROM answer_history_settings WHERE singleton=1", [], |row| { let enabled: bool = row.get(0)?; Ok(if enabled { Some(Capture { epoch: row.get(1)?, purge_sequence: row.get(2)? }) } else { None }) }).map_err(storage_failure)
    }
    fn save(&self, capture: Capture, record: Record<'_>) -> Result<(), Failure> {
        let mut owners: BTreeSet<i64> = record
            .answer
            .evidence_repository_ids
            .iter()
            .copied()
            .collect();
        owners.extend(record.scope.repository_id);
        owners.extend(record.answer.journal_repository_ids.iter().copied());
        if owners.iter().any(|id| *id <= 0) {
            return Err(Failure::Storage);
        }
        let scope = serde_json::to_string(record.scope).map_err(|_| Failure::Storage)?;
        let value = serde_json::to_value(record.answer).map_err(|_| Failure::Storage)?;
        if !valid_answer(&value) {
            return Err(Failure::Storage);
        }
        let answer = serde_json::to_string(&value).map_err(|_| Failure::Storage)?;
        let mut db = self.write()?;
        let tx = db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_failure)?;
        let allowed: bool = tx
            .query_row(
                "SELECT enabled=1 AND epoch=?1 FROM answer_history_settings WHERE singleton=1",
                [capture.epoch],
                |r| r.get(0),
            )
            .map_err(storage_failure)?;
        if !allowed {
            return Ok(());
        }
        for id in &owners {
            let purged: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM answer_history_purges WHERE repository_id=?1 AND sequence>?2 UNION ALL SELECT 1 FROM github_data_paused WHERE repository_id=?1)", params![id, capture.purge_sequence], |r| r.get(0)).map_err(storage_failure)?;
            if purged {
                return Ok(());
            }
        }
        let trace_id = record.trace_id.as_ref().map(Uuid::as_bytes);
        tx.execute("INSERT INTO answer_history_entries(question,scope_json,completed_at_ms,answer_version,answer_json,outcome,trace_id) VALUES(?1,?2,?3,1,?4,?5,?6)", params![record.question, scope, record.completed_at_ms, answer, record.answer.outcome, trace_id]).map_err(storage_failure)?;
        let id = tx.last_insert_rowid();
        for owner in owners {
            tx.execute(
                "INSERT INTO answer_history_owners(entry_id,repository_id) VALUES(?1,?2)",
                params![id, owner],
            )
            .map_err(storage_failure)?;
        }
        tx.commit().map_err(storage_failure)
    }
    fn list(&self, before_id: Option<i64>, now_ms: i64) -> Result<Page, Failure> {
        let db = self.read()?;
        let days = settings(&db).map_err(storage_failure)?.retention_days;
        let mut stmt = db.prepare("SELECT id,question,completed_at_ms,outcome FROM answer_history_entries WHERE (?1 IS NULL OR id<?1) AND completed_at_ms>=?2 ORDER BY id DESC LIMIT ?3").map_err(storage_failure)?;
        let rows = stmt
            .query_map(
                params![before_id, cutoff(now_ms, days), i64::from(PAGE_SIZE + 1)],
                |row| {
                    Ok(Summary {
                        id: row.get(0)?,
                        question: row.get(1)?,
                        completed_at_ms: row.get(2)?,
                        outcome: row.get(3)?,
                    })
                },
            )
            .map_err(storage_failure)?;
        let mut entries = rows
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(storage_failure)?;
        let next_before_id = if entries.len() > usize::from(PAGE_SIZE) {
            entries.truncate(usize::from(PAGE_SIZE));
            entries.last().map(|entry| entry.id)
        } else {
            None
        };
        Ok(Page {
            entries,
            next_before_id,
        })
    }
    fn get(&self, id: i64, now_ms: i64) -> Result<Option<Entry>, Failure> {
        load_entry(self, EntryKey::Id(id), now_ms)
    }
    fn get_by_trace(&self, trace_id: Uuid, now_ms: i64) -> Result<Option<Entry>, Failure> {
        load_entry(self, EntryKey::Trace(trace_id), now_ms)
    }
    fn clear(&self) -> Result<(), Failure> {
        let mut db = self.write()?;
        let tx = db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage_failure)?;
        tx.execute(
            "UPDATE answer_history_settings SET epoch=epoch+1 WHERE singleton=1",
            [],
        )
        .map_err(storage_failure)?;
        tx.execute("DELETE FROM answer_history_entries", [])
            .map_err(storage_failure)?;
        tx.commit().map_err(storage_failure)
    }
    fn prune(&self, now_ms: i64) -> Result<(), Failure> {
        let db = self.write()?;
        let days = settings(&db).map_err(storage_failure)?.retention_days;
        prune(&db, now_ms, days).map_err(storage_failure)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::application::answer_history::Scope;
    use crate::application::answers::{Answer, AnswerMode};
    use std::path::Path;

    fn answer(owners: Vec<i64>) -> Answer {
        Answer {
            outcome: "insufficientEvidence",
            model: "fixture".into(),
            mode: AnswerMode::Bm25,
            text: None,
            citations: vec![],
            evidence_repository_ids: owners,
            model_context: None,
            evidence_review: None,
            journal_repository_ids: Vec::new(),
        }
    }
    fn save(store: &SqliteStore, capture: Capture, owners: Vec<i64>, scope: &Scope, now: i64) {
        store
            .save(
                capture,
                Record {
                    question: "synthetic question",
                    scope,
                    answer: &answer(owners),
                    completed_at_ms: now,
                    trace_id: None,
                },
            )
            .unwrap();
    }
    fn enable(store: &SqliteStore) {
        store
            .update(
                Settings {
                    revision: 0,
                    enabled: true,
                    retention_days: 30,
                },
                0,
            )
            .unwrap();
    }
    fn repository(store: &SqliteStore, id: i64) {
        store.write().unwrap().execute("INSERT INTO github_repositories(id,full_name,default_branch,private,branch,interval_minutes,current_code,history,actions,code_graph,verified) VALUES(?1,'fixture/repo','main',0,'main',60,1,0,0,0,1)", [id]).unwrap();
    }
    #[test]
    fn opt_in_clear_disable_and_retention_survive_restart() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("journal.sqlite");
        let store = SqliteStore::open(&path).unwrap();
        assert!(store.begin().unwrap().is_none());
        assert_eq!(Journal::settings(&store).unwrap(), Settings::default());
        enable(&store);
        let stale = store.begin().unwrap().unwrap();
        save(
            &store,
            store.begin().unwrap().unwrap(),
            vec![],
            &Scope::default(),
            DAY_MS,
        );
        let id = store.list(None, DAY_MS).unwrap().entries[0].id;
        assert_eq!(
            store.get(id, DAY_MS).unwrap().unwrap().question,
            "synthetic question"
        );
        store.clear().unwrap();
        save(&store, stale, vec![], &Scope::default(), DAY_MS);
        assert!(store.list(None, DAY_MS).unwrap().entries.is_empty());
        save(
            &store,
            store.begin().unwrap().unwrap(),
            vec![],
            &Scope::default(),
            DAY_MS,
        );
        let retained_id = store.list(None, DAY_MS).unwrap().entries[0].id;
        let stale = store.begin().unwrap().unwrap();
        store
            .update(
                Settings {
                    revision: 1,
                    enabled: false,
                    retention_days: 1,
                },
                DAY_MS,
            )
            .unwrap();
        assert!(store.list(None, DAY_MS).unwrap().entries.is_empty());
        assert!(store.get(retained_id, DAY_MS).unwrap().is_none());
        store
            .update(
                Settings {
                    revision: 2,
                    enabled: true,
                    retention_days: 1,
                },
                DAY_MS,
            )
            .unwrap();
        save(&store, stale, vec![], &Scope::default(), DAY_MS);
        save(
            &store,
            store.begin().unwrap().unwrap(),
            vec![],
            &Scope::default(),
            DAY_MS,
        );
        drop(store);
        let store = SqliteStore::open(&path).unwrap();
        assert_eq!(store.list(None, DAY_MS).unwrap().entries.len(), 1);
        assert!(store.list(None, 2 * DAY_MS + 1).unwrap().entries.is_empty());
        store.prune(2 * DAY_MS + 1).unwrap();
        assert_eq!(
            store
                .read()
                .unwrap()
                .query_row("SELECT count(*) FROM answer_history_entries", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert_eq!(
            store.update(
                Settings {
                    revision: 0,
                    enabled: true,
                    retention_days: 1
                },
                0
            ),
            Err(Failure::Conflict)
        );
        assert_eq!(
            store.update(
                Settings {
                    revision: 3,
                    enabled: true,
                    retention_days: 0
                },
                0
            ),
            Err(Failure::InvalidAnswerHistoryRetention)
        );
    }
    #[test]
    fn extending_retention_cannot_restore_expired_entries() {
        let store = SqliteStore::open(Path::new(":memory:")).unwrap();
        store
            .update(
                Settings {
                    enabled: true,
                    retention_days: 1,
                    ..Settings::default()
                },
                0,
            )
            .unwrap();
        let trace = Uuid::from_u128(41);
        store
            .save(
                store.begin().unwrap().unwrap(),
                Record {
                    question: "expiry fixture",
                    scope: &Scope::default(),
                    answer: &answer(vec![]),
                    completed_at_ms: 0,
                    trace_id: Some(trace),
                },
            )
            .unwrap();
        let id = store.list(None, 0).unwrap().entries[0].id;
        let retained = store.get_by_trace(trace, DAY_MS).unwrap().unwrap();
        assert_eq!(retained.id, id);
        assert_eq!(retained.expires_at_ms, DAY_MS + 1);
        assert!(store.get(id, DAY_MS + 1).unwrap().is_none());
        assert!(store.get_by_trace(trace, DAY_MS + 1).unwrap().is_none());
        store
            .update(
                Settings {
                    revision: 1,
                    enabled: true,
                    retention_days: 30,
                },
                DAY_MS + 1,
            )
            .unwrap();
        assert!(store.get(id, DAY_MS + 1).unwrap().is_none());
        assert!(store.get_by_trace(trace, DAY_MS + 1).unwrap().is_none());
        assert!(store.list(None, DAY_MS + 1).unwrap().entries.is_empty());
    }

    #[test]
    fn version_32_retains_entries_without_fabricating_a_trace_link() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("legacy.sqlite");
        let mut database = Connection::open(&path).unwrap();
        let tx = database.transaction().unwrap();
        for migration in &super::super::MIGRATIONS[..32] {
            tx.execute_batch(migration).unwrap();
        }
        tx.pragma_update(None, "user_version", 32).unwrap();
        tx.execute(
            "UPDATE answer_history_settings SET enabled=1 WHERE singleton=1",
            [],
        )
        .unwrap();
        tx.execute("INSERT INTO answer_history_entries(question,scope_json,completed_at_ms,answer_version,answer_json,outcome) VALUES('original', '{}', 0, 1, ?1, 'insufficientEvidence')", [serde_json::to_string(&answer(vec![])).unwrap()]).unwrap();
        tx.commit().unwrap();
        drop(database);

        let store = SqliteStore::open(&path).unwrap();
        let retained = store.get(1, 0).unwrap().unwrap();
        assert_eq!(retained.question, "original");
        assert_eq!(retained.trace_id, None);
        assert!(store.get_by_trace(Uuid::from_u128(1), 0).unwrap().is_none());
    }
    #[test]
    fn purge_prevents_reinsertion_and_only_removes_dependent_entries() {
        let store = SqliteStore::open(Path::new(":memory:")).unwrap();
        repository(&store, 41);
        repository(&store, 42);
        enable(&store);
        let stale = store.begin().unwrap().unwrap();
        save(
            &store,
            store.begin().unwrap().unwrap(),
            vec![41],
            &Scope::default(),
            0,
        );
        save(
            &store,
            store.begin().unwrap().unwrap(),
            vec![42],
            &Scope::default(),
            0,
        );
        save(
            &store,
            store.begin().unwrap().unwrap(),
            vec![],
            &Scope {
                repository_id: Some(41),
                ..Scope::default()
            },
            0,
        );
        crate::application::github::Store::purge_data(&store, 41).unwrap();
        save(&store, stale, vec![41], &Scope::default(), 0);
        let remaining = store.list(None, 0).unwrap();
        assert_eq!(remaining.entries.len(), 1);
        assert_eq!(
            store
                .get(remaining.entries[0].id, 0)
                .unwrap()
                .unwrap()
                .answer["evidenceRepositoryIds"],
            serde_json::json!([42])
        );
        store
            .write()
            .unwrap()
            .execute("DELETE FROM github_data_paused WHERE repository_id=41", [])
            .unwrap();
        save(
            &store,
            store.begin().unwrap().unwrap(),
            vec![41],
            &Scope::default(),
            0,
        );
        assert_eq!(store.list(None, 0).unwrap().entries.len(), 2);
    }
    #[test]
    fn pages_are_bounded_and_invalid_versions_are_rejected() {
        let store = SqliteStore::open(Path::new(":memory:")).unwrap();
        enable(&store);
        for _ in 0..26 {
            save(
                &store,
                store.begin().unwrap().unwrap(),
                vec![],
                &Scope::default(),
                0,
            );
        }
        let first = store.list(None, 0).unwrap();
        assert_eq!(first.entries.len(), 25);
        let second = store.list(first.next_before_id, 0).unwrap();
        assert_eq!(second.entries.len(), 1);
        assert!(second.next_before_id.is_none());
        assert!(
            serde_json::to_value(&second)
                .unwrap()
                .get("nextBeforeId")
                .is_none()
        );
        let id = second.entries[0].id;
        store
            .write()
            .unwrap()
            .execute(
                "UPDATE answer_history_entries SET answer_version=99 WHERE id=?1",
                [id],
            )
            .unwrap();
        assert!(matches!(store.get(id, 0), Err(Failure::Storage)));
    }
    #[test]
    fn stored_citations_are_checked_recursively_and_bounded() {
        use serde_json::json;
        let store = SqliteStore::open(Path::new(":memory:")).unwrap();
        enable(&store);
        save(
            &store,
            store.begin().unwrap().unwrap(),
            vec![],
            &Scope::default(),
            0,
        );
        let id = store.list(None, 0).unwrap().entries[0].id;
        let history = json!({
            "evidence": {"source":"github:41:src/main.rs", "revision":"a", "kind":"history", "title":"src/main.rs", "access":"available", "location": {"path":"src/main.rs", "blob":"b", "fileUrl":"https://example.test/file", "startByte":0, "endByte":8, "startLine":1, "endLine":1}},
            "quote":"evidence", "lineUrl":"https://example.test/file#L1",
            "comparison":{"side":"before", "child":"a", "parent":"b", "changeIndex":0, "hunkIndex":0}
        });
        let workflow = json!({
            "evidence":{"source":"github:41:actions:9:1", "revision":"run-9-attempt-1", "kind":"workflow", "title":"Workflow", "access":"retained"},
            "quote":"evidence", "workflowRun":{"repositoryId":41,"workflowId":3,"runId":9,"attempt":1,"runUrl":"https://example.test/run","reportedHead":"a","commitProvenance":"verified"}
        });
        let valid = json!({"outcome":"answered", "model":"fixture", "mode":"selected", "text":"answer", "citations":[history.clone(), workflow.clone()], "evidenceRepositoryIds":[41]});
        let write = |value: &serde_json::Value| {
            store
                .write()
                .unwrap()
                .execute(
                    "UPDATE answer_history_entries SET answer_json=?1 WHERE id=?2",
                    params![value.to_string(), id],
                )
                .unwrap();
        };
        write(&valid);
        assert_eq!(store.get(id, 0).unwrap().unwrap().answer, valid);
        let mut invalid = Vec::new();
        let mut value = valid.clone();
        value["citations"][0]["evidence"] = json!({});
        invalid.push(value);
        let mut value = valid.clone();
        value["citations"] = json!([
            history.clone(),
            history.clone(),
            history.clone(),
            history.clone(),
            history.clone(),
            history.clone()
        ]);
        invalid.push(value);
        let mut value = valid.clone();
        value["citations"][0]["quote"] = json!("");
        invalid.push(value);
        let mut value = valid.clone();
        value["citations"][0]["evidence"]["location"]["endByte"] = json!(0);
        invalid.push(value);
        let mut value = valid.clone();
        value["citations"][0]["comparison"]["side"] = json!("outside");
        invalid.push(value);
        let mut value = valid.clone();
        value["citations"][1]["workflowRun"]["attempt"] = json!(0);
        invalid.push(value);
        let mut value = valid.clone();
        value["citations"][1]["workflowRun"]
            .as_object_mut()
            .unwrap()
            .remove("runUrl");
        invalid.push(value);
        let mut value = valid.clone();
        value["evidenceRepositoryIds"] = json!([41, 41]);
        invalid.push(value);
        for value in invalid {
            write(&value);
            assert!(
                matches!(store.get(id, 0), Err(Failure::Storage)),
                "accepted {value}"
            );
        }
    }
}
