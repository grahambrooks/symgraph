//! OpenTelemetry traces of symgraph's own use: one span per CLI command
//! and one per MCP tool call, exported as OTLP/JSON over plain HTTP to
//! any collector (habit, an OpenTelemetry Collector, Jaeger).
//!
//! **Configuration.** The standard `OTEL_*` variables, and a `[telemetry]`
//! table in the user config file (`$XDG_CONFIG_HOME/symgraph/config.toml`,
//! else `~/.config/symgraph/config.toml`). The file matters because some
//! launchers (Claude Code's Bash tool and MCP servers, for one) don't pass
//! `OTEL_*` on to the programs they start.
//!
//! ```toml
//! [telemetry]
//! endpoint = "http://127.0.0.1:4318"   # OTLP/HTTP base URL
//! # enabled = false                   # off, whatever the environment says
//! # service_name = "symgraph"
//! # timeout_ms = 500
//! ```
//!
//! Precedence, first match wins:
//! 1. `OTEL_SDK_DISABLED=true`, or `OTEL_TRACES_EXPORTER` without `otlp`: off.
//! 2. `enabled = false` in the file: off.
//! 3. The endpoint: `OTEL_EXPORTER_OTLP_TRACES_ENDPOINT` (as given), then
//!    `OTEL_EXPORTER_OTLP_ENDPOINT` (`/v1/traces` appended), then the
//!    file's `endpoint` (likewise). `enabled = true` with no endpoint
//!    anywhere means `http://127.0.0.1:4318`. No endpoint: off.
//!
//! A gRPC endpoint (port 4317, or `OTEL_EXPORTER_OTLP_PROTOCOL=grpc`) is
//! reached on the same host's OTLP/HTTP port, 4318. `https://` endpoints
//! turn export off: symgraph ships no TLS stack for this. `OTEL_SERVICE_NAME`
//! (then `service_name`), `OTEL_RESOURCE_ATTRIBUTES`,
//! `OTEL_EXPORTER_OTLP_(TRACES_)HEADERS` and
//! `OTEL_EXPORTER_OTLP_(TRACES_)TIMEOUT` (then `timeout_ms`, default 500 ms)
//! are honoured.
//!
//! **What a span carries.** The subcommand or MCP tool name, flag *names*,
//! exit code or success, duration, and the repository's directory name.
//! Never argument values, symbol names, queries, paths or file contents.
//!
//! **Never fails or delays the caller.** A CLI exports once, at exit,
//! within the timeout. The MCP server hands spans to a background thread
//! through a bounded queue and drops them when it is full. A failed export
//! is dropped with a `debug` log line.

use std::io::{Read, Write as _};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{SyncSender, TrySendError, sync_channel};
use std::sync::{Mutex, OnceLock, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Deserialize;
use serde_json::{Value as Json, json};
use tracing::debug;

const DEFAULT_TIMEOUT: Duration = Duration::from_millis(500);
const DEFAULT_ENDPOINT: &str = "http://127.0.0.1:4318";
const VERSION: &str = env!("CARGO_PKG_VERSION");
/// Spans waiting for the background exporter; more are dropped.
const QUEUE: usize = 256;

/// The `[telemetry]` table of the user config file.
#[derive(Debug, Default, Clone, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct FileConfig {
    pub enabled: Option<bool>,
    pub endpoint: Option<String>,
    pub service_name: Option<String>,
    pub timeout_ms: Option<u64>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct ConfigFile {
    telemetry: FileConfig,
}

impl FileConfig {
    /// The `[telemetry]` table of a config file's text. Malformed text is
    /// logged and ignored.
    pub fn parse(text: &str) -> Self {
        match toml::from_str::<ConfigFile>(text) {
            Ok(file) => file.telemetry,
            Err(e) => {
                debug!("telemetry: ignoring the config file: {e}");
                Self::default()
            }
        }
    }

    /// Read from the user config file; missing or unreadable is the default.
    pub fn load() -> Self {
        config_path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .map(|text| Self::parse(&text))
            .unwrap_or_default()
    }
}

/// `$XDG_CONFIG_HOME/symgraph/config.toml`, else `~/.config/symgraph/config.toml`.
pub fn config_path() -> Option<PathBuf> {
    let nonempty = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty());
    let base = nonempty("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| nonempty("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("symgraph").join("config.toml"))
}

