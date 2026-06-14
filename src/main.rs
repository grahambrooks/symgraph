//! symgraph: Semantic code intelligence MCP server and CLI.
//!
//! Commands: index, status, search, context, where, prune, serve.
//! Run `symgraph help` for full syntax, arguments, options, and examples.

mod server;

use std::env;

use anyhow::Result;
use tracing::Level;
use tracing_subscriber::FmtSubscriber;

use symgraph::cli::{
    context_command, index_command, prune_command, search_command, status_command, tools,
    where_command, OutputFormat,
};

/// First positional argument at `idx` that isn't a `--flag`.
fn positional(args: &[String], idx: usize) -> Option<&str> {
    args.get(idx)
        .map(|s| s.as_str())
        .filter(|s| !s.starts_with("--"))
}

/// Required positional symbol/path argument; prints `usage` and returns None if absent.
fn need(args: &[String], idx: usize, usage: &str) -> Option<String> {
    match positional(args, idx) {
        Some(s) => Some(s.to_string()),
        None => {
            eprintln!("Usage: {usage}");
            None
        }
    }
}

fn flag_value(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

fn flag_u32(args: &[String], name: &str) -> Option<u32> {
    flag_value(args, name).and_then(|s| s.parse().ok())
}

fn has_flag(args: &[String], name: &str) -> bool {
    args.iter().any(|a| a == name)
}

fn main() -> Result<()> {
    let mut args: Vec<String> = env::args().collect();

    if args.len() < 2 {
        print_usage();
        return Ok(());
    }

    // Global `--db <path>` override: applies to every command (CLI + serve) by
    // seeding SYMGRAPH_DB, which the path resolver consults first. Strip the
    // flag and its value so the remaining positional arguments are unaffected.
    if let Some(i) = args.iter().position(|a| a == "--db") {
        if i + 1 < args.len() {
            env::set_var("SYMGRAPH_DB", &args[i + 1]);
            args.drain(i..=i + 1);
        }
    }

    // Global `--format <text|json>` for command output (default: text).
    let mut format = OutputFormat::Text;
    if let Some(i) = args.iter().position(|a| a == "--format") {
        match args.get(i + 1).map(|s| OutputFormat::parse(s)) {
            Some(Some(f)) => {
                format = f;
                args.drain(i..=i + 1);
            }
            _ => {
                eprintln!("Invalid or missing value for --format (expected: text | json)");
                std::process::exit(2);
            }
        }
    }

    match args[1].as_str() {
        "serve" => {
            let port = args
                .iter()
                .position(|a| a == "--port")
                .and_then(|i| args.get(i + 1))
                .and_then(|p| p.parse::<u16>().ok());

            // Explicit bind override (e.g. `--bind 0.0.0.0:8080`). Takes
            // precedence over --port when both are given.
            let bind = args
                .iter()
                .position(|a| a == "--bind")
                .and_then(|i| args.get(i + 1))
                .cloned();

            let in_memory = args.iter().any(|a| a == "--in-memory");
            let auth_token = env::var("SYMGRAPH_AUTH_TOKEN")
                .ok()
                .filter(|s| !s.is_empty());

            match (bind, port) {
                (Some(bind), _) => server::start_http(server::HttpConfig {
                    bind,
                    in_memory,
                    auth_token,
                })?,
                (None, Some(port)) => server::start_http(server::HttpConfig {
                    bind: format!("127.0.0.1:{}", port),
                    in_memory,
                    auth_token,
                })?,
                (None, None) => server::start_stdio(in_memory)?,
            }
        }
        "index" => {
            let path = args.get(2).map(|s| s.as_str()).unwrap_or(".");
            // Log to a file beside the index so background indexing never
            // writes into the working tree.
            setup_logging(symgraph::cli::index_log_path(path).ok().as_deref());
            index_command(path, format)?;
        }
        "status" => {
            let path = args.get(2).map(|s| s.as_str()).unwrap_or(".");
            status_command(path, format)?;
        }
        "where" => {
            let path = args.get(2).map(|s| s.as_str()).unwrap_or(".");
            where_command(path, format)?;
        }
        "prune" => {
            prune_command(format)?;
        }
        "search" => {
            if args.len() < 3 {
                eprintln!("Usage: symgraph search <query>");
                eprintln!("  <query> is a symbol name or partial name; quote it if it has spaces.");
                eprintln!(
                    "  e.g. symgraph search authenticate   |   symgraph search \"User Service\""
                );
                return Ok(());
            }
            let path = ".";
            let query = &args[2];
            search_command(path, query, format)?;
        }
        "context" => {
            if args.len() < 3 {
                eprintln!("Usage: symgraph context <task...>");
                eprintln!("  <task...> is a free-text description of what you want to work on.");
                eprintln!("  e.g. symgraph context \"add OAuth login to the REST API\"");
                return Ok(());
            }
            let path = ".";
            let task = args[2..].join(" ");
            context_command(path, &task, format)?;
        }

        // ---- MCP-tool parity: symbol relationships ----
        "callers" => {
            if let Some(s) = need(&args, 2, "symgraph callers <symbol>") {
                tools::callers(".", &s, format)?;
            }
        }
        "callees" => {
            if let Some(s) = need(&args, 2, "symgraph callees <symbol>") {
                tools::callees(".", &s, format)?;
            }
        }
        "node" => {
            if let Some(s) = need(&args, 2, "symgraph node <symbol>") {
                tools::node(".", &s, format)?;
            }
        }
        "references" => {
            if let Some(s) = need(&args, 2, "symgraph references <symbol>") {
                tools::references(".", &s, format)?;
            }
        }
        "definition" => {
            if let Some(s) = need(&args, 2, "symgraph definition <symbol> [--context-lines N]") {
                tools::definition(".", &s, flag_u32(&args, "--context-lines"), format)?;
            }
        }
        "hierarchy" => {
            if let Some(s) = need(&args, 2, "symgraph hierarchy <symbol>") {
                tools::hierarchy(".", &s, format)?;
            }
        }
        "implementations" => {
            if let Some(s) = need(&args, 2, "symgraph implementations <symbol>") {
                tools::implementations(".", &s, format)?;
            }
        }
        "unused" => {
            tools::unused(".", format)?;
        }
        "file" => {
            if let Some(f) = need(&args, 2, "symgraph file <path>") {
                tools::file(".", &f, format)?;
            }
        }
        "path" => match (positional(&args, 2), positional(&args, 3)) {
            (Some(from), Some(to)) => tools::path_between(".", from, to, format)?,
            _ => eprintln!("Usage: symgraph path <from> <to>"),
        },

        // ---- impact / change analysis ----
        "impact" => {
            if let Some(s) = need(&args, 2, "symgraph impact <symbol> [--churn] [--days N]") {
                tools::impact(
                    ".",
                    &s,
                    format,
                    has_flag(&args, "--churn"),
                    flag_u32(&args, "--days"),
                )?;
            }
        }
        "diff-impact" => {
            tools::diff_impact(
                ".",
                flag_value(&args, "--file"),
                flag_u32(&args, "--start"),
                flag_u32(&args, "--end"),
                flag_value(&args, "--git-ref"),
            )?;
        }

        // ---- git history ----
        "blame" => {
            if let Some(s) = need(&args, 2, "symgraph blame <symbol>") {
                tools::blame(".", &s)?;
            }
        }
        "churn" => {
            tools::churn(
                ".",
                positional(&args, 2).map(|s| s.to_string()),
                flag_u32(&args, "--days"),
            )?;
        }

        // ---- coupling & architecture ----
        "module-graph" => {
            tools::module_graph(
                ".",
                flag_value(&args, "--granularity"),
                has_flag(&args, "--churn"),
                flag_u32(&args, "--days"),
                flag_u32(&args, "--limit"),
                format,
            )?;
        }
        "coupling-score" => {
            tools::coupling_score(
                ".",
                flag_value(&args, "--granularity"),
                has_flag(&args, "--churn"),
                flag_u32(&args, "--days"),
                flag_u32(&args, "--limit"),
                format,
            )?;
        }
        "god-struct" => {
            tools::god_struct(
                ".",
                has_flag(&args, "--churn"),
                flag_u32(&args, "--days"),
                flag_u32(&args, "--limit"),
                format,
            )?;
        }
        "dispatch-sites" => {
            if let Some(s) = need(&args, 2, "symgraph dispatch-sites <enum>") {
                tools::dispatch_sites(".", &s, format)?;
            }
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
        r#"symgraph: Semantic code intelligence — a searchable knowledge graph of your code

USAGE:
    symgraph <COMMAND> [ARGUMENTS] [OPTIONS]

    Build the index once with `symgraph index`, then query it with the commands
    below. The same on-disk index is shared by the CLI and the MCP server.

CORE COMMANDS:
    index [PATH]             Build or refresh the index for a project
    status [PATH]            Show index statistics (files, symbols, languages)
    search <QUERY>           Find symbols whose name matches QUERY
    context <TASK...>        Build focused context for a coding task
    where [PATH]             Show where this project's index is stored
    prune                    Delete cached indexes whose repo no longer exists
    serve [OPTIONS]          Run the MCP server (see SERVE OPTIONS)
    help, version            Show this help / the version

SYMBOL COMMANDS (query the current project's index):
    callers <SYMBOL>         Functions/methods that call SYMBOL
    callees <SYMBOL>         Functions/methods that SYMBOL calls
    references <SYMBOL>      All references to SYMBOL
    node <SYMBOL>            Detailed info about a symbol
    definition <SYMBOL>     Source of SYMBOL        [--context-lines N]
    hierarchy <SYMBOL>      Parent/child (contains) hierarchy
    implementations <SYMBOL> Implementations of an interface/trait
    file <PATH>              List symbols defined in a file
    path <FROM> <TO>         Call path(s) from FROM to TO
    unused                   Symbols with no incoming references (dead code)

ANALYSIS COMMANDS:
    impact <SYMBOL>          Change impact + coupling breakdown  [--churn] [--days N]
    diff-impact              Impact of a region/diff
                             [--file F --start N --end N --git-ref REF]
    blame <SYMBOL>           git blame over a symbol's definition lines
    churn [PATH]             File change frequency (volatility)  [--days N]
    module-graph             Module dependency graph: fan-in/out + cycles
                             [--granularity file|dir|module] [--churn] [--limit N]
    coupling-score           Rank coupling by strength × distance × volatility
                             [--granularity ...] [--churn] [--limit N]
    god-struct               Structs ranked by architectural debt  [--churn] [--limit N]
    dispatch-sites <ENUM>    Files that match/switch on an enum's members

ARGUMENTS:
    <QUERY>      A symbol name or partial name (case-insensitive, prefix/
                 substring match). Returns matching functions, types, methods,
                 etc. with their file:line. Quote it if it contains spaces:
                     symgraph search authenticate
                     symgraph search "User Service"

    <TASK...>    A free-text description of what you want to work on. Every word
                 after the command becomes the task (quotes optional but clearer).
                 symgraph returns the relevant entry points and related code:
                     symgraph context "add OAuth login to the REST API"
                     symgraph context why does indexing skip generated files

    [PATH]       Project root to act on (default: current directory). Point it
                 at another checkout to index/query that one instead:
                     symgraph index ~/code/myapp
                     symgraph status ~/code/myapp

SERVE OPTIONS:
    serve                    stdio transport (for editors / Claude Code)
    serve --port <PORT>      HTTP on 127.0.0.1:<PORT>
    serve --bind <ADDR:PORT> HTTP on an explicit address (e.g. 0.0.0.0:8080)
    serve --in-memory        Ephemeral in-memory index (no filesystem writes)

GLOBAL OPTIONS:
    --format <text|json>    Output format (default: text). `json` emits structured
                            output for scripts/agents (pipe to `jq`). Supported by
                            all commands except blame, churn, and diff-impact.
    --db <PATH>             Use an explicit index database file (any command)

ENVIRONMENT:
    SYMGRAPH_ROOT           Project root directory (default: current directory)
    SYMGRAPH_DB             Explicit index database path (overrides storage)
    SYMGRAPH_STORAGE        Index location strategy: git | cache | local.
                            Default: reuse existing .symgraph/, else the git dir
                            (<git-common-dir>/symgraph), else an OS cache dir.
                            `symgraph index` writes its progress log to
                            index.log in this same directory (never the worktree).
    SYMGRAPH_IN_MEMORY=1    Use in-memory database (same as serve --in-memory)
    SYMGRAPH_AUTH_TOKEN     Bearer token required on /mcp (required for non-
                            loopback binds; optional on 127.0.0.1)

EXAMPLES:
    symgraph index                       # index the current project
    symgraph index ~/projects/myapp      # index a specific project
    symgraph status                      # how much is indexed?
    symgraph search authenticate         # find symbols named like "authenticate"
    symgraph search "User Service"       # quote multi-word queries
    symgraph search auth --format json   # machine-readable output for scripts
    symgraph status --format json        # JSON stats (pipe to jq)
    symgraph context "fix the login bug" # gather context for a task
    symgraph where                       # where is this project's index stored?
    symgraph serve                       # start the MCP server (stdio)
    symgraph serve --port 8080           # start the MCP server over HTTP

NOTE:
    Query commands (status, search, context, where) need an existing index —
    run `symgraph index` first, and re-run it after code changes to refresh.
"#
    );
}

fn print_version() {
    println!("symgraph {}", env!("CARGO_PKG_VERSION"));
}

/// Install the global tracing subscriber. When `log_file` is `Some` and can be
/// created, index progress is written there (co-located with the index, never
/// the working tree); otherwise it falls back to stderr.
fn setup_logging(log_file: Option<&std::path::Path>) {
    let builder = FmtSubscriber::builder()
        .with_max_level(Level::INFO)
        .with_target(false);
    match log_file.and_then(|p| std::fs::File::create(p).ok()) {
        Some(file) => {
            let subscriber = builder
                .with_ansi(false)
                .with_writer(std::sync::Mutex::new(file))
                .finish();
            tracing::subscriber::set_global_default(subscriber).ok();
        }
        None => {
            let subscriber = builder.with_writer(std::io::stderr).finish();
            tracing::subscriber::set_global_default(subscriber).ok();
        }
    }
}
