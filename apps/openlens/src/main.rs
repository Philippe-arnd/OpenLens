//! The OpenLens server binary: API, ingestion and UI in one process.
//!
//! Configuration is read from the environment:
//! - `OPENLENS_ADDR`: listen address (default `0.0.0.0:8686`).
//! - `OPENLENS_DATA_DIR`: data root (default `./data`).
//! - `OPENLENS_ADMIN_TOKEN`: admin token; generated and stored on first start
//!   when unset.
//! - `OPENLENS_PROJECT_KEY`: when set and there is no project yet, a project
//!   `default` is created with this capture key (CI, demos, templates that
//!   want a key at once). Otherwise agents create projects (`init`).
//!
//! `openlens healthcheck` probes `/healthz` on the local port and exits 0 or 1;
//! the container image has no shell or curl, so the compose healthcheck uses it.
//!
//! `openlens backup <dest>` writes a consistent copy of the data dir to
//! `dest` (empty or new), safe while the server runs. In the container:
//! `docker exec <c> openlens backup /data/backups/2026-11-22`.
//! `openlens backup --into <dir> [--keep N]` names the copy after the UTC
//! time and keeps the newest N (7 by default): for scheduled jobs.

mod upgrade;

use std::process::ExitCode;

const DEFAULT_ADDR: &str = "0.0.0.0:8686";

fn main() -> ExitCode {
    match std::env::args().nth(1).as_deref() {
        Some("healthcheck") => return healthcheck(),
        Some("backup") => {
            let args: Vec<String> = std::env::args().skip(2).collect();
            let data = std::env::var("OPENLENS_DATA_DIR").unwrap_or_else(|_| "./data".into());
            if args.first().map(String::as_str) == Some("--into") {
                return backup_rotate(&data, &args[1..]);
            }
            let Some(dest) = args.first().cloned() else {
                eprintln!(
                    "usage: openlens backup <dest-dir> | openlens backup --into <dir> [--keep N]"
                );
                return ExitCode::FAILURE;
            };
            return match openlens_server::backup::run(
                std::path::Path::new(&data),
                std::path::Path::new(&dest),
            ) {
                Ok(n) => {
                    println!("{{\"ok\":true,\"files\":{n},\"dest\":{dest:?}}}");
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("backup failed: {e}");
                    ExitCode::FAILURE
                }
            };
        }
        _ => {}
    }
    match tokio::runtime::Runtime::new() {
        Ok(rt) => rt.block_on(serve()),
        Err(err) => {
            eprintln!("cannot start runtime: {err}");
            ExitCode::FAILURE
        }
    }
}

