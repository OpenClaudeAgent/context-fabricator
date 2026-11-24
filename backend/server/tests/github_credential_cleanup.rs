use ctxfab_server::adapters::outbound::sqlite::SqliteStore;
use ctxfab_server::application::credentials::{CredentialError, CredentialStore};
use ctxfab_server::application::github::{AccessProbe, GitHub, GitHubFuture, Provider, Store};
use ctxfab_server::domain::github::{Discovery, Error, Repository, RepositoryName};
use std::collections::HashMap;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

#[derive(Default)]
struct Credentials {
    values: Mutex<HashMap<String, Vec<u8>>>,
    deny: AtomicBool,
}
impl CredentialStore for Credentials {
    fn save(&self, key: &str, secret: &[u8]) -> Result<(), CredentialError> {
        self.values
            .lock()
            .map_err(|_| CredentialError::Unavailable)?
            .insert(key.into(), secret.into());
        Ok(())
    }
    fn read(&self, key: &str) -> Result<Vec<u8>, CredentialError> {
        self.values
            .lock()
            .map_err(|_| CredentialError::Unavailable)?
            .get(key)
            .cloned()
            .ok_or(CredentialError::Missing)
    }
    fn delete(&self, key: &str) -> Result<(), CredentialError> {
        if self.deny.load(Ordering::SeqCst) {
            return Err(CredentialError::Denied);
        }
        self.values
            .lock()
            .map_err(|_| CredentialError::Unavailable)?
            .remove(key)
            .map(|_| ())
            .ok_or(CredentialError::Missing)
    }
}
struct FakeProvider;
impl Provider for FakeProvider {
    fn verify<'a>(&'a self, _: &'a [u8]) -> GitHubFuture<'a, String> {
        Box::pin(async { Ok("fixture".into()) })
    }
    fn discover<'a>(&'a self, _: &'a [u8], _: u16) -> GitHubFuture<'a, Discovery> {
        Box::pin(async { Err(Error::Storage) })
    }
    fn branches<'a>(
        &'a self,
        _: ctxfab_server::application::github::MetadataRequest<'a>,
        _: RepositoryName<'a>,
        _: u16,
    ) -> GitHubFuture<'a, ctxfab_server::domain::github::branches::Page> {
        Box::pin(async { Err(Error::Storage) })
    }
    fn repository<'a>(
        &'a self,
        _: &'a [u8],
        _: RepositoryName<'a>,
    ) -> GitHubFuture<'a, Repository> {
        Box::pin(async { Err(Error::Storage) })
    }

    fn probe_access<'a>(
        &'a self,
        _: &'a [u8],
        _: &'a Repository,
        _: AccessProbe<'a>,
    ) -> GitHubFuture<'a, bool> {
        Box::pin(async { Err(Error::Storage) })
    }
}
#[expect(
    clippy::unnecessary_wraps,
    reason = "Matches the production reference factory signature"
)]
fn reference() -> Result<String, Error> {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    Ok(format!("fixture/{}", NEXT.fetch_add(1, Ordering::SeqCst)))
}
fn make_github(store: Arc<SqliteStore>, credentials: Arc<Credentials>) -> GitHub {
    GitHub::new(Arc::new(FakeProvider), store, Some(credentials), reference)
}

#[tokio::test]
async fn denied_rotation_is_retried_after_reopen_without_deleting_active_credential()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("db.sqlite");
    let credentials = Arc::new(Credentials::default());
    let store = Arc::new(SqliteStore::open(&path)?);
    let github = make_github(store.clone(), credentials.clone());
    github.authorize(0, "github_pat_first").await?;
    let first = store.binding()?.reference.ok_or(Error::Storage)?;
    credentials.deny.store(true, Ordering::SeqCst);
    github.authorize(1, "github_pat_second").await?;
    let second = store.binding()?.reference.ok_or(Error::Storage)?;
    assert!(credentials.read(&first).is_ok());
    assert_eq!(store.pending_credentials()?, vec![first.clone()]);
    drop(github);
    drop(store);
    credentials.deny.store(false, Ordering::SeqCst);
    let reopened = Arc::new(SqliteStore::open(&path)?);
    let _github = make_github(reopened.clone(), credentials.clone());
    assert_eq!(credentials.read(&first), Err(CredentialError::Missing));
    assert_eq!(credentials.read(&second)?, b"github_pat_second");
    assert_eq!(reopened.pending_credentials()?, Vec::<String>::new());
    assert_eq!(reopened.binding()?.reference, Some(second));
    Ok(())
}