/// Where and how spans are sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Exporter {
    host: String,
    port: u16,
    path: String,
    headers: Vec<(String, String)>,
    resource: Vec<(String, String)>,
    timeout: Duration,
}

impl Exporter {
    /// The exporter that `var` (an environment lookup) and `file` configure.
    pub fn configure(var: &dyn Fn(&str) -> Option<String>, file: &FileConfig) -> Option<Self> {
        let get = |key: &str| {
            var(key)
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
        };
        let either = |traces: &str, all: &str| get(traces).or_else(|| get(all));
        if get("OTEL_SDK_DISABLED").is_some_and(|v| v.eq_ignore_ascii_case("true")) {
            return None;
        }
        if get("OTEL_TRACES_EXPORTER").is_some_and(|v| !v.split(',').any(|e| e.trim() == "otlp")) {
            return None;
        }
        if file.enabled == Some(false) {
            return None;
        }
        let base = |url: &str| {
            let url = url.trim().trim_end_matches('/');
            if url.ends_with("/v1/traces") {
                url.to_string()
            } else {
                format!("{url}/v1/traces")
            }
        };
        let url = if let Some(url) = get("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT") {
            url
        } else if let Some(url) = get("OTEL_EXPORTER_OTLP_ENDPOINT") {
            base(&url)
        } else if let Some(url) = file.endpoint.as_deref().filter(|u| !u.trim().is_empty()) {
            base(url)
        } else if file.enabled == Some(true) {
            base(DEFAULT_ENDPOINT)
        } else {
            return None;
        };
        let Some(rest) = url.strip_prefix("http://") else {
            debug!(
                "telemetry: {url} is not plain http://; symgraph exports only OTLP/HTTP without TLS"
            );
            return None;
        };
        let (authority, path) = match rest.find('/') {
            Some(at) => (&rest[..at], rest[at..].to_string()),
            None => (rest, "/v1/traces".to_string()),
        };
        let (host, port) = match authority.rsplit_once(':') {
            Some((host, port)) => (host.to_string(), port.parse::<u16>().ok()?),
            None => (authority.to_string(), 80),
        };
        let protocol = either(
            "OTEL_EXPORTER_OTLP_TRACES_PROTOCOL",
            "OTEL_EXPORTER_OTLP_PROTOCOL",
        );
        // OTLP's gRPC and HTTP ports are 4317 and 4318.
        let port = if port == 4317 || protocol.as_deref() == Some("grpc") {
            4318
        } else {
            port
        };
        let headers = pairs(
            &either(
                "OTEL_EXPORTER_OTLP_TRACES_HEADERS",
                "OTEL_EXPORTER_OTLP_HEADERS",
            )
            .unwrap_or_default(),
        );
        let timeout = either(
            "OTEL_EXPORTER_OTLP_TRACES_TIMEOUT",
            "OTEL_EXPORTER_OTLP_TIMEOUT",
        )
        .and_then(|ms| ms.parse::<u64>().ok())
        .or(file.timeout_ms)
        .map_or(DEFAULT_TIMEOUT, Duration::from_millis);

        let mut resource = pairs(&get("OTEL_RESOURCE_ATTRIBUTES").unwrap_or_default());
        resource.retain(|(k, _)| k != "service.version" && k != "habit.kind");
        let service = get("OTEL_SERVICE_NAME")
            .or_else(|| {
                resource
                    .iter()
                    .find(|(k, _)| k == "service.name")
                    .map(|(_, v)| v.clone())
            })
            .or_else(|| file.service_name.clone().filter(|s| !s.trim().is_empty()))
            .unwrap_or_else(|| "symgraph".to_string());
        resource.retain(|(k, _)| k != "service.name");
        resource.insert(0, ("service.name".into(), service));
        resource.insert(1, ("service.version".into(), VERSION.to_string()));
        if !resource.iter().any(|(k, _)| k == "vcs.repository.name")
            && let Some(repo) = std::env::current_dir()
                .ok()
                .and_then(|d| repository_name(&d))
        {
            resource.push(("vcs.repository.name".into(), repo));
        }
        Some(Self {
            host,
            port,
            path,
            headers,
            resource,
            timeout,
        })
    }

