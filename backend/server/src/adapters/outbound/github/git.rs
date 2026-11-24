//! Fetches immutable branch objects into a private, shallow bare repository.

mod cache;
mod objects;
mod transport;
mod wire;

use std::io;
use std::path::Path;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use std::num::NonZeroU32;

use gix::{ObjectId, remote::fetch::Shallow};
use gix_transport::{Protocol, client::blocking_io::http::Transport};
use tokio::sync::mpsc;

use super::GitHubRest;
use crate::application::github::current::SourceRequest;
use crate::domain::github::{Error, RepositoryName};
use wire::{Exchange, Wire};

pub(super) use cache::Cache;

const MAX_PACK_BYTES: usize = 64 * 1024 * 1024;
const MAX_EXCHANGES: usize = 12;

pub(super) async fn public_snapshot(
    github: &GitHubRest,
    name: RepositoryName<'_>,
    revision: &str,
) -> Result<Arc<dyn crate::application::github::current::Objects>, Error> {
    let revision = ObjectId::from_hex(revision.as_bytes()).map_err(|_| Error::InvalidSelection)?;
    Ok(prepare(github, name, None, revision, None, None).await?)
}

async fn prepare(
    github: &GitHubRest,
    name: RepositoryName<'_>,
    source: Option<SourceRequest<'_>>,
    revision: ObjectId,
    base: Option<Arc<objects::Snapshot>>,
    workspace: Option<std::path::PathBuf>,
) -> Result<Arc<objects::Snapshot>, Error> {
    let (requests, mut inbound) = mpsc::channel(1);
    let cancellation = Cancellation(Arc::new(AtomicBool::new(false)));
    let stop = cancellation.0.clone();
    let mut worker = tokio::task::spawn_blocking(move || {
        let snapshot = match base {
            Some(base) => base,
            None => Arc::new(new_snapshot(workspace.as_deref())?),
        };
        download(&snapshot, requests, revision, &stop)?;
        Ok(snapshot)
    });
    let mut remaining = MAX_EXCHANGES;
    loop {
        tokio::select! {
            result = &mut worker => {
                return result.map_err(|_| Error::Storage)?;
            }
            exchange = inbound.recv() => {
                let Some(Exchange { endpoint, protocol_v2, body, reply }) = exchange else {
                    return worker.await.map_err(|_| Error::Storage)?;
                };
                remaining = remaining.checked_sub(1).ok_or(Error::CollectionLimit)?;
                let response = github.exchange_git(&name, source, endpoint, protocol_v2, body).await;
                match response {
                    Ok(bytes) => {
                        if reply.send(Ok(bytes)).is_err() {
                            return Err(Error::Upstream);
                        }
                    }
                    Err(error) => {
                        let _ = reply.send(Err(io::Error::other("GitHub exchange ended")));
                        return Err(error);
                    }
                }
            }
        }
    }
}

fn new_snapshot(workspace: Option<&Path>) -> Result<objects::Snapshot, Error> {
    let mut scratch = tempfile::Builder::new();
    scratch.prefix("ctxfab-pack-");
    let directory = workspace
        .map_or_else(|| scratch.tempdir(), |path| scratch.tempdir_in(path))
        .map_err(|_| Error::Storage)?;
    gix::init_bare(directory.path()).map_err(|_| Error::Storage)?;
    let options = gix::open::Options::isolated().config_overrides([
        "pack.threads=1".to_owned(),
        format!("gitoxide.objects.allocLimit={MAX_PACK_BYTES}"),
    ]);
    let repository = gix::open_opts(directory.path(), options).map_err(|_| Error::Storage)?;
    Ok(objects::Snapshot::new(repository, directory))
}

fn download(
    snapshot: &objects::Snapshot,
    requests: mpsc::Sender<Exchange>,
    revision: ObjectId,
    stop: &AtomicBool,
) -> Result<(), Error> {
    const URL: &str = "https://github.com/ctxfab-snapshot";
    let repository = snapshot.repository()?;
    if repository.find_commit(revision).is_ok() {
        return Ok(());
    }
    let remote = repository
        .remote_at(URL)
        .and_then(|remote| {
            remote.with_refspecs(
                [gix::bstr::BString::from(format!(
                    "+{revision}:refs/heads/ctxfab-snapshot"
                ))],
                gix::remote::Direction::Fetch,
            )
        })
        .map_err(|_| Error::Upstream)?
        .with_fetch_tags(gix::remote::fetch::Tags::None);
    let url = gix::Url::try_from(URL).map_err(|_| Error::Upstream)?;
    let transport = Transport::new_http(Wire(requests), url, Protocol::V2, false);
    let mut connection = remote
        .to_connection_with_transport(transport)
        .with_transport_options(Box::new(()));
    connection.set_credentials(|_| Ok(None));
    connection
        .prepare_fetch(
            gix::progress::Discard,
            gix::remote::ref_map::Options::default(),
        )
        .and_then(|fetch| {
            fetch
                .with_shallow(Shallow::DepthAtRemote(NonZeroU32::MIN))
                .receive(gix::progress::Discard, stop)
        })
        .map_err(|_| Error::Upstream)?;
    repository
        .find_commit(revision)
        .map_err(|_| Error::ExpiredPin)?;
    Ok(())
}

struct Cancellation(Arc<AtomicBool>);

impl Drop for Cancellation {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}