#[tokio::test]
async fn disconnect_and_interrupted_staging_are_retried_idempotently()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("db.sqlite");
    let credentials = Arc::new(Credentials::default());
    let store = Arc::new(SqliteStore::open(&path)?);
    let github = make_github(store.clone(), credentials.clone());
    github.authorize(0, "github_pat_first").await?;
    let first = store.binding()?.reference.ok_or(Error::Storage)?;
    credentials.deny.store(true, Ordering::SeqCst);
    github.disconnect(1)?;
    assert!(store.binding()?.reference.is_none());
    assert!(credentials.read(&first).is_ok());
    store.stage_credential("fixture/interrupted")?;
    credentials.save("fixture/interrupted", b"github_pat_interrupted")?;
    store.stage_credential("fixture/before-write")?;
    drop(github);
    drop(store);
    credentials.deny.store(false, Ordering::SeqCst);
    let reopened = Arc::new(SqliteStore::open(&path)?);
    let github = make_github(reopened.clone(), credentials.clone());
    assert_eq!(credentials.read(&first), Err(CredentialError::Missing));
    assert_eq!(
        credentials.read("fixture/interrupted"),
        Err(CredentialError::Missing)
    );
    assert_eq!(reopened.pending_credentials()?, Vec::<String>::new());
    github.retry_credential_cleanup();
    assert_eq!(reopened.pending_credentials()?, Vec::<String>::new());
    Ok(())
}

struct StaleProvider(Arc<SqliteStore>);
impl Provider for StaleProvider {
    fn verify<'a>(&'a self, _: &'a [u8]) -> GitHubFuture<'a, String> {
        Box::pin(async {
            self.0.bind(1, None)?;
            Ok("fixture".into())
        })
    }
    fn discover<'a>(&'a self, _: &'a [u8], _: u16) -> GitHubFuture<'a, Discovery> {
        Box::pin(async { Err(Error::Storage) })
    }
    fn branches<'a>(
        &'a self,
        _: ctxfab_server::application::github::MetadataRequest<'a>,
        _: RepositoryName<'a>,
        _: u16,
    ) -> GitHubFuture<'a, ctxfab_server::domain::github::branches::Page> {
        Box::pin(async { Err(Error::Storage) })
    }
    fn repository<'a>(
        &'a self,
        _: &'a [u8],
        _: RepositoryName<'a>,
    ) -> GitHubFuture<'a, Repository> {
        Box::pin(async { Err(Error::Storage) })
    }

    fn probe_access<'a>(
        &'a self,
        _: &'a [u8],
        _: &'a Repository,
        _: AccessProbe<'a>,
    ) -> GitHubFuture<'a, bool> {
        Box::pin(async { Err(Error::Storage) })
    }
}

#[tokio::test]
async fn stale_replacement_remains_queued_when_deletion_is_denied()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("db.sqlite");
    let credentials = Arc::new(Credentials::default());
    let store = Arc::new(SqliteStore::open(&path)?);
    let github = make_github(store.clone(), credentials.clone());
    github.authorize(0, "github_pat_first").await?;
    let original = store.binding()?.reference.ok_or(Error::Storage)?;
    credentials.deny.store(true, Ordering::SeqCst);
    let stale = GitHub::new(
        Arc::new(StaleProvider(store.clone())),
        store.clone(),
        Some(credentials.clone()),
        reference,
    );
    assert!(matches!(
        stale.authorize(1, "github_pat_second").await,
        Err(Error::Conflict)
    ));
    let pending = store.pending_credentials()?;
    assert_eq!(pending.len(), 2);
    assert!(pending.contains(&original));
    for key in &pending {
        assert!(credentials.read(key).is_ok());
    }
    drop(stale);
    drop(github);
    drop(store);
    credentials.deny.store(false, Ordering::SeqCst);
    let reopened = Arc::new(SqliteStore::open(&path)?);
    let _github = make_github(reopened.clone(), credentials.clone());
    for key in &pending {
        assert_eq!(credentials.read(key), Err(CredentialError::Missing));
    }
    assert_eq!(reopened.pending_credentials()?, Vec::<String>::new());
    Ok(())
}

#[tokio::test]
async fn disconnect_without_credential_adapter_fences_and_queues_cleanup()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("db.sqlite");
    let credentials = Arc::new(Credentials::default());
    let store = Arc::new(SqliteStore::open(&path)?);
    let github = make_github(store.clone(), credentials.clone());
    github.authorize(0, "github_pat_first").await?;
    let first = store.binding()?.reference.ok_or(Error::Storage)?;
    drop(github);
    let unavailable = GitHub::new(Arc::new(FakeProvider), store.clone(), None, reference);
    unavailable.disconnect(1)?;
    assert!(store.binding()?.reference.is_none());
    assert_eq!(store.pending_credentials()?, vec![first.clone()]);
    assert!(credentials.read(&first).is_ok());
    drop(unavailable);
    drop(store);
    let reopened = Arc::new(SqliteStore::open(&path)?);
    let _github = make_github(reopened.clone(), credentials.clone());
    assert_eq!(credentials.read(&first), Err(CredentialError::Missing));
    assert_eq!(reopened.pending_credentials()?, Vec::<String>::new());
    Ok(())
}