/// `openlens backup --into <dir> [--keep N]` (N defaults to 7).
fn backup_rotate(data: &str, args: &[String]) -> ExitCode {
    let (into, keep) = match args {
        [into] => (into, Ok(7)),
        [into, flag, n] if flag == "--keep" => (into, n.parse::<usize>()),
        _ => {
            eprintln!("usage: openlens backup --into <dir> [--keep N]");
            return ExitCode::FAILURE;
        }
    };
    let Ok(keep) = keep else {
        eprintln!("--keep needs a number");
        return ExitCode::FAILURE;
    };
    let into = std::path::Path::new(into);
    if let Err(e) = std::fs::create_dir_all(into) {
        eprintln!("backup failed: {}: {e}", into.display());
        return ExitCode::FAILURE;
    }
    match openlens_server::backup::rotate(
        std::path::Path::new(data),
        into,
        keep,
        chrono::Utc::now(),
    ) {
        Ok((dest, n, removed)) => {
            println!(
                "{{\"ok\":true,\"files\":{n},\"dest\":{:?},\"removed\":{}}}",
                dest.display().to_string(),
                removed.len()
            );
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("backup failed: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Probe the local `/healthz` endpoint without any HTTP client dependency.
fn healthcheck() -> ExitCode {
    use std::io::{Read, Write};
    use std::time::Duration;

    let addr = std::env::var("OPENLENS_ADDR").unwrap_or_else(|_| DEFAULT_ADDR.to_owned());
    let port = addr.rsplit(':').next().unwrap_or("8686");
    let probe = || -> std::io::Result<bool> {
        let mut stream = std::net::TcpStream::connect(format!("127.0.0.1:{port}"))?;
        stream.set_read_timeout(Some(Duration::from_secs(2)))?;
        stream.write_all(b"GET /healthz HTTP/1.0\r\nHost: localhost\r\n\r\n")?;
        let mut head = [0u8; 12];
        stream.read_exact(&mut head)?;
        Ok(head.ends_with(b" 200"))
    };
    match probe() {
        Ok(true) => ExitCode::SUCCESS,
        _ => ExitCode::FAILURE,
    }
}

async fn serve() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let state = match build_state() {
        Ok(s) => s,
        Err(err) => {
            tracing::error!(%err, "cannot open data directory");
            return ExitCode::FAILURE;
        }
    };
    tokio::spawn(openlens_server::background(state.clone()));

    let addr = std::env::var("OPENLENS_ADDR").unwrap_or_else(|_| DEFAULT_ADDR.to_owned());
    let listener = match tokio::net::TcpListener::bind(&addr).await {
        Ok(listener) => listener,
        Err(err) => {
            tracing::error!(%addr, %err, "cannot bind listen address");
            return ExitCode::FAILURE;
        }
    };
    tracing::info!(%addr, "openlens listening");

    let served = openlens_server::serve(
        listener,
        openlens_server::router(state.clone()),
        shutdown_signal(),
    )
    .await;
    match openlens_server::flush_now(&state) {
        Ok(n) => tracing::info!(n, "flushed buffered events on shutdown"),
        Err(err) => tracing::error!(%err, "final flush failed; events stay in the WAL"),
    }
    if let Err(err) = served {
        tracing::error!(%err, "server stopped with an error");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

async fn shutdown_signal() {
    let ctrl_c = async {
        if let Err(err) = tokio::signal::ctrl_c().await {
            tracing::warn!(%err, "cannot listen for ctrl-c");
            std::future::pending::<()>().await;
        }
    };
    #[cfg(unix)]
    let term = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut s) => {
                s.recv().await;
            }
            Err(err) => {
                tracing::warn!(%err, "cannot listen for SIGTERM");
                std::future::pending::<()>().await;
            }
        }
    };
    #[cfg(not(unix))]
    let term = std::future::pending::<()>();
    tokio::select! {
        () = ctrl_c => {},
        () = term => {},
    }
}

fn build_state() -> Result<openlens_server::AppState, String> {
    use std::sync::{Arc, Mutex};

    let data_dir = std::path::PathBuf::from(
        std::env::var("OPENLENS_DATA_DIR").unwrap_or_else(|_| "./data".to_owned()),
    );
    std::fs::create_dir_all(&data_dir).map_err(|e| e.to_string())?;
    // Before anything opens the data: refuse data from a newer release, and
    // back up when the version changed (#199).
    match upgrade::prepare(&data_dir, env!("CARGO_PKG_VERSION"))? {
        upgrade::Outcome::Upgraded { from, backup } => {
            tracing::info!(%from, to = env!("CARGO_PKG_VERSION"), %backup, "upgraded; backup taken first");
        }
        upgrade::Outcome::Fresh | upgrade::Outcome::Same => {}
    }
    let meta = openlens_meta::Meta::open(&data_dir.join("meta.db")).map_err(|e| e.to_string())?;

    // Projects are created by agents (`openlens-cli init --name <app>`). A
    // deploy that wants a capture key ready at once (CI, a demo, a template)
    // sets OPENLENS_PROJECT_KEY, and gets a project "default" with that key.
    if let Some(key) = std::env::var("OPENLENS_PROJECT_KEY")
        .ok()
        .filter(|k| !k.is_empty())
        && meta.projects().map_err(|e| e.to_string())?.is_empty()
    {
        let project = meta
            .ensure_default_project(Some(&key))
            .map_err(|e| e.to_string())?;
        tracing::info!(project = %project.id, "created the default project for OPENLENS_PROJECT_KEY");
    }
    let admin_token_hash = match std::env::var("OPENLENS_ADMIN_TOKEN")
        .ok()
        .filter(|t| !t.is_empty())
    {
        Some(t) => openlens_meta::sha256_hex(&t),
        None => {
            let (hash, generated) = meta.admin_token_hash().map_err(|e| e.to_string())?;
            if generated.is_some() {
                // The token itself never reaches the log (#342): a one-hour,
                // single-use owner link does.
                match meta.create_login_link_for(None, 3600) {
                    Ok(link) => {
                        let base = std::env::var("OPENLENS_PUBLIC_URL").unwrap_or_default();
                        let owner_link = format!("{}/l/{link}", base.trim_end_matches('/'));
                        tracing::info!(%owner_link, "generated admin token, stored hashed and never shown. Open this single-use owner link within 1 hour, then create API tokens in Settings → API tokens (or set OPENLENS_ADMIN_TOKEN to choose your own token)");
                    }
                    Err(err) => {
                        tracing::error!(%err, "generated admin token, but no owner link could be made: set OPENLENS_ADMIN_TOKEN and restart")
                    }
                }
            }
            hash
        }
    };

    let (writer, recovered) =
        openlens_storage::Writer::open(&data_dir, openlens_storage::FlushPolicy::default())
            .map_err(|e| e.to_string())?;
    if recovered > 0 {
        tracing::info!(recovered, "recovered events from the write-ahead log");
    }
    let session_key = openlens_server::secrets::session_key(&data_dir)?;
    let session_gen = openlens_server::tokens::session_generation(&meta);
    Ok(openlens_server::AppState {
        meta: Arc::new(meta),
        writer: Arc::new(Mutex::new(writer)),
        query: openlens_query::QueryEngine::new(&data_dir, openlens_query::Limits::default()),
        data_dir,
        admin_token_hash: admin_token_hash.into(),
        session_key: session_key.into(),
        boards: Default::default(),
        flushing: Default::default(),
        metrics: Default::default(),
        disk_space: openlens_server::disk_space,
        session_gen: Arc::new(std::sync::atomic::AtomicU64::new(session_gen)),
        caches: Default::default(),
    })
}
