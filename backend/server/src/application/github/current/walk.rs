//! Resumable walk over immutable, non-recursive GitHub trees.

use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use serde::{Deserialize, Serialize};

use super::{Collector, snapshot_error};
use crate::domain::corpus::snapshot::{File, Snapshot};
use crate::domain::github::{
    Error,
    current::{Entry, Scope, SourceFile},
};

const MAX_DEPTH: usize = 24;
const MAX_VISITED: u32 = 20_000;
const PATH_COMPONENT: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'.')
    .remove(b'-')
    .remove(b'_')
    .remove(b'~');

#[derive(Serialize, Deserialize)]
struct Cursor {
    frames: Vec<Frame>,
    visited: u32,
}

#[derive(Serialize, Deserialize)]
struct Frame {
    path: String,
    tree: String,
    offset: usize,
}

pub(super) async fn stage_files(
    worker: &Collector,
    scope: &Scope,
    secret: &[u8],
    stage: &mut Snapshot,
    root: Option<String>,
) -> Result<(), Error> {
    let mut cursor = if let Some(raw) = &stage.cursor {
        serde_json::from_str(raw).map_err(|_| Error::Storage)?
    } else {
        let root = if let Some(root) = root {
            root
        } else {
            worker.store.check_scope(scope)?;
            worker
                .source
                .commit_tree(secret, scope, &stage.revision)
                .await?
        };
        let cursor = Cursor::new(root);
        checkpoint(worker, stage, &cursor, &[])?;
        cursor
    };
    if cursor.frames.is_empty() || cursor.frames.len() > MAX_DEPTH {
        return Err(Error::Storage);
    }
    let mut fetched: [Option<Vec<Entry>>; MAX_DEPTH] = std::array::from_fn(|_| None);
    'walk: while let Some(frame) = cursor.frames.last() {
        let level = cursor.frames.len() - 1;
        if fetched[level].is_none() {
            worker.store.check_scope(scope)?;
            fetched[level] = Some(worker.source.tree(secret, scope, &frame.tree).await?);
        }
        let entries = fetched[level].as_deref().ok_or(Error::Storage)?;
        if frame.offset > entries.len() {
            return Err(Error::Upstream);
        }
        for entry in entries.iter().skip(frame.offset) {
            cursor.advance()?;
            match entry {
                Entry::Tree { path, oid } => {
                    if cursor.frames.len() >= MAX_DEPTH {
                        return Err(Error::CollectionLimit);
                    }
                    cursor.full_path(path)?;
                    cursor.frames.push(Frame {
                        path: path.clone(),
                        tree: oid.clone(),
                        offset: 0,
                    });
                    checkpoint(worker, stage, &cursor, &[])?;
                    continue 'walk;
                }
                Entry::File(file) => {
                    stage_file(worker, scope, secret, stage, &cursor, file).await?;
                }
                Entry::Skipped => {}
            }
        }
        fetched[level] = None;
        cursor.frames.pop();
        checkpoint(worker, stage, &cursor, &[])?;
    }
    Ok(())
}

async fn stage_file(
    worker: &Collector,
    scope: &Scope,
    secret: &[u8],
    stage: &mut Snapshot,
    cursor: &Cursor,
    file: &SourceFile,
) -> Result<(), Error> {
    worker.store.check_scope(scope)?;
    let path = cursor.full_path(&file.path)?;
    let text = if worker
        .store
        .has_source_blob(&scope.collection, &file.oid)
        .map_err(snapshot_error)?
    {
        None
    } else if let Some(text) = worker
        .source
        .blob(secret, scope, &file.oid, file.bytes)
        .await?
    {
        Some(text)
    } else {
        return Ok(());
    };
    let file_url = format!(
        "https://github.com/{}/blob/{}/{}",
        scope.full_name,
        stage.revision,
        path.split('/')
            .map(|part| utf8_percent_encode(part, PATH_COMPONENT).to_string())
            .collect::<Vec<_>>()
            .join("/")
    );
    checkpoint(
        worker,
        stage,
        cursor,
        &[File {
            source: &format!("github:{}:{path}", scope.repository_id),
            path: &path,
            blob: &file.oid,
            file_url: &file_url,
            text: text.as_deref(),
        }],
    )
}

fn checkpoint(
    worker: &Collector,
    stage: &mut Snapshot,
    cursor: &Cursor,
    files: &[File<'_>],
) -> Result<(), Error> {
    let cursor = (!cursor.frames.is_empty())
        .then(|| serde_json::to_string(cursor))
        .transpose()
        .map_err(|_| Error::Storage)?;
    worker
        .store
        .append_snapshot(stage.id, stage.sequence, cursor.as_deref(), files)
        .map_err(snapshot_error)?;
    stage.sequence += 1;
    Ok(())
}

impl Cursor {
    fn new(root: String) -> Self {
        Self {
            frames: vec![Frame {
                path: String::new(),
                tree: root,
                offset: 0,
            }],
            visited: 0,
        }
    }

    fn advance(&mut self) -> Result<(), Error> {
        if self.visited >= MAX_VISITED {
            return Err(Error::CollectionLimit);
        }
        self.visited += 1;
        self.frames.last_mut().ok_or(Error::Storage)?.offset += 1;
        Ok(())
    }

    fn full_path(&self, leaf: &str) -> Result<String, Error> {
        let path = self
            .frames
            .iter()
            .skip(1)
            .map(|frame| frame.path.as_str())
            .chain(std::iter::once(leaf))
            .collect::<Vec<_>>()
            .join("/");
        if path.len() > 1_024 {
            return Err(Error::CollectionLimit);
        }
        Ok(path)
    }
}
