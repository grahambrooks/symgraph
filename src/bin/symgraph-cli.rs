//! symgraph-cli: the standalone command-line interface to symgraph.
//!
//! This is the CLI-only sibling of the `symgraph` binary: every query and
//! analysis command, but no MCP `serve` command and none of the server
//! (rmcp/axum/tokio) stack. Build it lean with:
//!
//! ```sh
//! cargo build --release --bin symgraph-cli --no-default-features --features sqlite
//! ```
//!
//! Beyond the shared query commands it adds CLI-oriented extras that the
//! server mode never needed: `index` here is an incremental update (with a
//! full `reindex`), a `watch` command that re-indexes on change, and
//! `completions` / `man` generators for packaging.

use std::env;
use std::io::Write;
use std::time::{Duration, UNIX_EPOCH};

use anyhow::Result;

use symgraph::cli::{
    canonicalize_path, context_command, index_command, open_project_database,
    print_unsupported_types, prune_command, search_command, status_command, tools, where_command,
    OutputFormat,
};
use symgraph::{index_codebase, IndexConfig};

const BIN: &str = "symgraph-cli";

// ---------------------------------------------------------------------------
// Command catalog — the single source of truth for dispatch, help,
// completions, and the man page. Keeping one table means the three generated
// surfaces can never drift out of sync with what the binary actually accepts.
// ---------------------------------------------------------------------------

struct Command {
    name: &'static str,
    args: &'static str,
    /// One-line summary used in help, completions, and the man page.
    help: &'static str,
}

struct Group {
    title: &'static str,
    commands: &'static [Command],
}

#[rustfmt::skip]
const GROUPS: &[Group] = &[
    Group {
        title: "CORE COMMANDS",
        commands: &[
            Command { name: "index", args: "[PATH]", help: "Incrementally update the index (only changed files)" },
            Command { name: "reindex", args: "[PATH]", help: "Full clean rebuild of the index from scratch" },
            Command { name: "watch", args: "[PATH] [--interval SECS]", help: "Re-index on file changes until interrupted" },
            Command { name: "status", args: "[PATH]", help: "Show index statistics (files, symbols, languages)" },
            Command { name: "search", args: "<QUERY>", help: "Find symbols whose name matches QUERY" },
            Command { name: "context", args: "<TASK...>", help: "Build focused context for a coding task" },
            Command { name: "where", args: "[PATH]", help: "Show where this project's index is stored" },
            Command { name: "prune", args: "[--max-age-days N]", help: "Delete stale cached indexes" },
        ],
    },
    Group {
        title: "SYMBOL COMMANDS",
        commands: &[
            Command { name: "callers", args: "<SYMBOL>", help: "Functions/methods that call SYMBOL" },
            Command { name: "callees", args: "<SYMBOL>", help: "Functions/methods that SYMBOL calls" },
            Command { name: "references", args: "<SYMBOL>", help: "All references to SYMBOL" },
            Command { name: "node", args: "<SYMBOL>", help: "Detailed info about a symbol" },
            Command { name: "definition", args: "<SYMBOL> [--context-lines N]", help: "Source of SYMBOL" },
            Command { name: "hierarchy", args: "<SYMBOL>", help: "Parent/child (contains) hierarchy" },
            Command { name: "implementations", args: "<SYMBOL>", help: "Implementations of an interface/trait" },
            Command { name: "file", args: "<PATH>", help: "List symbols defined in a file" },
            Command { name: "path", args: "<FROM> <TO>", help: "Call path(s) from FROM to TO" },
            Command { name: "unused", args: "", help: "Symbols with no incoming references (dead code)" },
        ],
    },
    Group {
        title: "ANALYSIS COMMANDS",
        commands: &[
            Command { name: "impact", args: "<SYMBOL> [--churn] [--days N]", help: "Change impact + coupling breakdown" },
            Command { name: "diff-impact", args: "[--file F --start N --end N --git-ref REF]", help: "Impact of a region/diff" },
            Command { name: "blame", args: "<SYMBOL>", help: "git blame over a symbol's definition lines" },
            Command { name: "churn", args: "[PATH] [--days N]", help: "File change frequency (volatility)" },
            Command { name: "module-graph", args: "[--granularity file|dir|module] [--churn] [--limit N]", help: "Module dependency graph: fan-in/out + cycles" },
            Command { name: "coupling-score", args: "[--granularity ...] [--churn] [--limit N]", help: "Rank coupling by strength × distance × volatility" },
            Command { name: "god-struct", args: "[--churn] [--limit N]", help: "Structs ranked by architectural debt" },
            Command { name: "dispatch-sites", args: "<ENUM>", help: "Files that match/switch on an enum's members" },
        ],
    },
    Group {
        title: "TOOLING",
        commands: &[
            Command { name: "completions", args: "<bash|zsh|fish>", help: "Print a shell completion script" },
            Command { name: "man", args: "", help: "Print a roff man page for symgraph-cli" },
            Command { name: "help", args: "", help: "Show this help" },
            Command { name: "version", args: "", help: "Show the version" },
        ],
    },
];