    /// The OTLP/JSON request for one finished span.
    pub fn body(&self, span: &SpanData) -> Json {
        let mut resource = self.resource.clone();
        resource.push(("habit.kind".into(), span.kind.as_str().into()));
        let nanos = |t: SystemTime| {
            t.duration_since(UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos())
                .to_string()
        };
        let mut out = json!({
            "traceId": hex(&span.trace_id),
            "spanId": hex(&span.span_id),
            "name": span.name,
            "kind": 1, // SPAN_KIND_INTERNAL
            "startTimeUnixNano": nanos(span.start),
            "endTimeUnixNano": nanos(span.end),
            "attributes": span.attributes.iter().map(|(k, v)| json!({"key": k, "value": v})).collect::<Vec<_>>(),
            "status": {"code": 1}, // STATUS_CODE_OK
        });
        if let Some(why) = &span.error {
            out["status"] = json!({"code": 2, "message": why});
        }
        json!({
            "resourceSpans": [{
                "resource": {"attributes": resource.iter().map(|(k, v)| json!({"key": k, "value": {"stringValue": v}})).collect::<Vec<_>>()},
                "scopeSpans": [{
                    "scope": {"name": "symgraph", "version": VERSION},
                    "spans": [out],
                }],
            }],
        })
    }

    /// POST one span, within the timeout; failures are logged and dropped.
    pub fn export(&self, span: &SpanData) {
        let body = self.body(span).to_string();
        match self.post(&body) {
            Ok(code) if (200..300).contains(&code) => {}
            Ok(code) => debug!("telemetry: the collector answered {code}"),
            Err(e) => debug!(
                "telemetry: export to {}:{} failed: {e}",
                self.host, self.port
            ),
        }
    }

    fn post(&self, body: &str) -> std::io::Result<u16> {
        let address = (self.host.as_str(), self.port)
            .to_socket_addrs()?
            .next()
            .ok_or_else(|| std::io::Error::other("the endpoint's host does not resolve"))?;
        let mut stream = TcpStream::connect_timeout(&address, self.timeout)?;
        stream.set_write_timeout(Some(self.timeout))?;
        stream.set_read_timeout(Some(self.timeout))?;
        let mut request = format!(
            "POST {} HTTP/1.1\r\nHost: {}:{}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n",
            self.path,
            self.host,
            self.port,
            body.len()
        );
        for (k, v) in &self.headers {
            request.push_str(&format!("{k}: {v}\r\n"));
        }
        request.push_str("\r\n");
        stream.write_all(request.as_bytes())?;
        stream.write_all(body.as_bytes())?;
        let mut head = [0u8; 12];
        stream.read_exact(&mut head)?;
        // `HTTP/1.1 200`
        std::str::from_utf8(&head[9..12])
            .ok()
            .and_then(|code| code.parse().ok())
            .ok_or_else(|| std::io::Error::other("the endpoint did not answer HTTP"))
    }
}

/// What a span is to habit (`habit.kind` on its resource).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpanKind {
    /// A CLI run: habit's `command` record.
    Command,
    /// An MCP tool call.
    Mcp,
}

impl SpanKind {
    fn as_str(self) -> &'static str {
        match self {
            SpanKind::Command => "command",
            SpanKind::Mcp => "mcp",
        }
    }
}

/// A finished span.
#[derive(Debug, Clone, PartialEq)]
pub struct SpanData {
    pub kind: SpanKind,
    pub trace_id: [u8; 16],
    pub span_id: [u8; 8],
    pub name: String,
    pub start: SystemTime,
    pub end: SystemTime,
    /// OTLP/JSON attribute values (`{"stringValue": …}` and so on).
    pub attributes: Vec<(String, Json)>,
    pub error: Option<String>,
}

enum Sink {
    /// Export on the thread that ends the span (a CLI, at exit).
    Inline(Exporter),
    /// Hand spans to a background thread (the MCP server).
    Queue(SyncSender<SpanData>),
}

static SINK: OnceLock<Sink> = OnceLock::new();
/// The open command span, ended by [`finish_command`].
static COMMAND: Mutex<Option<Span>> = Mutex::new(None);

