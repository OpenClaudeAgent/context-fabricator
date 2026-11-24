//! Filesystem capacity probe for the desktop application's owned artifacts.

use std::fs::{self, Metadata};
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use crate::application::artifact_storage::{
    ArtifactStorageBudget, CapacityProbe, CapacitySnapshot, Reservation,
};

pub type DesktopArtifactBudget = ArtifactStorageBudget<DesktopArtifactCapacityProbe>;
pub type DesktopArtifactReservation = Reservation<DesktopArtifactCapacityProbe>;

/// Counts only the inference and managed-runtime trees under the support root.
/// Symlink entries are counted by their own length and never followed. Mount
/// crossings, unsupported entries, and trees deeper than the configured bound
/// cause an error rather than undercounting. This path-based
/// scan assumes the application owns these trees and does not protect against
/// concurrent adversarial replacement of an entry between filesystem calls.
pub struct DesktopArtifactCapacityProbe {
    root: PathBuf,
    logical_limit: u64,
    live_limit: Option<Box<dyn Fn() -> io::Result<u64> + Send + Sync>>,
    system_reserve: u64,
    max_depth: usize,
}

impl DesktopArtifactCapacityProbe {
    #[must_use]
    pub fn new(root: PathBuf, logical_limit: u64, system_reserve: u64, max_depth: usize) -> Self {
        Self {
            root,
            logical_limit,
            live_limit: None,
            system_reserve,
            max_depth,
        }
    }
    #[must_use]
    pub fn with_live_limit(
        mut self,
        read_limit: impl Fn() -> io::Result<u64> + Send + Sync + 'static,
    ) -> Self {
        self.live_limit = Some(Box::new(read_limit));
        self
    }
}

impl CapacityProbe for DesktopArtifactCapacityProbe {
    type Error = io::Error;

