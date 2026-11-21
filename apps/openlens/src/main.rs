//! The OpenLens server binary: API, ingestion and UI in one process.
//!
//! Configuration is read from the environment:
//! - `OPENLENS_ADDR`: listen address (default `0.0.0.0:8686`).
//! - `OPENLENS_DATA_DIR`: data root (default `./data`).
//! - `OPENLENS_ADMIN_TOKEN`: admin token; generated and stored on first start
//!   when unset.
//! - `OPENLENS_PROJECT_KEY`: capture key of the default project, used only
//!   when the first project is created (lets a deploy template choose it).
//!
//! `openlens healthcheck` probes `/healthz` on the local port and exits 0 or 1;
//! the container image has no shell or curl, so the compose healthcheck uses it.

use std::process::ExitCode;

const DEFAULT_ADDR: &str = "0.0.0.0:8686";

fn main() -> ExitCode {
    if std::env::args().nth(1).as_deref() == Some("healthcheck") {
        return healthcheck();
    }
    match tokio::runtime::Runtime::new() {
        Ok(rt) => rt.block_on(serve()),
        Err(err) => {
            eprintln!("cannot start runtime: {err}");
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

    let serve = axum::serve(listener, openlens_server::router(state.clone()))
        .with_graceful_shutdown(shutdown_signal());
    let served = serve.await;
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
    let meta = openlens_meta::Meta::open(&data_dir.join("meta.db")).map_err(|e| e.to_string())?;

    let first_start = meta.projects().map_err(|e| e.to_string())?.is_empty();
    let key = std::env::var("OPENLENS_PROJECT_KEY")
        .ok()
        .filter(|k| !k.is_empty());
    let project = meta
        .ensure_default_project(key.as_deref())
        .map_err(|e| e.to_string())?;
    let admin_token = match std::env::var("OPENLENS_ADMIN_TOKEN")
        .ok()
        .filter(|t| !t.is_empty())
    {
        Some(t) => t,
        None => meta.admin_token().map_err(|e| e.to_string())?,
    };
    if first_start {
        tracing::info!(project = %project.id, capture_key = %project.api_key, "created default project");
        if std::env::var("OPENLENS_ADMIN_TOKEN").is_err() {
            tracing::info!(%admin_token, "generated admin token (store it; set OPENLENS_ADMIN_TOKEN to choose your own)");
        }
    }

    let (writer, recovered) =
        openlens_storage::Writer::open(&data_dir, openlens_storage::FlushPolicy::default())
            .map_err(|e| e.to_string())?;
    if recovered > 0 {
        tracing::info!(recovered, "recovered events from the write-ahead log");
    }
    Ok(openlens_server::AppState {
        meta: Arc::new(meta),
        writer: Arc::new(Mutex::new(writer)),
        query: openlens_query::QueryEngine::new(&data_dir, openlens_query::Limits::default()),
        data_dir,
        admin_token: admin_token.into(),
    })
}
