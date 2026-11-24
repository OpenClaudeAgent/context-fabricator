//! Pure immutable source verification for a bounded historical comparison.
use crate::domain::github::Error;
use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};
use std::fmt::Write;

pub const MAX_CHANGES: usize = 64;
pub const MAX_FILE_BYTES: u32 = 256 * 1024;
pub const MAX_TOTAL_BYTES: usize = 512 * 1024;

#[derive(Clone)]
pub struct File {
    pub path: String,
    pub blob: String,
    pub size: Option<u64>,
    pub mode: String,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Side {
    pub path: String,
    pub blob: String,
    pub bytes: Option<u64>,
    pub mode: String,
    pub text: Option<String>,
    pub coverage: String,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Change {
    pub kind: String,
    pub before: Option<Side>,
    pub after: Option<Side>,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Comparison {
    pub repository_id: i64,
    pub repository: String,
    pub branch: String,
    pub lineage_tip: String,
    pub child: String,
    pub parent: Option<String>,
    pub baseline: String,
    pub coverage: String,
    pub changes: Vec<Change>,
}

/// Verify the Git blob identity independently of the source transport.
/// # Errors
/// Rejects bytes that do not match the selected tree entry.
pub fn side(file: &File, bytes: Vec<u8>) -> Result<Side, Error> {
    if u64::try_from(bytes.len()).ok() != file.size {
        return Err(Error::Upstream);
    }
    let mut hash = Sha1::new();
    hash.update(format!("blob {}\0", bytes.len()));
    hash.update(&bytes);
    let mut id = String::with_capacity(40);
    for byte in hash.finalize() {
        let _ = write!(id, "{byte:02x}");
    }
    if id != file.blob {
        return Err(Error::Upstream);
    }
    let text = String::from_utf8(bytes).ok().filter(|text| {
        !text.contains('\0') && !text.starts_with("version https://git-lfs.github.com/spec/v1\n")
    });
    let coverage = if text.is_some() {
        "fullBlob"
    } else {
        "binaryOrUnsupported"
    };
    Ok(Side {
        path: file.path.clone(),
        blob: file.blob.clone(),
        bytes: file.size,
        mode: file.mode.clone(),
        text,
        coverage: coverage.to_owned(),
    })
}

/// Metadata-only side when a blob cannot be read within the supported scope.
#[must_use]
pub fn unavailable(file: &File) -> Side {
    Side {
        path: file.path.clone(),
        blob: file.blob.clone(),
        bytes: file.size,
        mode: file.mode.clone(),
        text: None,
        coverage: if matches!(file.mode.as_str(), "100644" | "100755") {
            "tooLarge"
        } else {
            "unsupportedMode"
        }
        .to_owned(),
    }
}

#[must_use]
pub fn readable(file: &File) -> bool {
    matches!(file.mode.as_str(), "100644" | "100755")
        && file
            .size
            .is_some_and(|size| size <= u64::from(MAX_FILE_BYTES))
}

pub type PlannedChange = (&'static str, Option<File>, Option<File>);

/// Derive changes from complete, validated trees; equal blobs at unique paths are renames.
/// # Errors
/// Rejects duplicate paths and comparisons beyond the fixed change limit.
pub fn plan(mut old: Vec<File>, mut new: Vec<File>) -> Result<Vec<PlannedChange>, Error> {
    old.sort_by(|a, b| a.path.cmp(&b.path));
    new.sort_by(|a, b| a.path.cmp(&b.path));
    if old.windows(2).any(|p| p[0].path == p[1].path)
        || new.windows(2).any(|p| p[0].path == p[1].path)
    {
        return Err(Error::Upstream);
    }
    let mut changes = Vec::new();
    let mut removed = Vec::new();
    let mut added = Vec::new();
    for f in &old {
        match new.binary_search_by(|n| n.path.cmp(&f.path)) {
            Ok(i) if f.blob != new[i].blob || f.mode != new[i].mode => {
                changes.push(("modified", Some(f.clone()), Some(new[i].clone())));
            }
            Ok(_) => {}
            Err(_) => removed.push(f.clone()),
        }
    }
    for f in &new {
        if old.binary_search_by(|o| o.path.cmp(&f.path)).is_err() {
            added.push(f.clone());
        }
    }
    for f in removed {
        let matches: Vec<usize> = added
            .iter()
            .enumerate()
            .filter(|(_, n)| n.blob == f.blob && n.size == f.size && n.mode == f.mode)
            .map(|(i, _)| i)
            .collect();
        if matches.len() == 1
            && old.iter().filter(|o| o.blob == f.blob).count() == 1
            && new.iter().filter(|n| n.blob == f.blob).count() == 1
        {
            changes.push(("renamed", Some(f), Some(added.remove(matches[0]))));
        } else {
            changes.push(("deleted", Some(f), None));
        }
    }
    changes.extend(added.into_iter().map(|f| ("added", None, Some(f))));
    changes.sort_by(|a, b| {
        a.2.as_ref()
            .or(a.1.as_ref())
            .map(|f| &f.path)
            .cmp(&b.2.as_ref().or(b.1.as_ref()).map(|f| &f.path))
    });
    if changes.len() > MAX_CHANGES {
        return Err(Error::CollectionLimit);
    }
    Ok(changes)
}

#[cfg(test)]
mod tests {
    use super::{File, plan, readable, side, unavailable};

    fn file(path: &str, blob: &str, size: u64, mode: &str) -> File {
        File {
            path: path.to_owned(),
            blob: blob.to_owned(),
            size: Some(size),
            mode: mode.to_owned(),
        }
    }

    #[test]
    fn rejects_forged_blob_and_preserves_mode_changes_and_partial_coverage()
    -> Result<(), Box<dyn std::error::Error>> {
        let old = file(
            "src/a",
            "ce013625030ba8dba906f756967f9e9ca394464a",
            6,
            "100644",
        );
        assert!(side(&old, b"hello\n".to_vec()).is_ok());
        assert!(side(&old, b"jello\n".to_vec()).is_err());
        let mut changed = old.clone();
        changed.mode = "100755".to_owned();
        let plan = plan(vec![old], vec![changed])?;
        assert_eq!(plan.len(), 1);
        assert_eq!(plan[0].0, "modified");
        let large = file("large", "a", 300_000, "100644");
        assert!(!readable(&large));
        assert_eq!(unavailable(&large).coverage, "tooLarge");
        let link = file("link", "a", 1, "120000");
        assert_eq!(unavailable(&link).coverage, "unsupportedMode");
        Ok(())
    }
}
