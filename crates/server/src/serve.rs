//! The HTTP server loop (#346): `axum::serve` with a header read timeout,
//! which `axum::serve` does not expose. A client that opens a connection
//! and sends its headers slowly, or never finishes them, is cut off after
//! [`HEADER_READ_TIMEOUT`] instead of holding the connection open.

use std::future::Future;
use std::net::SocketAddr;
use std::time::Duration;

use axum::Router;
use axum::extract::ConnectInfo;
use hyper::body::Incoming;
use hyper_util::rt::{TokioExecutor, TokioIo, TokioTimer};
use hyper_util::server::conn::auto::Builder;
use hyper_util::server::graceful::GracefulShutdown;
use hyper_util::service::TowerToHyperService;
use tokio::net::TcpListener;
use tower::ServiceExt;

/// How long a client has to send a request's headers. Fixed (D-014).
pub const HEADER_READ_TIMEOUT: Duration = Duration::from_secs(10);

/// How long open connections get to finish once shutdown starts.
const DRAIN: Duration = Duration::from_secs(30);

/// Serve `app` on `listener` until `shutdown` resolves, then let open
/// connections finish (at most [`DRAIN`]). Handlers can read the client
/// address with `ConnectInfo<SocketAddr>`, as with
/// `into_make_service_with_connect_info`.
pub async fn serve(
    listener: TcpListener,
    app: Router,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> std::io::Result<()> {
    let mut builder = Builder::new(TokioExecutor::new());
    builder
        .http1()
        .timer(TokioTimer::new())
        .header_read_timeout(HEADER_READ_TIMEOUT);
    let graceful = GracefulShutdown::new();
    tokio::pin!(shutdown);
    loop {
        let (stream, peer) = tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok(a) => a,
                Err(err) => {
                    // Out of file descriptors and the like: wait, then go on.
                    tracing::warn!(%err, "accept failed");
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    continue;
                }
            },
            () = &mut shutdown => break,
        };
        let app = app.clone();
        let svc = tower::service_fn(move |mut req: hyper::Request<Incoming>| {
            req.extensions_mut().insert(ConnectInfo::<SocketAddr>(peer));
            app.clone().oneshot(req)
        });
        let conn = builder
            .serve_connection_with_upgrades(TokioIo::new(stream), TowerToHyperService::new(svc))
            .into_owned();
        let conn = graceful.watch(conn);
        tokio::spawn(async move {
            let _ = conn.await;
        });
    }
    drop(listener);
    if tokio::time::timeout(DRAIN, graceful.shutdown())
        .await
        .is_err()
    {
        tracing::warn!("connections still open after the drain time; closing them");
    }
    Ok(())
}
