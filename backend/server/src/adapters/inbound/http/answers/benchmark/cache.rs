//! Retain one isolated SQLite index per immutable public snapshot.
use std::{
    collections::HashMap,
    fs,
    io::{self, ErrorKind},
    path::{Path, PathBuf},
    sync::Arc,
};

use serde::Serialize;
use sha2::{Digest, Sha256};

use super::{
    COLLECTION, hex,
    reference::{Info, Loaded, Reference},
};
use crate::{
    adapters::outbound::sqlite::SqliteStore,
    application::{github::current::Objects, ports::SnapshotStore},
    domain::github::Error,
};

const DATABASE: &str = "index.sqlite3";
const METADATA: &str = "corpus.json";
const DISABLED: &str = "inactive";

pub(in crate::adapters::inbound::http) type Shared = Arc<tokio::sync::Mutex<Cache>>;

pub(in crate::adapters::inbound::http) struct Cache {
    root: Option<PathBuf>,
    open: HashMap<Reference, Loaded>,
}

#[derive(Serialize)]
pub(super) struct Entry {
    #[serde(flatten)]
    pub(super) info: Info,
    pub(super) active: bool,
}

impl Cache {
    pub(in crate::adapters::inbound::http) fn shared(root: Option<PathBuf>) -> Shared {
        Arc::new(tokio::sync::Mutex::new(Self {
            root,
            open: HashMap::new(),
        }))
    }

    pub(super) fn list(&self) -> Result<Vec<Entry>, Error> {
        let Some(root) = &self.root else {
            return Ok(self
                .open
                .values()
                .map(|loaded| Entry {
                    info: loaded.info.clone(),
                    active: true,
                })
                .collect());
        };
        let mut entries = match fs::read_dir(root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
            Err(_) => return Err(Error::Storage),
        }
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let name = entry.file_name();
            let name = name.to_str()?;
            if name.len() != 64 || !name.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                return None;
            }
            match read_entry(&entry.path()) {
                Ok(Some(saved)) if key(&saved.info.reference) == name => Some(Ok(saved)),
                Ok(_) => None,
                Err(error) => Some(Err(error)),
            }
        })
        .collect::<Result<Vec<_>, _>>()?;
        entries.sort_by(|left, right| {
            left.info
                .reference
                .sort_key()
                .cmp(&right.info.reference.sort_key())
        });
        Ok(entries)
    }

    pub(super) fn get(
        &mut self,
        reference: &Reference,
        activate: bool,
    ) -> Result<Option<&Loaded>, Error> {
        if self.open.contains_key(reference) {
            return Ok(self.open.get(reference));
        }
        let Some(path) = self.path(reference) else {
            return Ok(None);
        };
        let Some(entry) = read_entry(&path)? else {
            return Ok(None);
        };
        if entry.info.reference != *reference || (!entry.active && !activate) {
            return Ok(None);
        }
        let store = SqliteStore::open(&path.join(DATABASE)).map_err(|_| Error::Storage)?;
        if store
            .active_snapshot(COLLECTION)
            .map_err(|_| Error::Storage)?
            .is_none_or(|snapshot| snapshot.revision != reference.revision)
        {
            return Err(Error::Storage);
        }
        if !entry.active {
            fs::remove_file(path.join(DISABLED)).map_err(|_| Error::Storage)?;
        }
        self.open
            .insert(reference.clone(), Loaded::from_store(store, entry.info));
        Ok(self.open.get(reference))
    }

    pub(super) fn import(
        &mut self,
        reference: Reference,
        objects: &dyn Objects,
    ) -> Result<Info, Error> {
        let loaded = if let (Some(root), Some(path)) = (&self.root, self.path(&reference)) {
            fs::create_dir_all(root).map_err(|_| Error::Storage)?;
            let scratch = tempfile::Builder::new()
                .prefix(".import-")
                .tempdir_in(root)
                .map_err(|_| Error::Storage)?;
            let staged = Loaded::build(reference.clone(), objects, &scratch.path().join(DATABASE))?;
            let metadata = serde_json::to_vec(&staged.info).map_err(|_| Error::Storage)?;
            fs::write(scratch.path().join(METADATA), metadata).map_err(|_| Error::Storage)?;
            // Close SQLite before moving the complete database and metadata together.
            drop(staged);
            fs::rename(scratch.path(), path).map_err(|_| Error::Storage)?;
            return self
                .get(&reference, false)?
                .map(|loaded| loaded.info.clone())
                .ok_or(Error::Storage);
        } else {
            Loaded::build(reference.clone(), objects, Path::new(":memory:"))?
        };
        let info = loaded.info.clone();
        self.open.insert(reference, loaded);
        Ok(info)
    }

    pub(super) fn deactivate(&mut self, reference: &Reference) -> Result<(), Error> {
        if let Some(path) = self.path(reference)
            && read_entry(&path)?.is_some_and(|entry| entry.active)
        {
            fs::write(path.join(DISABLED), []).map_err(|_| Error::Storage)?;
        }
        self.open.remove(reference);
        Ok(())
    }

    fn path(&self, reference: &Reference) -> Option<PathBuf> {
        self.root.as_ref().map(|root| root.join(key(reference)))
    }
}