/// Flat list of every command name, for shell completion.
fn all_command_names() -> Vec<&'static str> {
    GROUPS
        .iter()
        .flat_map(|g| g.commands.iter().map(|c| c.name))
        .collect()
}

// ---------------------------------------------------------------------------
// Argument helpers (mirrors the hand-rolled parsing in src/main.rs).
// ---------------------------------------------------------------------------

/// First positional argument at `idx` that isn't a `--flag`.
fn positional(args: &[String], idx: usize) -> Option<&str> {
    args.get(idx)
        .map(|s| s.as_str())
        .filter(|s| !s.starts_with("--"))
}

/// Required positional argument; prints `usage` and returns None if absent.
fn need(args: &[String], idx: usize, usage: &str) -> Option<String> {
    match positional(args, idx) {
        Some(s) => Some(s.to_string()),
        None => {
            eprintln!("Usage: {BIN} {usage}");
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

fn flag_u64(args: &[String], name: &str) -> Option<u64> {
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

    // Global `--db <path>` override: seeds SYMGRAPH_DB (consulted first by the
    // path resolver). Strip the flag+value so positional parsing is unaffected.
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
        // ---- indexing ----
        "index" => {
            let path = positional(&args, 2).unwrap_or(".");
            index_incremental(path, format)?;
        }
        "reindex" => {
            // Full clean rebuild via the shared shadow-swap path.
            let path = positional(&args, 2).unwrap_or(".");
            index_command(path, format)?;
        }
        "watch" => {
            let path = positional(&args, 2).unwrap_or(".").to_string();
            let interval = flag_u64(&args, "--interval")
                .filter(|s| *s > 0)
                .unwrap_or(2);
            watch_command(&path, interval, format)?;
        }
        "status" => {
            let path = positional(&args, 2).unwrap_or(".");
            status_command(path, format)?;
        }
        "where" => {
            let path = positional(&args, 2).unwrap_or(".");
            where_command(path, format)?;
        }
        "prune" => {
            prune_command(flag_u64(&args, "--max-age-days"), format)?;
        }
        "search" => {
            if args.len() < 3 {
                eprintln!("Usage: {BIN} search <query>");
                eprintln!("  <query> is a symbol name or partial name; quote it if it has spaces.");
                return Ok(());
            }
            search_command(".", &args[2], format)?;
        }
        "context" => {
            if args.len() < 3 {
                eprintln!("Usage: {BIN} context <task...>");
                eprintln!("  <task...> is a free-text description of what you want to work on.");
                return Ok(());
            }
            let task = args[2..].join(" ");
            context_command(".", &task, format)?;
        }

        // ---- symbol relationships ----
        "callers" => {
            if let Some(s) = need(&args, 2, "callers <symbol>") {
                tools::callers(".", &s, format)?;
            }
        }
        "callees" => {
            if let Some(s) = need(&args, 2, "callees <symbol>") {
                tools::callees(".", &s, format)?;
            }
        }
        "node" => {
            if let Some(s) = need(&args, 2, "node <symbol>") {
                tools::node(".", &s, format)?;
            }
        }
        "references" => {
            if let Some(s) = need(&args, 2, "references <symbol>") {
                tools::references(".", &s, format)?;
            }
        }
        "definition" => {
            if let Some(s) = need(&args, 2, "definition <symbol> [--context-lines N]") {
                tools::definition(".", &s, flag_u32(&args, "--context-lines"), format)?;
            }
        }
        "hierarchy" => {
            if let Some(s) = need(&args, 2, "hierarchy <symbol>") {
                tools::hierarchy(".", &s, format)?;
            }
        }
        "implementations" => {
            if let Some(s) = need(&args, 2, "implementations <symbol>") {
                tools::implementations(".", &s, format)?;
            }
        }
        "unused" => {
            tools::unused(".", format)?;
        }
        "file" => {
            if let Some(f) = need(&args, 2, "file <path>") {
                tools::file(".", &f, format)?;
            }
        }
        "path" => match (positional(&args, 2), positional(&args, 3)) {
            (Some(from), Some(to)) => tools::path_between(".", from, to, format)?,
            _ => eprintln!("Usage: {BIN} path <from> <to>"),
        },

        // ---- impact / change analysis ----
        "impact" => {
            if let Some(s) = need(&args, 2, "impact <symbol> [--churn] [--days N]") {
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
            if let Some(s) = need(&args, 2, "blame <symbol>") {
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
            if let Some(s) = need(&args, 2, "dispatch-sites <enum>") {
                tools::dispatch_sites(".", &s, format)?;
            }
        }

        // ---- tooling ----
        "completions" => match positional(&args, 2) {
            Some(shell) => print_completions(shell)?,
            None => {
                eprintln!("Usage: {BIN} completions <bash|zsh|fish>");
                std::process::exit(2);
            }
        },
        "man" => print_man_page(),

        "help" | "--help" | "-h" => print_usage(),
        "--version" | "-V" | "version" => print_version(),
        cmd => {
            eprintln!("Unknown command: {}", cmd);
            print_usage();
            std::process::exit(2);
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Incremental indexing + watch
// ---------------------------------------------------------------------------

/// Incrementally update the project index in place: only files whose content
/// hash changed are re-parsed. Unlike `reindex` (a full shadow rebuild) this is
/// the fast path meant for repeated interactive use.
fn index_incremental(path: &str, fmt: OutputFormat) -> Result<()> {
    let json = fmt.request_format().is_some();
    let project_root = canonicalize_path(path)?;
    let mut db = open_project_database(&project_root)?;

    let config = IndexConfig {
        root: project_root.clone(),
        show_progress: !json,
        ..Default::default()
    };

    let stats = index_codebase(&mut db, &config)?;

    if json {
        println!("{}", serde_json::to_string_pretty(&stats)?);
        return Ok(());
    }

    println!("\nIndex updated (incremental).");
    println!("  Files indexed: {}", stats.files);
    println!("  Files skipped: {}", stats.skipped);
    println!("  Symbols found: {}", stats.nodes);
    println!("  Relationships: {}", stats.edges);
    println!("  Refs resolved: {}", stats.resolved_refs);
    if stats.errors > 0 {
        println!("  Errors: {}", stats.errors);
    }
    print_unsupported_types(&stats.unsupported_types);
    Ok(())
}

/// Re-index whenever the source tree changes, until the process is interrupted.
///
/// This is deliberately dependency-free: rather than hook OS file-system
/// events, it polls a cheap signature (source-file count + newest mtime) every
/// `interval` seconds and runs an incremental index when the signature moves.
/// That keeps the lean CLI build from pulling in a file-watcher crate while
/// still giving fresh queries between edits.
fn watch_command(path: &str, interval: u64, fmt: OutputFormat) -> Result<()> {
    let project_root = canonicalize_path(path)?;

    // Prime the index once so the first query after `watch` starts is current.
    eprintln!("watch: indexing {} ...", project_root);
    index_incremental(&project_root, fmt)?;

    let mut last = scan_signature(&project_root);
    eprintln!(
        "watch: watching {} for changes (every {}s). Press Ctrl-C to stop.",
        project_root, interval
    );

    loop {
        std::thread::sleep(Duration::from_secs(interval));
        let current = scan_signature(&project_root);
        if current != last {
            last = current;
            eprintln!("watch: change detected, re-indexing ...");
            if let Err(err) = index_incremental(&project_root, fmt) {
                eprintln!("watch: index error: {err:#}");
            }
        }
    }
}

/// A cheap change-detection signature over indexable source files: the number
/// of files and the newest modification time (in whole seconds). Any add,
/// remove, or edit moves at least one of the two.
fn scan_signature(root: &str) -> (usize, i64) {
    let defaults = IndexConfig::default();
    let mut count = 0usize;
    let mut newest = 0i64;

    let walker = ignore::WalkBuilder::new(root)
        .hidden(false)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .build();

    for entry in walker.flatten() {
        let p = entry.path();
        if !p.is_file() {
            continue;
        }
        if p.components().any(|c| {
            defaults
                .exclude_dirs
                .iter()
                .any(|d| c.as_os_str() == d.as_str())
        }) {
            continue;
        }
        let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("");
        if !defaults.extensions.iter().any(|e| e == ext) {
            continue;
        }
        count += 1;
        if let Some(secs) = entry
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
        {
            newest = newest.max(secs);
        }
    }

    (count, newest)
}

// ---------------------------------------------------------------------------
// Help / version
// ---------------------------------------------------------------------------

fn print_version() {
    println!("{BIN} {}", env!("CARGO_PKG_VERSION"));
}

fn print_usage() {
    println!(
        "{BIN}: Semantic code intelligence — the standalone CLI\n\n\
         USAGE:\n    {BIN} <COMMAND> [ARGUMENTS] [OPTIONS]\n\n\
         Build the index once with `{BIN} index`, then query it with the\n\
         commands below. `index` updates incrementally; `reindex` rebuilds from\n\
         scratch; `watch` keeps the index fresh as you edit."
    );
    for group in GROUPS {
        println!("\n{}:", group.title);
        for c in group.commands {
            let left = if c.args.is_empty() {
                c.name.to_string()
            } else {
                format!("{} {}", c.name, c.args)
            };
            println!("    {left:<40} {}", c.help);
        }
    }
    println!(
        "\nGLOBAL OPTIONS:\n\
         \x20   --format <text|json>    Output format (default: text)\n\
         \x20   --db <PATH>             Use an explicit index database file\n\n\
         ENVIRONMENT:\n\
         \x20   SYMGRAPH_ROOT           Project root directory (default: cwd)\n\
         \x20   SYMGRAPH_DB             Explicit index database path\n\
         \x20   SYMGRAPH_STORAGE        Index location strategy: git | cache | local\n\n\
         EXAMPLES:\n\
         \x20   {BIN} index                       # incremental index of the current project\n\
         \x20   {BIN} reindex ~/projects/myapp    # full rebuild of a specific project\n\
         \x20   {BIN} watch --interval 5          # re-index on change every 5s\n\
         \x20   {BIN} search authenticate         # find symbols named like \"authenticate\"\n\
         \x20   {BIN} context \"fix the login bug\" # gather context for a task\n\
         \x20   {BIN} coupling-score --limit 20   # architectural coupling hotspots\n\n\
         NOTE:\n\
         \x20   Query commands need an existing index — run `{BIN} index` first,\n\
         \x20   and re-run it (or use `watch`) after code changes to refresh.\n"
    );
}

// ---------------------------------------------------------------------------
// Shell completions
// ---------------------------------------------------------------------------

fn print_completions(shell: &str) -> Result<()> {
    let names = all_command_names().join(" ");
    match shell {
        "bash" => print!("{}", bash_completions(&names)),
        "zsh" => print!("{}", zsh_completions()),
        "fish" => print!("{}", fish_completions()),
        other => {
            eprintln!("Unsupported shell: {other} (expected: bash | zsh | fish)");
            std::process::exit(2);
        }
    }
    Ok(())
}

fn bash_completions(names: &str) -> String {
    format!(
        "# bash completion for {BIN}. Install: {BIN} completions bash > \
         /usr/local/etc/bash_completion.d/{BIN}\n\
         _{ident}() {{\n\
         \x20   local cur cmds\n\
         \x20   cur=\"${{COMP_WORDS[COMP_CWORD]}}\"\n\
         \x20   cmds=\"{names}\"\n\
         \x20   if [ \"$COMP_CWORD\" -eq 1 ]; then\n\
         \x20       COMPREPLY=( $(compgen -W \"$cmds\" -- \"$cur\") )\n\
         \x20   else\n\
         \x20       COMPREPLY=( $(compgen -f -- \"$cur\") )\n\
         \x20   fi\n\
         }}\n\
         complete -F _{ident} {BIN}\n",
        ident = "symgraph_cli",
    )
}

fn zsh_completions() -> String {
    let mut lines = String::new();
    for group in GROUPS {
        for c in group.commands {
            // Escape single quotes and colons for the zsh describe format.
            let desc = c.help.replace('\'', "'\\''").replace(':', "\\:");
            lines.push_str(&format!("        '{}:{}'\n", c.name, desc));
        }
    }
    format!(
        "#compdef {BIN}\n\
         # zsh completion for {BIN}. Install: place this file on your $fpath as _{BIN}\n\
         _{ident}() {{\n\
         \x20   local -a commands\n\
         \x20   commands=(\n{lines}    )\n\
         \x20   if (( CURRENT == 2 )); then\n\
         \x20       _describe '{BIN} command' commands\n\
         \x20   else\n\
         \x20       _files\n\
         \x20   fi\n\
         }}\n\
         _{ident} \"$@\"\n",
        ident = "symgraph_cli",
    )
}

fn fish_completions() -> String {
    let mut out = format!("# fish completion for {BIN}\n");
    for group in GROUPS {
        for c in group.commands {
            let desc = c.help.replace('\'', "\\'");
            out.push_str(&format!(
                "complete -c {BIN} -n '__fish_use_subcommand' -f -a '{}' -d '{}'\n",
                c.name, desc
            ));
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Man page (roff)
// ---------------------------------------------------------------------------

fn print_man_page() {
    let version = env!("CARGO_PKG_VERSION");
    let mut out = String::new();
    out.push_str(&format!(
        ".TH SYMGRAPH-CLI 1 \"\" \"symgraph {version}\" \"User Commands\"\n"
    ));
    out.push_str(".SH NAME\n");
    out.push_str(&format!(
        "{BIN} \\- semantic code intelligence, standalone command-line interface\n"
    ));
    out.push_str(".SH SYNOPSIS\n");
    out.push_str(&format!(
        ".B {BIN}\n.RI [ COMMAND ] \" \" [ ARGUMENTS ] \" \" [ OPTIONS ]\n"
    ));
    out.push_str(".SH DESCRIPTION\n");
    out.push_str(
        "symgraph-cli builds a searchable knowledge graph of a codebase using tree-sitter \
         and answers structural queries about it \\(em callers, callees, impact, coupling, \
         and more. It is the CLI-only build of symgraph and does not include the MCP server.\n",
    );
    out.push_str(".SH COMMANDS\n");
    for group in GROUPS {
        out.push_str(&format!(".SS {}\n", group.title));
        for c in group.commands {
            let head = if c.args.is_empty() {
                c.name.to_string()
            } else {
                format!("{} {}", c.name, c.args)
            };
            out.push_str(&format!(".TP\n.B {}\n{}\n", head, c.help));
        }
    }
    out.push_str(".SH OPTIONS\n");
    out.push_str(".TP\n.B \\-\\-format <text|json>\nOutput format (default: text).\n");
    out.push_str(".TP\n.B \\-\\-db <PATH>\nUse an explicit index database file.\n");
    out.push_str(".SH ENVIRONMENT\n");
    out.push_str(".TP\n.B SYMGRAPH_ROOT\nProject root directory (default: current directory).\n");
    out.push_str(".TP\n.B SYMGRAPH_DB\nExplicit index database path.\n");
    out.push_str(".TP\n.B SYMGRAPH_STORAGE\nIndex location strategy: git | cache | local.\n");
    out.push_str(".SH EXAMPLES\n");
    out.push_str(&format!(
        ".TP\n.B {BIN} index\nIncrementally index the current project.\n"
    ));
    out.push_str(&format!(".TP\n.B {BIN} watch\nRe-index on every change.\n"));
    out.push_str(&format!(
        ".TP\n.B {BIN} coupling-score --limit 20\nShow architectural coupling hotspots.\n"
    ));
    out.push_str(".SH SEE ALSO\n");
    out.push_str(".BR symgraph (1)\n");

    // Write straight to stdout so `... man > symgraph-cli.1` works.
    let stdout = std::io::stdout();
    let mut lock = stdout.lock();
    let _ = lock.write_all(out.as_bytes());
}
