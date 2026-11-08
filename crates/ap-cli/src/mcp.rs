//! All MCP (ap-mcp) wiring for the CLI, behind one function.

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Result;
use ap_core::{Config, DeviceProvider};

/// Default HTTP address for `agentphone mcp --http` without a value.
pub const DEFAULT_HTTP_ADDR: &str = ap_mcp::DEFAULT_HTTP_ADDR;

/// Serve MCP over stdio (default) or streamable HTTP on `http` (loopback
/// only). Returns when the client disconnects. Nothing else may write to
/// stdout while this runs: all logging goes to stderr.
pub async fn serve(cfg: Config, wanted: Option<String>, http: Option<SocketAddr>) -> Result<()> {
    if let Some(addr) = http
        && !addr.ip().is_loopback()
    {
        anyhow::bail!(
            "for safety the MCP HTTP server only listens on this computer: use 127.0.0.1:{}",
            addr.port()
        );
    }
    let providers: Vec<Arc<dyn DeviceProvider>> = crate::devices::providers(&cfg)
        .into_iter()
        .map(Arc::from)
        .collect();
    let device = wanted.or_else(|| cfg.default_device.clone());
    let server = ap_mcp::PhoneServer::new(ap_mcp::ProviderSource::new(providers, device));
    match http {
        None => ap_mcp::serve_stdio(server)
            .await
            .map_err(|e| anyhow::anyhow!("MCP server stopped: {e}"))?,
        Some(addr) => {
            eprintln!("agentphone MCP server on http://{addr}/mcp (Ctrl+C to stop)");
            ap_mcp::serve_http(server, addr)
                .await
                .map_err(|e| anyhow::anyhow!("MCP server stopped: {e}"))?
        }
    }
    Ok(())
}