pub(super) fn key(reference: &Reference) -> String {
    let (repository, revision) = reference.sort_key();
    let mut digest = Sha256::new();
    digest.update(repository.as_bytes());
    digest.update([0]);
    digest.update(revision.as_bytes());
    hex(&digest.finalize())
}

fn read_entry(path: &Path) -> Result<Option<Entry>, Error> {
    let metadata = match fs::read(path.join(METADATA)) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(Error::Storage),
    };
    let info: Info = serde_json::from_slice(&metadata).map_err(|_| Error::Storage)?;
    if info.reference.name().is_none() || !exists(&path.join(DATABASE))? {
        return Err(Error::Storage);
    }
    Ok(Some(Entry {
        info,
        active: !exists(&path.join(DISABLED))?,
    }))
}

fn exists(path: &Path) -> Result<bool, Error> {
    path.try_exists().map_err(|_: io::Error| Error::Storage)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::retrieval::Retrieval;
    use crate::domain::github::current::{Entry as GitEntry, SourceFile};
    use ctxfab_inference::{DIMENSIONS, Embedder, Embedding, EmbeddingError, Input};

    struct Fixture;

    impl Objects for Fixture {
        fn root(&self, _: &str) -> Result<String, Error> {
            Ok("tree".into())
        }

        fn tree(&self, _: &str) -> Result<Vec<GitEntry>, Error> {
            Ok(vec![GitEntry::File(SourceFile {
                path: "cache.rs".into(),
                oid: "b".repeat(40),
                bytes: 24,
            })])
        }

        fn blob(&self, _: &str, _: u32) -> Result<Option<String>, Error> {
            Ok(Some("The local cache persists.".into()))
        }
    }

    impl Embedder for Fixture {
        fn embed(&self, _: Input<'_>) -> Result<Embedding, EmbeddingError> {
            Embedding::new(vec![1.0; DIMENSIONS])
        }
    }

    #[tokio::test]
    async fn benchmark_cache_reopens_vectors_and_preserves_deactivation()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let reference: Reference = serde_json::from_value(serde_json::json!({
            "repository": "fixture/test", "revision": "a".repeat(40)
        }))?;
        let open = || Cache::shared(Some(directory.path().to_owned()));
        let indexed = |loaded: &Loaded| {
            Retrieval::new(
                loaded.store.clone(),
                loaded.store.clone(),
                Arc::new(Fixture),
                "fixture",
            )
        };

        let first = open();
        let mut cache = first.lock().await;
        cache.import(reference.clone(), &Fixture)?;
        let loaded = cache.get(&reference, false)?.ok_or("active corpus")?;
        assert_eq!(indexed(loaded).rebuild(8)?.indexed, 1);
        drop(cache);
        drop(first);

        let second = open();
        let mut cache = second.lock().await;
        assert!(cache.list()?[0].active);
        let loaded = cache.get(&reference, false)?.ok_or("restored corpus")?;
        assert_eq!(indexed(loaded).progress()?.indexed, 1);
        cache.deactivate(&reference)?;
        drop(cache);
        drop(second);

        let third = open();
        let mut cache = third.lock().await;
        assert!(!cache.list()?[0].active);
        assert!(cache.get(&reference, false)?.is_none());
        let loaded = cache
            .get(&reference, true)?
            .ok_or("activated local corpus")?;
        assert_eq!(indexed(loaded).progress()?.indexed, 1);
        assert!(cache.list()?[0].active);
        Ok(())
    }
}
