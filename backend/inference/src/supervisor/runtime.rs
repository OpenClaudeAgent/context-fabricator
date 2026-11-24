use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Holds the worker executable for a session. Managed releases publish one verified
/// binary from their immutable package catalog.
#[derive(Default)]
pub struct Runtime {
    executable: OnceLock<PathBuf>,
    weight_policy: Option<Box<WeightPolicy>>,
    guardian: Option<PathBuf>,
}

type WeightPolicy = dyn Fn(&Path, &str) -> bool + Send + Sync;

impl Runtime {
    #[must_use]
    pub fn preview(directory: &Path) -> Self {
        let runtime = Self::default();
        if let Ok(binary) = directory.join("llama-server").canonicalize()
            && binary.is_file()
        {
            let _ = runtime.executable.set(binary);
        }
        runtime
    }

    /// Publishes the canonical path returned by the package verifier.
    /// # Errors
    /// Returns the path if an executable is already published or the path is invalid.
    pub fn publish(&self, binary: PathBuf) -> Result<(), PathBuf> {
        if !binary.is_absolute() || !binary.is_file() {
            return Err(binary);
        }
        self.executable.set(binary)
    }

    #[must_use]
    pub fn is_available(&self) -> bool {
        self.binary().is_some()
    }

    #[must_use]
    pub fn with_weight_policy(
        policy: impl Fn(&Path, &str) -> bool + Send + Sync + 'static,
    ) -> Self {
        Self {
            executable: OnceLock::new(),
            weight_policy: Some(Box::new(policy)),
            guardian: None,
        }
    }

    /// Use an explicitly composed trusted executable implementing the guardian protocol.
    #[must_use]
    pub fn with_guardian(mut self, program: PathBuf) -> Self {
        self.guardian = Some(program);
        self
    }

    pub(super) fn guardian(&self) -> Option<&Path> {
        self.guardian.as_deref()
    }

    pub(super) fn admits_weights(&self, path: &Path, profile: &str) -> bool {
        self.weight_policy
            .as_ref()
            .map_or_else(|| path.is_file(), |policy| policy(path, profile))
    }

    pub(super) fn managed_weights(&self) -> bool {
        self.weight_policy.is_some()
    }

    pub(super) fn binary(&self) -> Option<&Path> {
        self.executable
            .get()
            .map(PathBuf::as_path)
            .filter(|path| path.is_file())
    }
}
