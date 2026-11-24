//! Resumable walk over immutable, non-recursive GitHub trees.

use serde::{Deserialize, Serialize};

use super::{Collector, Objects, SourceRequest, snapshot_error};
use crate::domain::corpus::snapshot::{File, Snapshot};
use crate::domain::github::{
    Error,
    current::{Entry, SourceFile},
    source::pinned_file_url,
};

const MAX_DEPTH: usize = 24;
const MAX_VISITED: u32 = 20_000;

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
    request: SourceRequest<'_>,
    stage: &mut Snapshot,
    root: Option<String>,
) -> Result<(), Error> {
    let scope = request.scope;
    let objects = worker.source.prepare(request, &stage.revision).await?;
    let objects = objects.as_deref();
    worker.store.check_scope(scope)?;
    let mut cursor = if let Some(raw) = &stage.cursor {
        serde_json::from_str(raw).map_err(|_| Error::Storage)?
    } else {
        let root = match (objects, root) {
            (Some(objects), Some(expected)) => {
                if objects.root(&stage.revision)? != expected {
                    return Err(Error::Upstream);
                }
                expected
            }
            (Some(objects), None) => objects.root(&stage.revision)?,
            (None, Some(root)) => root,
            (None, None) => worker.source.commit_tree(request, &stage.revision).await?,
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
            fetched[level] = Some(if let Some(objects) = objects {
                objects.tree(&frame.tree)?
            } else {
                worker.source.tree(request, &frame.tree).await?
            });
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
                    stage_file(worker, request, stage, &cursor, file, objects).await?;
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
    request: SourceRequest<'_>,
    stage: &mut Snapshot,
    cursor: &Cursor,
    file: &SourceFile,
    objects: Option<&dyn Objects>,
) -> Result<(), Error> {
    let scope = request.scope;
    worker.store.check_scope(scope)?;
    let path = cursor.full_path(&file.path)?;
    let text = if worker
        .store
        .has_source_blob(&scope.collection, &file.oid)
        .map_err(snapshot_error)?
    {
        None
    } else {
        let text = if let Some(objects) = objects {
            objects.blob(&file.oid, file.bytes)?
        } else {
            worker.source.blob(request, &file.oid, file.bytes).await?
        };
        let Some(text) = text else {
            return Ok(());
        };
        Some(text)
    };
    let file_url = pinned_file_url(&scope.full_name, &stage.revision, &path);
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
