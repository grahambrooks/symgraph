//! symgraph: Semantic code intelligence MCP server
//!
//! Usage:
//!   symgraph serve              Start the MCP server (stdio transport)
//!   symgraph serve --port 8080  Start the MCP server (HTTP transport)
//!   symgraph index [path]       Index a codebase
//!   symgraph status [path]      Show index statistics
//!   symgraph search <query>     Search for symbols
//!   symgraph context <task>     Build context for a task

mod server;

use std::env;

use anyhow::Result;
use tracing::Level;
use tracing_subscriber::FmtSubscriber;

use symgraph::cli::{context_command, index_command, search_command, status_command};

fn main() -> Result<()> {
    let args: Vec<String> = env::args().collect();

    if args.len() < 2 {
        print_usage();
        return Ok(());
    }

    match args[1].as_str() {
        "serve" => {
            // Check for --port flag
            let port = args
                .iter()
                .position(|a| a == "--port")
                .and_then(|i| args.get(i + 1))
                .and_then(|p| p.parse::<u16>().ok());

            let in_memory = args.iter().any(|a| a == "--in-memory");

            if let Some(port) = port {
                server::start_http(port, in_memory)?;
            } else {
                server::start_stdio(in_memory)?;
            }
        }
        "index" => {
            setup_logging();
            let path = args.get(2).map(|s| s.as_str()).unwrap_or(".");
            index_command(path)?;
        }
        "status" => {
            let path = args.get(2).map(|s| s.as_str()).unwrap_or(".");
            status_command(path)?;
        }
        "search" => {
            if args.len() < 3 {
                eprintln!("Usage: symgraph search <query>");
                return Ok(());
            }
            let path = ".";
            let query = &args[2];
            search_command(path, query)?;
        }
        "context" => {
            if args.len() < 3 {
                eprintln!("Usage: symgraph context <task>");
                return Ok(());
            }
            let path = ".";
            let task = args[2..].join(" ");
            context_command(path, &task)?;
        }
        "help" | "--help" | "-h" => {
            print_usage();
        }
        "--version" | "-V" | "version" => {
            print_version();
        }
        cmd => {
            eprintln!("Unknown command: {}", cmd);
            print_usage();
        }
    }

    Ok(())
}

fn print_usage() {
    println!(
        r#"symgraph: Semantic code intelligence MCP server

USAGE:
    symgraph <COMMAND> [OPTIONS]

COMMANDS:
    serve                  Start the MCP server (stdio transport)
    serve --port <PORT>    Start the MCP server (HTTP transport)
    serve --in-memory      Use in-memory database (no filesystem writes)
    index [path]           Index a codebase (default: current directory)
    status [path]          Show index statistics
    search <query>         Search for symbols by name
    context <task>         Build context for a task description
    help                   Show this help message

ENVIRONMENT:
    SYMGRAPH_ROOT           Project root directory (default: current directory)
    SYMGRAPH_IN_MEMORY=1    Use in-memory database (alternative to --in-memory)

EXAMPLES:
    symgraph index                    # Index current directory
    symgraph index ~/projects/myapp   # Index specific directory
    symgraph serve                    # Start MCP server (stdio)
    symgraph serve --port 8080        # Start MCP server (HTTP on port 8080)
    symgraph serve --in-memory        # Start MCP server with in-memory database
    symgraph search "authenticate"    # Find symbols matching "authenticate"
    symgraph context "add user login" # Build context for implementing login
"#
    );
}

fn print_version() {
    println!("symgraph {}", env!("CARGO_PKG_VERSION"));
}

fn setup_logging() {
    let subscriber = FmtSubscriber::builder()
        .with_max_level(Level::INFO)
        .with_target(false)
        .with_writer(std::io::stderr)
        .finish();
    tracing::subscriber::set_global_default(subscriber).ok();
}
