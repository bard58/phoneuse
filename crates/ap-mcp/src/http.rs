//! Streamable HTTP transport (MCP 2025-03-26+), loopback only.

use std::net::SocketAddr;
use std::sync::Arc;

use hyper_util::rt::TokioIo;
use hyper_util::service::TowerToHyperService;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::{StreamableHttpServerConfig, StreamableHttpService};
use tokio::net::TcpListener;

use crate::{DeviceSource, PhoneServer, ServeError};

/// Default listen address for `serve_http`. The MCP endpoint is
/// `http://127.0.0.1:8778/mcp` (any path is accepted).
pub const DEFAULT_HTTP_ADDR: &str = "127.0.0.1:8778";

/// Serve `server` over streamable HTTP on `addr` until the task is dropped.
/// `addr` must be a loopback address: the phone must not be reachable from
/// the network. Host headers other than localhost / 127.0.0.1 / ::1 are refused
/// (DNS rebinding protection).
pub async fn serve_http<S: DeviceSource>(
    server: PhoneServer<S>,
    addr: SocketAddr,
) -> Result<(), ServeError> {
    if !addr.ip().is_loopback() {
        return Err(format!(
            "refusing to listen on {addr}: the MCP HTTP server only listens on a loopback address such as 127.0.0.1"
        )
        .into());
    }
    let listener = TcpListener::bind(addr).await?;
    serve_http_listener(server, listener).await
}

/// Like [`serve_http`] on an already bound listener (port 0 in tests). The
/// caller is responsible for binding it to a loopback address.
pub async fn serve_http_listener<S: DeviceSource>(
    server: PhoneServer<S>,
    listener: TcpListener,
) -> Result<(), ServeError> {
    let local = listener.local_addr()?;
    if !local.ip().is_loopback() {
        return Err(format!("refusing to serve on non-loopback address {local}").into());
    }
    tracing::info!("MCP streamable HTTP on http://{local}/mcp");
    let service = StreamableHttpService::new(
        move || Ok(server.clone()),
        Arc::new(LocalSessionManager::default()),
        StreamableHttpServerConfig::default(),
    );
    loop {
        let (stream, peer) = listener.accept().await?;
        let svc = TowerToHyperService::new(service.clone());
        tokio::spawn(async move {
            if let Err(e) = hyper::server::conn::http1::Builder::new()
                .serve_connection(TokioIo::new(stream), svc)
                .await
            {
                tracing::debug!(%peer, error = %e, "HTTP connection ended with an error");
            }
        });
    }
}
