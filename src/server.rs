//! MCP server initialization and startup
//!
//! Handles both stdio and HTTP transport modes for the MCP server.

use anyhow::Result;
use axum::{
    extract::State,
    http::{header, Request, StatusCode},
    middleware::{self, Next},
    response::Response,
};
use rmcp::{
    transport::stdio,
    transport::streamable_http_server::{
        session::local::LocalSessionManager, StreamableHttpServerConfig, StreamableHttpService,
    },
    ServiceExt,
};
use std::sync::Arc;
use tracing::{info, warn, Level};
use tracing_subscriber::FmtSubscriber;

use symgraph::cli::initialize_server_database;
use symgraph::mcp::{SymgraphHandler, SyncDatabase};

fn setup_debug_logging() {
    let subscriber = FmtSubscriber::builder()
        .with_max_level(Level::DEBUG)
        .with_target(false)
        .with_writer(std::io::stderr)
        .finish();
    tracing::subscriber::set_global_default(subscriber).ok();
}

/// Start MCP server with stdio transport
#[tokio::main]
pub async fn start_stdio(in_memory: bool) -> Result<()> {
    setup_debug_logging();
    info!("Starting symgraph MCP server (stdio)");

    let (project_root, db) = initialize_server_database(in_memory)?;
    info!("Project root: {}", project_root);

    let handler = SymgraphHandler::new(db, project_root);
    let service = handler.serve(stdio()).await?;

    info!("MCP server running on stdio");
    service.waiting().await?;

    Ok(())
}

/// HTTP transport configuration.
///
/// Defaults bind to loopback. If the caller explicitly binds to a non-
/// loopback address without setting `auth_token`, the server refuses to
/// start — running an unauthenticated code-intelligence endpoint on a
/// network interface is a foot-gun, not a feature.
pub struct HttpConfig {
    pub bind: String,
    pub in_memory: bool,
    pub auth_token: Option<String>,
}

/// Start MCP server with HTTP transport.
#[tokio::main]
pub async fn start_http(cfg: HttpConfig) -> Result<()> {
    setup_debug_logging();
    info!("Starting symgraph MCP server (HTTP on {})", cfg.bind);

    let is_loopback = is_loopback_bind(&cfg.bind);
    if !is_loopback && cfg.auth_token.is_none() {
        return Err(anyhow::anyhow!(
            "refusing to start: bind address {} is not loopback and SYMGRAPH_AUTH_TOKEN is not set. \
             Set SYMGRAPH_AUTH_TOKEN to enable bearer auth, or bind to 127.0.0.1.",
            cfg.bind
        ));
    }
    if !is_loopback {
        warn!(
            "binding to non-loopback address {}; bearer auth is required",
            cfg.bind
        );
    }

    let (project_root, db) = initialize_server_database(cfg.in_memory)?;
    info!("Project root: {}", project_root);

    // Wrap database in Arc for sharing across HTTP sessions
    let db = Arc::new(std::sync::RwLock::new(SyncDatabase(db)));
    let cancellation_token = tokio_util::sync::CancellationToken::new();

    // Create HTTP service - each session gets a handler with shared database
    let service = StreamableHttpService::new(
        move || Ok(SymgraphHandler::new_shared(db.clone(), project_root.clone())),
        LocalSessionManager::default().into(),
        {
            let mut config = StreamableHttpServerConfig::default();
            config.cancellation_token = cancellation_token.child_token();
            config
        },
    );

    // Optional bearer-token middleware.
    let auth_state = Arc::new(cfg.auth_token.clone());
    let mcp_routes = axum::Router::new()
        .nest_service("/mcp", service)
        .route_layer(middleware::from_fn_with_state(
            auth_state.clone(),
            require_bearer_auth,
        ));
    let router = axum::Router::new().merge(mcp_routes);

    info!("Listening on http://{}/mcp", cfg.bind);
    if cfg.auth_token.is_some() {
        info!("Bearer-token auth: ENABLED");
    } else {
        info!("Bearer-token auth: disabled (loopback-only)");
    }

    let tcp_listener = tokio::net::TcpListener::bind(&cfg.bind).await?;
    axum::serve(tcp_listener, router)
        .with_graceful_shutdown(async move {
            tokio::signal::ctrl_c().await.ok();
            info!("Shutting down...");
            cancellation_token.cancel();
        })
        .await?;

    Ok(())
}

fn is_loopback_bind(bind: &str) -> bool {
    // Strip the `:port` suffix for parsing.
    let host = bind.rsplit_once(':').map(|(h, _)| h).unwrap_or(bind);
    let host = host.trim_start_matches('[').trim_end_matches(']');
    matches!(host, "" | "127.0.0.1" | "localhost" | "::1")
}

async fn require_bearer_auth(
    State(expected): State<Arc<Option<String>>>,
    req: Request<axum::body::Body>,
    next: Next,
) -> std::result::Result<Response, StatusCode> {
    if let Some(token) = expected.as_ref() {
        let provided = req
            .headers()
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .map(|s| s.to_string());
        match provided {
            Some(p) if constant_time_eq(p.as_bytes(), token.as_bytes()) => Ok(next.run(req).await),
            _ => Err(StatusCode::UNAUTHORIZED),
        }
    } else {
        Ok(next.run(req).await)
    }
}

/// Length-aware constant-time comparison — prevents a timing side-channel
/// from leaking the expected token byte-by-byte.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff: u8 = 0;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_detection() {
        assert!(is_loopback_bind("127.0.0.1:8080"));
        assert!(is_loopback_bind("localhost:8080"));
        assert!(is_loopback_bind("[::1]:8080"));
        assert!(!is_loopback_bind("0.0.0.0:8080"));
        assert!(!is_loopback_bind("192.168.1.10:8080"));
    }

    #[test]
    fn ct_eq_matches_only_on_equal_bytes() {
        assert!(constant_time_eq(b"secret", b"secret"));
        assert!(!constant_time_eq(b"secret", b"secreT"));
        assert!(!constant_time_eq(b"secret", b"secret1"));
        assert!(!constant_time_eq(b"", b"x"));
    }
}