    fn snapshot(&self) -> io::Result<CapacitySnapshot> {
        let root = fs::symlink_metadata(&self.root)?;
        if !root.is_dir() || root.file_type().is_symlink() {
            return Err(invalid("support root must be a directory, not a symlink"));
        }
        let stats = rustix::fs::statvfs(&self.root)?;
        let mut usage = 0_u64;
        for name in ["inference", "managed-runtime"] {
            let path = self.root.join(name);
            match fs::symlink_metadata(&path) {
                Ok(metadata) => {
                    if !metadata.is_dir() || metadata.file_type().is_symlink() {
                        return Err(invalid("owned artifact root must be a directory"));
                    }
                    usage = usage.saturating_add(count(
                        &path,
                        &metadata,
                        root.dev(),
                        0,
                        self.max_depth,
                    )?);
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }
        Ok(CapacitySnapshot {
            logical_limit: match &self.live_limit {
                Some(read_limit) => read_limit()?,
                None => self.logical_limit,
            },
            logical_usage: usage,
            physical_available: stats.f_bavail.saturating_mul(stats.f_frsize),
            system_reserve: self.system_reserve,
        })
    }
}

fn count(
    path: &Path,
    metadata: &Metadata,
    device: u64,
    depth: usize,
    max_depth: usize,
) -> io::Result<u64> {
    if metadata.dev() != device {
        return Err(invalid("artifact entry is on another volume"));
    }
    if metadata.is_file() || metadata.file_type().is_symlink() {
        return Ok(metadata.len());
    }
    if !metadata.is_dir() {
        return Err(invalid("unsupported artifact entry"));
    }
    let mut total = 0_u64;
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let next_depth = depth
            .checked_add(1)
            .ok_or_else(|| invalid("artifact depth overflow"))?;
        if next_depth > max_depth {
            return Err(invalid("artifact tree exceeds depth limit"));
        }
        let metadata = fs::symlink_metadata(entry.path())?;
        total = total.saturating_add(count(
            &entry.path(),
            &metadata,
            device,
            next_depth,
            max_depth,
        )?);
    }
    Ok(total)
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    fn probe(root: &Path, depth: usize) -> DesktopArtifactCapacityProbe {
        DesktopArtifactCapacityProbe::new(root.to_owned(), 1000, 50, depth)
    }

    #[test]
    fn reads_live_limit_for_each_snapshot_and_fails_closed() -> io::Result<()> {
        use std::sync::{
            Arc,
            atomic::{AtomicU64, Ordering},
        };
        let temp = tempfile::tempdir()?;
        let limit = Arc::new(AtomicU64::new(10));
        let current = Arc::clone(&limit);
        let probe = probe(temp.path(), 1).with_live_limit(move || {
            let value = current.load(Ordering::SeqCst);
            if value == 0 {
                Err(io::Error::other("settings unavailable"))
            } else {
                Ok(value)
            }
        });
        assert_eq!(probe.snapshot()?.logical_limit, 10);
        limit.store(20, Ordering::SeqCst);
        assert_eq!(probe.snapshot()?.logical_limit, 20);
        limit.store(0, Ordering::SeqCst);
        assert!(probe.snapshot().is_err());
        Ok(())
    }

    #[test]
    fn counts_only_owned_regular_files_and_ignores_missing_owned_roots() -> io::Result<()> {
        let temp = tempfile::tempdir()?;
        fs::create_dir_all(temp.path().join("inference/nested"))?;
        fs::write(temp.path().join("inference/model"), [1; 10])?;
        fs::write(temp.path().join("inference/nested/partial"), [1; 7])?;
        fs::write(temp.path().join("unowned"), [1; 100])?;
        let snapshot = probe(temp.path(), 2).snapshot()?;
        assert_eq!(snapshot.logical_usage, 17);
        assert_eq!(snapshot.logical_limit, 1000);
        assert_eq!(snapshot.system_reserve, 50);
        fs::create_dir(temp.path().join("managed-runtime"))?;
        fs::write(temp.path().join("managed-runtime/archive"), [1; 3])?;
        assert_eq!(probe(temp.path(), 2).snapshot()?.logical_usage, 20);
        Ok(())
    }

    #[test]
    fn rejects_missing_or_invalid_support_root() -> io::Result<()> {
        let temp = tempfile::tempdir()?;
        assert_eq!(
            probe(&temp.path().join("missing"), 1)
                .snapshot()
                .err()
                .map(|error| error.kind()),
            Some(io::ErrorKind::NotFound)
        );
        let file = temp.path().join("file");
        fs::write(&file, [])?;
        assert!(probe(&file, 1).snapshot().is_err());
        let link = temp.path().join("link");
        symlink(temp.path(), &link)?;
        assert!(probe(&link, 1).snapshot().is_err());
        Ok(())
    }

    #[test]
    fn rejects_symlinked_owned_roots() -> io::Result<()> {
        let temp = tempfile::tempdir()?;
        for name in ["inference", "managed-runtime"] {
            let path = temp.path().join(name);
            symlink(temp.path(), &path)?;
            assert!(probe(temp.path(), 2).snapshot().is_err());
            fs::remove_file(path)?;
        }
        Ok(())
    }

    #[test]
    fn counts_runtime_aliases_without_following_external_links() -> io::Result<()> {
        let temp = tempfile::tempdir()?;
        let external = tempfile::tempdir()?;
        fs::write(external.path().join("large"), [1; 4096])?;
        let framework = temp.path().join("managed-runtime/Example.framework");
        fs::create_dir_all(framework.join("Versions/A"))?;
        fs::write(framework.join("Versions/A/library"), [1; 7])?;
        let alias = framework.join("Versions/Current");
        symlink("A", &alias)?;
        let outside = framework.join("external");
        symlink(external.path(), &outside)?;
        let expected =
            7 + fs::symlink_metadata(alias)?.len() + fs::symlink_metadata(outside)?.len();
        assert_eq!(probe(temp.path(), 4).snapshot()?.logical_usage, expected);
        Ok(())
    }

    #[test]
    fn fails_closed_when_depth_exceeded_or_owned_root_is_file() -> io::Result<()> {
        let temp = tempfile::tempdir()?;
        let inference = temp.path().join("inference");
        fs::write(&inference, [])?;
        assert!(probe(temp.path(), 1).snapshot().is_err());
        fs::remove_file(&inference)?;
        fs::create_dir_all(inference.join("nested"))?;
        fs::write(inference.join("nested/file"), [1])?;
        assert!(probe(temp.path(), 1).snapshot().is_err());
        assert_eq!(probe(temp.path(), 2).snapshot()?.logical_usage, 1);
        Ok(())
    }
}
