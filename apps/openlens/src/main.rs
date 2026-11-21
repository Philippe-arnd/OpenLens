//! The OpenLens server binary: API, ingestion and UI in one process.
//!
//! Configuration is read from the environment:
//! - `OPENLENS_ADDR`: listen address (default `0.0.0.0:8686`).
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

    let addr = std::env::var("OPENLENS_ADDR").unwrap_or_else(|_| DEFAULT_ADDR.to_owned());
    let listener = match tokio::net::TcpListener::bind(&addr).await {
        Ok(listener) => listener,
        Err(err) => {
            tracing::error!(%addr, %err, "cannot bind listen address");
            return ExitCode::FAILURE;
        }
    };
    tracing::info!(%addr, "openlens listening");

    let serve =
        axum::serve(listener, openlens_server::router()).with_graceful_shutdown(shutdown_signal());
    if let Err(err) = serve.await {
        tracing::error!(%err, "server stopped with an error");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

async fn shutdown_signal() {
    if let Err(err) = tokio::signal::ctrl_c().await {
        tracing::warn!(%err, "cannot listen for shutdown signal");
        std::future::pending::<()>().await;
    }
}
