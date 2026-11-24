use std::sync::{Mutex, MutexGuard};

use gix::{
    ObjectId, Repository,
    objs::{Kind, tree::EntryKind},
};
use tempfile::TempDir;

use crate::application::github::current::Objects;
use crate::domain::github::{
    Error,
    current::{Entry, SourceFile},
};

use super::super::current::{MAX_FILE_BYTES, source_text, valid_component};

const MAX_TREE_ENTRIES: usize = 20_000;

pub(super) struct Snapshot {
    repository: Mutex<Repository>,
    directory: TempDir,
}

impl Snapshot {
    pub(super) fn new(repository: Repository, directory: TempDir) -> Self {
        Self {
            repository: Mutex::new(repository),
            directory,
        }
    }

    pub(super) fn repository(&self) -> Result<MutexGuard<'_, Repository>, Error> {
        self.repository.lock().map_err(|_| Error::Storage)
    }

    pub(super) fn pack_size(&self) -> std::io::Result<u64> {
        self.directory
            .path()
            .join("objects/pack")
            .read_dir()?
            .try_fold(0u64, |total, entry| {
                Ok(total.saturating_add(entry?.metadata()?.len()))
            })
    }
}

impl Objects for Snapshot {
    fn root(&self, revision: &str) -> Result<String, Error> {
        let repository = self.repository()?;
        let commit = repository
            .find_commit(parse(revision)?)
            .map_err(|_| Error::Upstream)?;
        Ok(commit.tree_id().map_err(|_| Error::Upstream)?.to_string())
    }

    fn tree(&self, id: &str) -> Result<Vec<Entry>, Error> {
        let repository = self.repository()?;
        let tree = repository
            .find_tree(parse(id)?)
            .map_err(|_| Error::Upstream)?;
        let mut entries = Vec::new();
        for entry in tree.iter() {
            if entries.len() == MAX_TREE_ENTRIES {
                return Err(Error::CollectionLimit);
            }
            let entry = entry.map_err(|_| Error::Upstream)?;
            let path = std::str::from_utf8(entry.filename().as_ref())
                .ok()
                .filter(|name| valid_component(name))
                .ok_or(Error::Upstream)?;
            let oid = entry.oid();
            let mapped = match entry.mode().kind() {
                EntryKind::Tree => Entry::Tree {
                    path: path.to_owned(),
                    oid: oid.to_string(),
                },
                EntryKind::Blob | EntryKind::BlobExecutable => {
                    let header = repository
                        .find_header(oid.to_owned())
                        .map_err(|_| Error::Upstream)?;
                    if header.kind() != Kind::Blob {
                        return Err(Error::Upstream);
                    }
                    if header.size() > MAX_FILE_BYTES {
                        Entry::Skipped
                    } else {
                        Entry::File(SourceFile {
                            path: path.to_owned(),
                            oid: oid.to_string(),
                            bytes: u32::try_from(header.size()).map_err(|_| Error::Upstream)?,
                        })
                    }
                }
                EntryKind::Link | EntryKind::Commit => Entry::Skipped,
            };
            entries.push((path.to_owned(), mapped));
        }
        // Preserve the order used by the resumable REST tree cursor.
        entries.sort_unstable_by(|a, b| a.0.cmp(&b.0));
        if entries.windows(2).any(|pair| pair[0].0 == pair[1].0) {
            return Err(Error::Upstream);
        }
        Ok(entries.into_iter().map(|(_, entry)| entry).collect())
    }

    fn blob(&self, id: &str, expected: u32) -> Result<Option<String>, Error> {
        let repository = self.repository()?;
        let oid = parse(id)?;
        let blob = repository.find_blob(oid).map_err(|_| Error::Upstream)?;
        let bytes = blob.data.as_slice();
        if usize::try_from(expected).ok() != Some(bytes.len())
            || gix::objs::compute_hash(oid.kind(), Kind::Blob, bytes)
                .map_err(|_| Error::Upstream)?
                != oid
        {
            return Err(Error::Upstream);
        }
        Ok(source_text(bytes))
    }
}

fn parse(id: &str) -> Result<ObjectId, Error> {
    ObjectId::from_hex(id.as_bytes()).map_err(|_| Error::Upstream)
}