/// Configure telemetry from the environment and the user config file.
/// `background` (for long-running servers) exports on a worker thread.
/// Returns whether telemetry is on.
pub fn init(background: bool) -> bool {
    if SINK.get().is_some() {
        return true;
    }
    let file = FileConfig::load();
    let Some(exporter) = Exporter::configure(&|k| std::env::var(k).ok(), &file) else {
        return false;
    };
    let sink = if background {
        let (tx, rx) = sync_channel::<SpanData>(QUEUE);
        let spawned = std::thread::Builder::new()
            .name("symgraph-telemetry".into())
            .spawn(move || {
                for span in rx {
                    exporter.export(&span);
                }
            });
        match spawned {
            Ok(_) => Sink::Queue(tx),
            Err(e) => {
                debug!("telemetry: no exporter thread: {e}");
                return false;
            }
        }
    } else {
        Sink::Inline(exporter)
    };
    SINK.set(sink).is_ok()
}

/// Whether telemetry is on.
pub fn enabled() -> bool {
    SINK.get().is_some()
}

fn send(span: SpanData) {
    match SINK.get() {
        Some(Sink::Inline(exporter)) => exporter.export(&span),
        Some(Sink::Queue(tx)) => match tx.try_send(span) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => debug!("telemetry: queue full; span dropped"),
            Err(TrySendError::Disconnected(_)) => {}
        },
        None => {}
    }
}

/// A span; exported when dropped (if telemetry is on).
#[must_use = "a span ends when it is dropped"]
pub struct Span(Option<SpanData>);

impl Span {
    /// Start a span, or an inert one when telemetry is off.
    pub fn start(kind: SpanKind, name: impl Into<String>) -> Self {
        if !enabled() {
            return Span(None);
        }
        Self::recording(kind, name.into())
    }

    fn recording(kind: SpanKind, name: String) -> Self {
        Span(Some(SpanData {
            kind,
            trace_id: random(),
            span_id: random(),
            name,
            start: SystemTime::now(),
            end: SystemTime::now(),
            attributes: Vec::new(),
            error: None,
        }))
    }

    pub fn is_recording(&self) -> bool {
        self.0.is_some()
    }

    pub fn set_str(&mut self, key: &str, value: &str) {
        self.set(key, json!({"stringValue": value}));
    }

    pub fn set_int(&mut self, key: &str, value: i64) {
        // OTLP/JSON carries 64-bit integers as strings.
        self.set(key, json!({"intValue": value.to_string()}));
    }

    pub fn set_bool(&mut self, key: &str, value: bool) {
        self.set(key, json!({"boolValue": value}));
    }

    pub fn set_strs(&mut self, key: &str, values: &[String]) {
        let values: Vec<Json> = values.iter().map(|s| json!({"stringValue": s})).collect();
        self.set(key, json!({"arrayValue": {"values": values}}));
    }

    fn set(&mut self, key: &str, value: Json) {
        if let Some(data) = &mut self.0 {
            match data.attributes.iter_mut().find(|(k, _)| k == key) {
                Some(slot) => slot.1 = value,
                None => data.attributes.push((key.to_string(), value)),
            }
        }
    }

    /// Mark the span's work as failed, with a short error kind (never a message).
    pub fn fail(&mut self, kind: &str) {
        if let Some(data) = &mut self.0 {
            data.error = Some(kind.to_string());
        }
        self.set_str("error.type", kind);
    }

    /// End the span now, returning what would be exported.
    fn end(&mut self) -> Option<SpanData> {
        let mut data = self.0.take()?;
        data.end = SystemTime::now();
        Some(data)
    }
}

impl Drop for Span {
    fn drop(&mut self) {
        if let Some(data) = self.end() {
            send(data);
        }
    }
}

/// The span for a CLI command line: `symgraph <subcommand>`, with the flag
/// names it was given. `known` lists the real subcommand names, so a
/// mistyped one (which could be a path) is never recorded. Long-running
/// `serve` and `watch` get none.
pub fn command_span(argv: &[String], known: &[&str]) -> Span {
    if !enabled() {
        return Span(None);
    }
    build_command_span(argv, known)
}

fn build_command_span(argv: &[String], known: &[&str]) -> Span {
    let sub = argv
        .get(1)
        .map(String::as_str)
        .filter(|a| known.contains(a));
    if matches!(sub, Some("serve" | "watch")) {
        return Span(None);
    }
    let mut span = Span::recording(
        SpanKind::Command,
        match sub {
            Some(sub) => format!("symgraph {sub}"),
            None => "symgraph".to_string(),
        },
    );
    if let Some(sub) = sub {
        span.set_str("habit.subcommand", sub);
    }
    let mut flags: Vec<String> = argv.iter().skip(1).filter_map(|a| flag_name(a)).collect();
    flags.sort();
    flags.dedup();
    if !flags.is_empty() {
        span.set_strs("habit.flags", &flags);
    }
    span
}

