//! Shared GitHub wire objects for pinned branch and Git object reads.

use crate::domain::github::Error;
use serde::Deserialize;

pub(super) struct BranchPin {
    pub(super) revision: String,
    pub(super) tree: String,
}

#[derive(Deserialize)]
pub(super) struct Branch {
    name: String,
    commit: BranchCommit,
}

impl Branch {
    pub(super) fn pin(self, name: &str) -> Result<BranchPin, Error> {
        if self.name != name {
            return Err(Error::Upstream);
        }
        Ok(BranchPin {
            revision: oid(self.commit.sha)?,
            tree: oid(self.commit.commit.tree.sha)?,
        })
    }
}

#[derive(Deserialize)]
struct BranchCommit {
    sha: String,
    commit: CommitTree,
}

#[derive(Deserialize)]
struct CommitTree {
    tree: Object,
}

#[derive(Deserialize)]
pub(super) struct Object {
    pub(super) sha: String,
}

pub(super) fn oid(value: String) -> Result<String, Error> {
    if valid_oid(&value) {
        Ok(value)
    } else {
        Err(Error::Upstream)
    }
}

pub(super) use crate::domain::github::source::valid_object_id as valid_oid;
