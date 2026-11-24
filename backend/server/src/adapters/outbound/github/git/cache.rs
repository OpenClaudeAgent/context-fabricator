use std::fs::{self, DirBuilder};
use std::io;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use gix::ObjectId;

use super::{GitHubRest, MAX_PACK_BYTES, SourceRequest, objects::Snapshot, prepare};
use crate::application::github::current::{Objects, Store};
use crate::domain::github::{Error, RepositoryName, current::Scope};

/// Keep at most one private shallow object database as a negotiation baseline.
#[derive(Default)]
pub(in super::super) struct Cache {
    workspace: Option<PathBuf>,
    latest: Mutex<Option<Baseline>>,
}

struct Baseline {
    scope: Scope,
    snapshot: Arc<Snapshot>,
}

impl Cache {
    pub(in super::super) fn open(data_directory: &Path) -> io::Result<Self> {
        let workspace = data_directory.join("github-fetch");
        match DirBuilder::new().mode(0o700).create(&workspace) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                let metadata = fs::symlink_metadata(&workspace)?;
                if !metadata.is_dir() || metadata.permissions().mode() & 0o077 != 0 {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "private Git workspace required",
                    ));
                }
                // The owner of the data-directory session lock can clear interrupted transfers.
                for item in fs::read_dir(&workspace)? {
                    let item = item?;
                    if item.file_type()?.is_dir()
                        && item
                            .file_name()
                            .as_encoded_bytes()
                            .starts_with(b"ctxfab-pack-")
                    {
                        fs::remove_dir_all(item.path())?;
                    }
                }
            }
            Err(error) => return Err(error),
        }
        Ok(Self {
            workspace: Some(workspace),
            latest: Mutex::new(None),
        })
    }

    pub(in super::super) async fn prepare(
        &self,
        github: &GitHubRest,
        source: SourceRequest<'_>,
        revision: &str,
    ) -> Result<Arc<dyn Objects>, Error> {
        let revision = ObjectId::from_hex(revision.as_bytes()).map_err(|_| Error::Upstream)?;
        let authority = github.scope_authority.as_deref().ok_or(Error::Storage)?;
        authority.check_scope(source.scope)?;
        let base = self
            .latest
            .lock()
            .map_err(|_| Error::Storage)?
            .take()
            .filter(|cached| same_scope(&cached.scope, source.scope))
            .map(|cached| cached.snapshot);
        let name = RepositoryName::parse(&source.scope.full_name)?;
        let snapshot = prepare(
            github,
            name,
            Some(source),
            revision,
            base,
            self.workspace.clone(),
        )
        .await?;
        let mut latest = self.latest.lock().map_err(|_| Error::Storage)?;
        authority.check_scope(source.scope)?;
        if snapshot
            .pack_size()
            .is_ok_and(|bytes| bytes <= MAX_PACK_BYTES as u64)
        {
            *latest = Some(Baseline {
                scope: source.scope.clone(),
                snapshot: snapshot.clone(),
            });
        }
        Ok(snapshot)
    }

    pub(in super::super) fn evict_inactive(&self, authority: Option<&dyn Store>) {
        if let Ok(mut latest) = self.latest.lock()
            && latest.as_ref().is_some_and(|cached| {
                authority.is_none_or(|store| store.check_scope(&cached.scope).is_err())
            })
        {
            *latest = None;
        }
    }
}

fn same_scope(previous: &Scope, current: &Scope) -> bool {
    previous.collection == current.collection
        && previous.generation == current.generation
        && previous.credential == current.credential
        && previous.repository_id == current.repository_id
        && previous.branch == current.branch
}
