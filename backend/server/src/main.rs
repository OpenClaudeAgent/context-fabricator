use std::ffi::OsStr;
use std::io;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use ctxfab_inference::supervisor::{GUARD_ARGUMENT, run_guard};
use ctxfab_server::adapters::{
    inbound::http,
    outbound::{
        sqlite::SqliteStore,
        telemetry::{EmbeddedTelemetry, Limits},
    },
};
use ctxfab_server::application::{
    Settings,
    ports::Repository,
    telemetry::{Observer, Stage, Telemetry},
};
use ctxfab_server::runtime::entropy::random_uuid;
use ctxfab_server::runtime::inference::Inference;
use ctxfab_server::runtime::{
    clock::Clock, model_downloads::ModelDownloads, outbound::OutboundPool, session::Session,
};
use tokio::net::TcpListener;
use tokio::signal::unix::{Signal, SignalKind, signal};
use tokio::sync::{oneshot, watch};

#[cfg(all(target_os = "macos", debug_assertions))]
#[path = "qualification/credential_fixture.rs"]
mod credential_fixture;
#[cfg(debug_assertions)]
#[path = "qualification/sync_fixture.rs"]
mod sync_fixture;

mod composition;
mod launch;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    if args.next().as_deref() == Some(OsStr::new(GUARD_ARGUMENT)) {
        return Ok(run_guard(args)?);
    }
    serve_app()
}

#[tokio::main]
async fn serve_app() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt().with_writer(io::stderr).init();
    let options = launch::parse(std::env::args_os().skip(1))?;
    let directory = options.directory;
    let parent_pipe = options.parent_pipe;
    let shutdown = signal(SignalKind::terminate())?;
    let interrupted = signal(SignalKind::interrupt())?;
    let mut session = Session::acquire(&directory)?;
    let store = Arc::new(SqliteStore::open(&directory.join("context.sqlite3"))?);
    #[cfg(all(target_os = "macos", debug_assertions))]
    if let Some(fixture) = options.credential {
        use ctxfab_server::{
            adapters::outbound::keychain::Keychain, application::credentials::CredentialStore,
        };
        let keychain = Keychain::noninteractive();
        let adapter = keychain
            .as_ref()
            .map(|adapter| adapter as &dyn CredentialStore)
            .map_err(|error| *error);
        fixture.run(adapter, &store)?;
    }
    let clock = Clock::system();
    store.recover_traffic(clock.now())?;
    let outbound = OutboundPool::new(store.clone(), clock);
    let assets = Inference::asset_directory(&directory);
    let downloads = Arc::new(ModelDownloads::configured(&assets, &outbound)?);
    let github =
        composition::github::connect(&outbound, store.clone(), options.github_fixture, clock)?;
    let telemetry = start_telemetry(&directory, &store.settings()?);
    let inference = Inference::configured(&assets, &store);
    let job_trace = telemetry
        .as_ref()
        .map(|tel| tel.clone() as Arc<dyn Observer>);
    let scheduler =
        composition::github::scheduler(&github, clock, job_trace.clone(), options.github_scheduled);
    let import_fixture = None;
    #[cfg(debug_assertions)]
    let (scheduler, import_fixture) = if let Some(manual) = options.sync_fixture {
        let (scheduler, refresh) = sync_fixture::register(
            scheduler,
            store.clone(),
            &outbound,
            clock,
            job_trace,
            manual,
        )?;
        (scheduler, manual.then_some(refresh))
    } else {
        (scheduler, import_fixture)
    };
    store.record("backend_started", "Local backend is ready.")?;

    let (listener, address) = bind_loopback().await?;
    let token = format!("{}{}", random_uuid()?.simple(), random_uuid()?.simple());

    let (keep_alive, parent_closed) = app_session(parent_pipe)?;
    session.publish(address, &token)?;
    tracing::info!(code = "backend_started", "Local backend is ready");
    if let Some(telemetry) = &telemetry {
        telemetry.start(Stage::Startup, None).finish(true);
    }

    let api = http::router_with_services(
        store.clone(),
        &token,
        http::Services {
            telemetry: telemetry
                .as_ref()
                .map(|tel| tel.clone() as Arc<dyn Telemetry>),
            fixture: import_fixture,
            retrieval: inference.retrieval.clone(),
            answers: inference.answers.clone(),
            downloads: Some(downloads.clone()),
            github: Some(github),
        },
    );
    serve_http(
        listener,
        api,
        shutdown,
        interrupted,
        parent_closed,
        keep_alive,
        scheduler.stop_signal(),
    )
    .await?;

    downloads.stop().await;
    inference.shutdown();
    scheduler.stop().await;
    stop_telemetry(&store, telemetry.as_deref())?;
    Ok(())
}

async fn serve_http(
    listener: TcpListener,
    api: axum::Router,
    mut shutdown: Signal,
    mut interrupted: Signal,
    parent_closed: oneshot::Receiver<()>,
    keep_alive: Option<oneshot::Sender<()>>,
    stop_jobs: watch::Sender<bool>,
) -> io::Result<()> {
    let (stopping, stop_requested) = oneshot::channel();
    let serve = async {
        axum::serve(listener, api)
            .with_graceful_shutdown(async move {
                let _keep_alive = keep_alive;
                tokio::select! {
                    _ = shutdown.recv() => {}
                    _ = interrupted.recv() => {}
                    _ = parent_closed => {}
                }
                stop_jobs.send_replace(true);
                let _ = stopping.send(());
            })
            .await
    };
    let deadline = async {
        let _ = stop_requested.await;
        tokio::time::sleep(Duration::from_secs(3)).await;
    };
    tokio::select! {
        result = serve => result,
        () = deadline => {
            tracing::warn!(code = "shutdown_timeout", "Closing remaining local connections");
            Ok(())
        },
    }
}

async fn bind_loopback() -> io::Result<(TcpListener, SocketAddrV4)> {
    let listener = TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)).await?;
    let SocketAddr::V4(address) = listener.local_addr()? else {
        return Err(io::Error::other("unexpected address family"));
    };
    Ok((listener, address))
}

fn app_session(
    parent_pipe: bool,
) -> io::Result<(Option<oneshot::Sender<()>>, oneshot::Receiver<()>)> {
    let (closed, parent_closed) = oneshot::channel();
    if !parent_pipe {
        return Ok((Some(closed), parent_closed));
    }
    std::thread::Builder::new()
        .name("app-session".to_owned())
        .spawn(move || {
            let _ = io::copy(&mut io::stdin().lock(), &mut io::sink());
            let _ = closed.send(());
        })?;
    Ok((None, parent_closed))
}

fn stop_telemetry(
    store: &SqliteStore,
    telemetry: Option<&EmbeddedTelemetry>,
) -> Result<(), ctxfab_server::application::Failure> {
    store.record("backend_stopped", "Local backend stopped.")?;
    if let Some(telemetry) = telemetry {
        telemetry.start(Stage::Shutdown, None).finish(true);
        telemetry.shutdown();
    }
    Ok(())
}

fn start_telemetry(directory: &Path, settings: &Settings) -> Option<Arc<EmbeddedTelemetry>> {
    if let Ok(telemetry) = EmbeddedTelemetry::open(
        &directory.join("telemetry.sqlite3"),
        settings.diagnostic_retention_hours,
        Limits {
            storage_limit_mb: settings.diagnostic_storage_limit_mb,
            ..Limits::default()
        },
    ) {
        Some(Arc::new(telemetry))
    } else {
        tracing::warn!(
            code = "telemetry_unavailable",
            "Local telemetry storage could not be opened"
        );
        None
    }
}
