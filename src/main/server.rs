//! MCP server initialization and startup
//!
//! Handles both stdio and HTTP transport modes for the MCP server.

use anyhow::Result;
use rmcp::{
    transport::stdio,
    transport::streamable_http_server::{
        session::local::LocalSessionManager, StreamableHttpServerConfig, StreamableHttpService,
    },
    ServiceExt,
};
use std::sync::Arc;
use tracing::info;

use symgraph::cli::initialize_server_database;
use symgraph::mcp::SymgraphHandler;

/// Start MCP server with stdio transport
#[tokio::main]
pub async fn start_stdio() -> Result<()> {
    super::setup_debug_logging();
    info!("Starting symgraph MCP server (stdio)");

    let (project_root, db) = initialize_server_database()?;
    info!("Project root: {}", project_root);

    let handler = SymgraphHandler::new(db, project_root);
    let service = handler.serve(stdio()).await?;

    info!("MCP server running on stdio");
    service.waiting().await?;

    Ok(())
}

/// Start MCP server with HTTP transport
#[tokio::main]
pub async fn start_http(port: u16) -> Result<()> {
    super::setup_debug_logging();
    info!("Starting symgraph MCP server (HTTP on port {})", port);

    let (project_root, db) = initialize_server_database()?;
    info!("Project root: {}", project_root);

    // Wrap database in Arc for sharing across HTTP sessions
    let db = Arc::new(std::sync::Mutex::new(db));
    let cancellation_token = tokio_util::sync::CancellationToken::new();

    // Create HTTP service - each session gets a handler with shared database
    let service = StreamableHttpService::new(
        move || Ok(SymgraphHandler::new_shared(db.clone(), project_root.clone())),
        LocalSessionManager::default().into(),
        StreamableHttpServerConfig {
            cancellation_token: cancellation_token.child_token(),
            ..Default::default()
        },
    );

    // Create axum router with the MCP endpoint
    let router = axum::Router::new().nest_service("/mcp", service);

    let bind_addr = format!("127.0.0.1:{}", port);
    info!("Listening on http://{}/mcp", bind_addr);

    let tcp_listener = tokio::net::TcpListener::bind(&bind_addr).await?;
    axum::serve(tcp_listener, router)
        .with_graceful_shutdown(async move {
            tokio::signal::ctrl_c().await.ok();
            info!("Shutting down...");
            cancellation_token.cancel();
        })
        .await?;

    Ok(())
}
