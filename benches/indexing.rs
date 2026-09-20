//! Benchmarks for the operations whose cost scales with codebase size.
//!
//! symgraph's value proposition is that it stays fast on a large repository,
//! and nothing previously guarded that. These cover the three things that
//! dominate: building an index from scratch, the no-op incremental pass a
//! watcher runs constantly, and folding the whole graph for coupling analysis.
//!
//! Run with `cargo bench`. The synthetic tree is generated rather than checked
//! in so the numbers do not drift with the repository's own contents.

use std::fs;
use std::path::Path;

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use symgraph::db::Database;
use symgraph::{build_full_index, index_codebase, IndexConfig};
use tempfile::TempDir;

/// Write `files` Rust modules, each with `per_file` functions that call each
/// other and reach across module boundaries — enough structure that reference
/// resolution has real work to do.
fn synthetic_tree(files: usize, per_file: usize) -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src");
    fs::create_dir_all(&src).unwrap();

    for f in 0..files {
        let mut body = String::new();
        // Cross-module imports, so import-scoped resolution is exercised.
        for other in [(f + 1) % files, (f + 7) % files] {
            body.push_str(&format!("use crate::module{other}::Thing{other};\n"));
        }
        body.push_str(&format!("pub struct Thing{f} {{ pub field: u32 }}\n"));
        for i in 0..per_file {
            body.push_str(&format!(
                "/// Does the {i}th thing in module {f}.\n\
                 pub fn helper_{f}_{i}(input: u32) -> u32 {{\n\
                 \x20   let value = input + {i};\n\
                 \x20   shared_name(value)\n\
                 }}\n"
            ));
        }
        // Every module defines `shared_name`, so resolution has to choose.
        body.push_str("fn shared_name(v: u32) -> u32 { v * 2 }\n");
        fs::write(src.join(format!("module{f}.rs")), body).unwrap();
    }

    let lib: String = (0..files)
        .map(|f| format!("pub mod module{f};\n"))
        .collect();
    fs::write(src.join("lib.rs"), lib).unwrap();
    dir
}

fn config_for(root: &Path) -> IndexConfig {
    IndexConfig {
        root: root.display().to_string(),
        show_progress: false,
        ..Default::default()
    }
}

/// Full index build: walk, read, parse, store, resolve.
fn bench_full_index(c: &mut Criterion) {
    let mut group = c.benchmark_group("full_index");
    for files in [25usize, 100] {
        let tree = synthetic_tree(files, 10);
        let config = config_for(tree.path());
        group.throughput(Throughput::Elements(files as u64));
        group.bench_with_input(BenchmarkId::from_parameter(files), &config, |b, config| {
            b.iter(|| {
                let mut db = Database::in_memory().unwrap();
                build_full_index(&mut db, config).unwrap()
            })
        });
    }
    group.finish();
}

/// The pass a file watcher runs on every save with nothing changed. Should be
/// dominated by the directory walk, not by reading and hashing every file.
fn bench_noop_incremental(c: &mut Criterion) {
    let mut group = c.benchmark_group("noop_incremental");
    for files in [25usize, 100] {
        let tree = synthetic_tree(files, 10);
        let config = config_for(tree.path());
        let db_path = tree.path().join("index.db");
        let mut db = Database::open(&db_path).unwrap();
        build_full_index(&mut db, &config).unwrap();

        group.throughput(Throughput::Elements(files as u64));
        group.bench_with_input(BenchmarkId::from_parameter(files), &config, |b, config| {
            b.iter(|| index_codebase(&mut db, config).unwrap())
        });
    }
    group.finish();
}

/// Folding the graph to a module boundary — what every coupling tool does
/// first, and the query that used to pull the whole edge table into memory.
fn bench_edge_endpoints(c: &mut Criterion) {
    let mut group = c.benchmark_group("edge_endpoints");
    for files in [25usize, 100] {
        let tree = synthetic_tree(files, 10);
        let mut db = Database::in_memory().unwrap();
        build_full_index(&mut db, &config_for(tree.path())).unwrap();

        group.throughput(Throughput::Elements(files as u64));
        group.bench_with_input(BenchmarkId::from_parameter(files), &db, |b, db| {
            b.iter(|| db.get_edge_endpoints().unwrap())
        });
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_full_index,
    bench_noop_incremental,
    bench_edge_endpoints
);
criterion_main!(benches);