/// Open the command span for this process's command line.
pub fn start_command(argv: &[String], known: &[&str]) {
    let span = command_span(argv, known);
    if span.is_recording() {
        *COMMAND.lock().unwrap_or_else(PoisonError::into_inner) = Some(span);
    }
}

/// End the command span with the process's exit code, and export it. Call
/// before exiting; later calls do nothing.
pub fn finish_command(code: i32) {
    let span = COMMAND
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .take();
    if let Some(mut span) = span {
        span.set_int("process.exit.code", i64::from(code));
        if code != 0 {
            span.fail("exit");
        }
    }
}

/// The span for one MCP tool call, `symgraph mcp <tool>`.
pub fn mcp_span(tool: &str) -> Span {
    let mut span = Span::start(SpanKind::Mcp, format!("symgraph mcp {tool}"));
    span.set_str("symgraph.mcp.tool", tool);
    span
}

/// A flag's name, without dashes or value: `--limit=5` → `limit`. `None`
/// for anything that is not a flag (a path, a symbol, `-`).
pub fn flag_name(arg: &str) -> Option<String> {
    let name = arg.strip_prefix('-')?.trim_start_matches('-');
    let name = name.split('=').next().unwrap_or("");
    (!name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'))
    .then(|| name.to_string())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// A random id from the standard library's per-process hash keys and the
/// clock: unique enough for tracing, with no dependency.
fn random<const N: usize>() -> [u8; N] {
    use std::hash::{BuildHasher, Hasher};
    use std::sync::atomic::{AtomicU64, Ordering};
    static IDS: AtomicU64 = AtomicU64::new(0);
    let mut out = [0u8; N];
    for (i, chunk) in out.chunks_mut(8).enumerate() {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u64(IDS.fetch_add(1, Ordering::Relaxed));
        h.write_usize(i);
        h.write_u128(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos()),
        );
        let bytes = h.finish().to_le_bytes();
        chunk.copy_from_slice(&bytes[..chunk.len()]);
    }
    if out.iter().all(|b| *b == 0) {
        out[0] = 1; // all zeros is "no id" in OpenTelemetry
    }
    out
}

/// `k=v,k2=v2` (values percent-decoded), as `OTEL_RESOURCE_ATTRIBUTES` and
/// `OTEL_EXPORTER_OTLP_HEADERS` are written.
fn pairs(text: &str) -> Vec<(String, String)> {
    text.split(',')
        .filter_map(|pair| {
            let (k, v) = pair.split_once('=')?;
            let k = k.trim();
            (!k.is_empty()).then(|| (k.to_string(), percent_decode(v.trim())))
        })
        .collect()
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let pair = bytes
            .get(i + 1..i + 3)
            .and_then(|h| std::str::from_utf8(h).ok())
            .and_then(|h| u8::from_str_radix(h, 16).ok());
        match (bytes[i], pair) {
            (b'%', Some(b)) => {
                out.push(b);
                i += 3;
            }
            (b, _) => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The git repository `dir` is in, by its top directory's name (never the path).
fn repository_name(dir: &Path) -> Option<String> {
    dir.ancestors()
        .find(|d| d.join(".git").exists())
        .and_then(Path::file_name)
        .map(|n| n.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn configured(vars: &[(&str, &str)], file: &FileConfig) -> Option<Exporter> {
        let vars: HashMap<String, String> = vars
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        Exporter::configure(&|k| vars.get(k).cloned(), file)
    }

    fn file(text: &str) -> FileConfig {
        FileConfig::parse(text)
    }

    const ENDPOINT: (&str, &str) = ("OTEL_EXPORTER_OTLP_ENDPOINT", "http://127.0.0.1:4318");

    #[test]
    fn off_without_an_endpoint_or_when_disabled() {
        let none = FileConfig::default();
        assert!(configured(&[], &none).is_none());
        assert!(configured(&[ENDPOINT], &none).is_some());
        assert!(configured(&[ENDPOINT, ("OTEL_SDK_DISABLED", "true")], &none).is_none());
        assert!(configured(&[ENDPOINT, ("OTEL_TRACES_EXPORTER", "none")], &none).is_none());
        assert!(
            configured(
                &[("OTEL_EXPORTER_OTLP_ENDPOINT", "https://x.example")],
                &none
            )
            .is_none()
        );
    }

    #[test]
    fn the_config_file_alone_turns_export_on() {
        let f = file(
            "[telemetry]\nendpoint = \"http://collector:9000\"\ntimeout_ms = 250\nservice_name = \"sg\"\n",
        );
        let e = configured(&[], &f).unwrap();
        assert_eq!(
            (e.host.as_str(), e.port, e.path.as_str()),
            ("collector", 9000, "/v1/traces")
        );
        assert_eq!(e.timeout, Duration::from_millis(250));
        assert_eq!(e.resource[0], ("service.name".into(), "sg".into()));
        // enabled = true with no endpoint: habit's default port on loopback.
        let e = configured(&[], &file("[telemetry]\nenabled = true\n")).unwrap();
        assert_eq!((e.host.as_str(), e.port), ("127.0.0.1", 4318));
    }

    #[test]
    fn the_environment_wins_over_the_file_except_enabled_false() {
        let f = file("[telemetry]\nendpoint = \"http://collector:9000\"\ntimeout_ms = 250\n");
        let e = configured(
            &[
                ("OTEL_EXPORTER_OTLP_ENDPOINT", "http://envhost:4318"),
                ("OTEL_EXPORTER_OTLP_TIMEOUT", "100"),
            ],
            &f,
        )
        .unwrap();
        assert_eq!(e.host, "envhost");
        assert_eq!(e.timeout, Duration::from_millis(100));
        assert!(configured(&[ENDPOINT], &file("[telemetry]\nenabled = false\n")).is_none());
        // OTEL_SDK_DISABLED beats a file that says enabled.
        let on = file("[telemetry]\nenabled = true\nendpoint = \"http://h:4318\"\n");
        assert!(configured(&[("OTEL_SDK_DISABLED", "TRUE")], &on).is_none());
    }

    #[test]
    fn a_malformed_or_foreign_config_file_is_ignored() {
        assert_eq!(file("not = [toml"), FileConfig::default());
        assert_eq!(file("[telemetry]\nendpoint = 3\n"), FileConfig::default());
        assert_eq!(file("[other]\nx = 1\n"), FileConfig::default());
    }

    #[test]
    fn a_grpc_endpoint_is_reached_on_its_hosts_otlp_http_port() {
        let none = FileConfig::default();
        let e = configured(
            &[
                ("OTEL_EXPORTER_OTLP_ENDPOINT", "http://127.0.0.1:4317"),
                ("OTEL_EXPORTER_OTLP_PROTOCOL", "grpc"),
            ],
            &none,
        )
        .unwrap();
        assert_eq!((e.port, e.path.as_str()), (4318, "/v1/traces"));
        let e = configured(
            &[(
                "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT",
                "http://c:9000/custom/traces",
            )],
            &none,
        )
        .unwrap();
        assert_eq!((e.port, e.path.as_str()), (9000, "/custom/traces"));
    }

    #[test]
    fn resource_and_headers_come_from_the_standard_variables() {
        let e = configured(
            &[
                ENDPOINT,
                (
                    "OTEL_RESOURCE_ATTRIBUTES",
                    "team=dev%20tools,service.name=x,habit.kind=bogus",
                ),
                ("OTEL_EXPORTER_OTLP_HEADERS", "x-token=abc"),
            ],
            &FileConfig::default(),
        )
        .unwrap();
        assert_eq!(e.resource[0], ("service.name".into(), "x".into()));
        assert!(e.resource.contains(&("team".into(), "dev tools".into())));
        assert!(!e.resource.iter().any(|(k, _)| k == "habit.kind"));
        assert_eq!(e.headers, [("x-token".to_string(), "abc".to_string())]);
    }

    #[test]
    fn flag_names_never_values_or_positionals() {
        assert_eq!(flag_name("--limit=5").as_deref(), Some("limit"));
        assert_eq!(flag_name("-h").as_deref(), Some("h"));
        assert_eq!(flag_name("-"), None);
        assert_eq!(flag_name("src/main.rs"), None);
        let argv: Vec<String> = [
            "symgraph",
            "callers",
            "secret_fn",
            "--file",
            "src/x.rs",
            "--limit=3",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let mut span = build_command_span(&argv, &["callers"]);
        let data = span.end().unwrap();
        let text = serde_json::to_string(&data.attributes).unwrap();
        assert!(
            text.contains("\"file\"") && text.contains("\"limit\""),
            "{text}"
        );
        assert!(
            !text.contains("secret_fn") && !text.contains("src/x.rs") && !text.contains('3'),
            "{text}"
        );
        assert_eq!(data.name, "symgraph callers");
    }

    #[test]
    fn an_unknown_subcommand_is_not_recorded_and_servers_get_no_span() {
        let argv = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let mut span = build_command_span(&argv(&["symgraph", "/home/me/secret"]), &["status"]);
        let data = span.end().unwrap();
        assert_eq!(data.name, "symgraph");
        assert!(data.attributes.is_empty());
        assert!(!build_command_span(&argv(&["symgraph", "serve"]), &["serve"]).is_recording());
    }

    #[test]
    fn the_body_is_otlp_json_with_habits_kind() {
        let e = configured(&[ENDPOINT], &FileConfig::default()).unwrap();
        let mut span = Span::recording(SpanKind::Command, "symgraph status".into());
        span.set_int("process.exit.code", 2);
        span.fail("exit");
        let body = e.body(&span.end().unwrap());
        let s = &body["resourceSpans"][0]["scopeSpans"][0]["spans"][0];
        assert_eq!(s["name"], "symgraph status");
        assert_eq!(s["status"]["code"], 2);
        assert_eq!(s["attributes"][0]["value"]["intValue"], "2");
        let res = body["resourceSpans"][0]["resource"]["attributes"].to_string();
        assert!(
            res.contains(r#"{"key":"habit.kind","value":{"stringValue":"command"}}"#),
            "{res}"
        );
        assert!(
            res.contains(r#"{"key":"service.name","value":{"stringValue":"symgraph"}}"#),
            "{res}"
        );

        let mut mcp = Span::recording(SpanKind::Mcp, "symgraph mcp symgraph-search".into());
        mcp.set_bool("success", true);
        let res = e.body(&mcp.end().unwrap())["resourceSpans"][0]["resource"].to_string();
        assert!(res.contains(r#""stringValue":"mcp""#), "{res}");
    }

    #[test]
    fn export_posts_the_span_to_the_collector() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut seen = Vec::new();
            let mut buf = [0u8; 4096];
            loop {
                let n = stream.read(&mut buf).unwrap();
                seen.extend_from_slice(&buf[..n]);
                let text = String::from_utf8_lossy(&seen);
                if let Some((head, body)) = text.split_once("\r\n\r\n") {
                    let len: usize = head
                        .lines()
                        .find_map(|l| l.strip_prefix("Content-Length: "))
                        .and_then(|n| n.parse().ok())
                        .unwrap();
                    if body.len() >= len {
                        break;
                    }
                }
            }
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
                .unwrap();
            String::from_utf8(seen).unwrap()
        });
        let endpoint = format!("http://127.0.0.1:{port}");
        let e = configured(
            &[("OTEL_EXPORTER_OTLP_ENDPOINT", &endpoint)],
            &FileConfig::default(),
        )
        .unwrap();
        let mut span = Span::recording(SpanKind::Command, "symgraph where".into());
        e.export(&span.end().unwrap());
        let request = server.join().unwrap();
        assert!(
            request.starts_with("POST /v1/traces HTTP/1.1\r\n"),
            "{request}"
        );
        assert!(request.contains("\"name\":\"symgraph where\""), "{request}");
    }

    #[test]
    fn an_unreachable_collector_is_dropped_quickly() {
        let e = configured(
            &[
                ("OTEL_EXPORTER_OTLP_ENDPOINT", "http://127.0.0.1:9"),
                ("OTEL_EXPORTER_OTLP_TIMEOUT", "200"),
            ],
            &FileConfig::default(),
        )
        .unwrap();
        let mut span = Span::recording(SpanKind::Command, "symgraph x".into());
        let started = std::time::Instant::now();
        e.export(&span.end().unwrap());
        assert!(started.elapsed() < Duration::from_secs(2));
    }
}
