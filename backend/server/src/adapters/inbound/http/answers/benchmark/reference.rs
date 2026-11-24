use std::{
    path::Path,
    sync::{Arc, Mutex},
};

use serde::{Deserialize, Serialize};

use crate::{
    adapters::outbound::sqlite::SqliteStore,
    application::{github::current::Objects, ports::SnapshotStore},
    domain::{
        corpus::snapshot::{File, Plan},
        github::{
            Error, RepositoryName,
            current::Entry,
            source::{pinned_file_url, valid_object_id},
        },
    },
};

use super::{COLLECTION, REF};

#[derive(Clone, Deserialize, PartialEq, Eq, Hash, Serialize)]
#[serde(deny_unknown_fields)]
pub(in crate::adapters::inbound::http) struct Reference {
    repository: String,
    pub(super) revision: String,
}

impl Reference {
    pub(super) fn sort_key(&self) -> (&str, &str) {
        (&self.repository, &self.revision)
    }

    pub(super) fn name(&self) -> Option<RepositoryName<'_>> {
        let name = RepositoryName::parse(&self.repository).ok()?;
        (valid_object_id(&self.revision)
            && self.repository.split_once('/') == Some((name.owner, name.name)))
        .then_some(name)
    }
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Info {
    #[serde(flatten)]
    pub(super) reference: Reference,
    files: usize,
    bytes: usize,
}

#[derive(Clone)]
pub(in crate::adapters::inbound::http) struct Loaded {
    pub(super) store: Arc<SqliteStore>,
    pub(super) info: Info,
    pub(super) embedding_admission: Arc<Mutex<()>>,
}

impl Loaded {
    pub(super) fn from_store(store: SqliteStore, info: Info) -> Self {
        Self {
            store: Arc::new(store),
            info,
            embedding_admission: Arc::default(),
        }
    }

    pub(super) fn build(
        reference: Reference,
        objects: &dyn Objects,
        path: &Path,
    ) -> Result<Self, Error> {
        let root = objects.root(&reference.revision)?;
        let mut input = Input::default();
        input.walk(objects, &root, "", 0)?;
        if input.files.is_empty() {
            return Err(Error::Upstream);
        }
        let store = SqliteStore::open(path).map_err(|_| Error::Storage)?;
        let snapshot = store
            .begin_snapshot(&Plan {
                collection: COLLECTION,
                generation: 0,
                predecessor: None,
                revision: &reference.revision,
                tracked_ref: REF,
            })
            .map_err(|_| Error::Storage)?;
        for file in &mut input.files {
            file.url = pinned_file_url(&reference.repository, &reference.revision, &file.path);
        }
        let pages = input.files.chunks(64);
        let page_count = pages.len();
        for (sequence, page) in (0_u32..).zip(pages) {
            let files = page
                .iter()
                .map(|file| File {
                    source: &file.path,
                    path: &file.path,
                    blob: &file.blob,
                    file_url: &file.url,
                    text: Some(&file.text),
                })
                .collect::<Vec<_>>();
            store
                .append_snapshot(
                    snapshot.id,
                    sequence,
                    (sequence as usize + 1 < page_count).then_some("next"),
                    &files,
                )
                .map_err(|_| Error::Storage)?;
        }
        store
            .publish_snapshot(snapshot.id)
            .map_err(|_| Error::Storage)?;
        Ok(Self::from_store(
            store,
            Info {
                reference,
                files: input.files.len(),
                bytes: input.bytes,
            },
        ))
    }
}

struct Source {
    path: String,
    blob: String,
    url: String,
    text: String,
}

#[derive(Default)]
struct Input {
    files: Vec<Source>,
    bytes: usize,
    visited: usize,
}

impl Input {
    fn walk(
        &mut self,
        objects: &dyn Objects,
        tree: &str,
        prefix: &str,
        depth: usize,
    ) -> Result<(), Error> {
        if depth >= 24 {
            return Err(Error::CollectionLimit);
        }
        for entry in objects.tree(tree)? {
            self.visited += 1;
            if self.visited > 20_000 {
                return Err(Error::CollectionLimit);
            }
            match entry {
                Entry::Tree { path, oid } => {
                    self.walk(objects, &oid, &format!("{prefix}{path}/"), depth + 1)?;
                }
                Entry::File(file) => {
                    let Some(text) = objects.blob(&file.oid, file.bytes)? else {
                        continue;
                    };
                    self.bytes += text.len();
                    if self.files.len() >= 4_096 || self.bytes > 32 * 1024 * 1024 {
                        return Err(Error::CollectionLimit);
                    }
                    let path = format!("{prefix}{}", file.path);
                    self.files.push(Source {
                        path,
                        blob: file.oid,
                        url: String::new(),
                        text,
                    });
                }
                Entry::Skipped => {}
            }
        }
        Ok(())
    }
}
