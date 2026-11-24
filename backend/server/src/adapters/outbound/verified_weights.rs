//! File identities recorded only after validating the catalog-pinned digest.

use std::fs::{self, Metadata};
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WeightIdentity {
    device: u64,
    inode: u64,
    bytes: u64,
    modified: (i64, i64),
    changed: (i64, i64),
}

impl WeightIdentity {
    #[must_use]
    pub fn of(metadata: &Metadata) -> Option<Self> {
        metadata.is_file().then(|| Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            bytes: metadata.len(),
            modified: (metadata.mtime(), metadata.mtime_nsec()),
            changed: (metadata.ctime(), metadata.ctime_nsec()),
        })
    }

    #[must_use]
    pub fn same_payload(&self, other: &Self) -> bool {
        self.device == other.device
            && self.inode == other.inode
            && self.bytes == other.bytes
            && self.modified == other.modified
    }

    /// # Errors
    /// Returns an error when the path cannot be inspected.
    pub fn at(path: &Path) -> io::Result<Option<Self>> {
        Ok(Self::of(&fs::symlink_metadata(path)?))
    }
}
