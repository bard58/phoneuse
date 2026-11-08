//! MCP server that lets an external agent (Claude Code, Claude Desktop,
//! Cursor, any MCP client) see and operate a phone. Port of phoneuse
//! `mcp_server.py`.
//!
//! - [`PhoneServer`]: the tool handler, generic over a [`DeviceSource`] so the
//!   CLI can plug in the iOS / Android providers ([`ProviderSource`]).
//! - [`serve_stdio`]: run over stdin/stdout (what MCP clients launch).
//! - [`serve_http`] (feature `http`, default): streamable HTTP on a loopback address.
//!
//! Tools: look, wait, tap, tap_on, long_press, swipe, scroll, type_text,
//! press_key, button, open_app, navigate, list_apps, ui_tree, end_session,
//! status. All coordinates are normalized fractions of the screen (0..1 from
//! the top-left corner), like [`ap_core::Point`]. Each action returns the
//! screen: a JPEG frame strip (+0.1/+0.4/+0.8 s, unchanged frames dropped)
//! and a text block with the strip legend and, with OCR, `"x y  text"` lines.
//!
//! Logging must go to stderr: stdout carries the protocol in stdio mode.

mod apps;
mod instructions;
pub mod params;
mod server;
mod source;

#[cfg(feature = "http")]
mod http;

pub use ap_core;
#[cfg(feature = "http")]
pub use http::{DEFAULT_HTTP_ADDR, serve_http, serve_http_listener};
pub use instructions::instructions;
pub use server::{OcrSetting, PhoneServer, ServerOptions};
#[cfg(feature = "mock")]
pub use source::MockSource;
pub use source::{DeviceSource, ProviderSource, StaticSource};

use rmcp::ServiceExt;

/// Error type of the serve functions.
pub type ServeError = Box<dyn std::error::Error + Send + Sync>;

/// Serve `server` over stdin/stdout until the client disconnects.
pub async fn serve_stdio<S: DeviceSource>(server: PhoneServer<S>) -> Result<(), ServeError> {
    let running = server.serve(rmcp::transport::stdio()).await?;
    running.waiting().await?;
    Ok(())
}
