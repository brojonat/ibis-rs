//! tpch-bench [--backend duckdb,postgres,sqlite] [--repeat 3] [QUERY_SUBSTRING]
//!
//! Times Ibis's TPC-H queries on ibis-rs. Same JSONL shape as
//! bench/tpch_ibis.py (impl "ibis-rs"), so the two can be joined:
//!   {"impl": "ibis-rs", "backend": "duckdb", "query": "h01", "sf": "0.01",
//!    "rows": 4, "build_s": ..., "compile_s": ..., "execute_s": ..., "error": null}
//! Timings are the minimum over --repeat runs. Uses the sf=0.01 databases from
//! `bench/tpch_data.py`; build with --release for meaningful numbers.

use std::io::Write;
use std::process::ExitCode;
use std::time::Instant;

use ibis_conformance::tpch::{QUERIES, Tpch};
use ibis_conformance::{ALL_BACKENDS, Paths, tpch_backend};
use ibis_core::Result;
use serde_json::json;

const USAGE: &str = "usage: tpch-bench [--backend duckdb,postgres,sqlite] [--repeat N] [QUERY_SUBSTRING]";

fn timed<T>(repeat: usize, mut f: impl FnMut() -> Result<T>) -> Result<(f64, T)> {
    let mut best = f64::INFINITY;
    let mut out = None;
    for _ in 0..repeat.max(1) {
        let t0 = Instant::now();
        let v = f()?;
        best = best.min(t0.elapsed().as_secs_f64());
        out = Some(v);
    }
    Ok((best, out.expect("ran at least once")))
}

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let mut names: Vec<String> = ALL_BACKENDS.iter().map(|s| s.to_string()).collect();
    let (mut repeat, mut filter) = (3usize, String::new());
    while let Some(a) = args.next() {
        match a.as_str() {
            "-h" | "--help" => {
                eprintln!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            "--backend" => match args.next() {
                Some(v) => names = v.split(',').map(String::from).collect(),
                None => return usage(),
            },
            "--repeat" => match args.next().and_then(|v| v.parse().ok()) {
                Some(n) => repeat = n,
                None => return usage(),
            },
            _ => filter = a,
        }
    }
    let paths = Paths::from_repo_root(&Paths::default_root());
    let mut failed = false;
    let mut stdout = std::io::stdout().lock();
    for name in &names {
        let backend = match tpch_backend(&paths, name) {
            Ok(b) => b,
            Err(e) => {
                eprintln!("tpch-bench: {name}: {e}");
                return ExitCode::from(2);
            }
        };
        let db = match Tpch::load(backend.as_ref()) {
            Ok(db) => db,
            Err(e) => {
                eprintln!("tpch-bench: {name}: {e}");
                return ExitCode::from(2);
            }
        };
        for (q, f) in QUERIES.iter().filter(|(q, _)| q.contains(filter.as_str())) {
            let mut rec = json!({"impl": "ibis-rs", "backend": name, "query": q, "sf": "0.01"});
            let r = (|| -> Result<()> {
                let (build_s, expr) = timed(repeat, || f(&db))?;
                let (compile_s, _) = timed(repeat, || backend.compile(&expr))?;
                let (execute_s, batches) = timed(repeat, || backend.execute(&expr))?;
                let rows: usize = batches.iter().map(|b| b.num_rows()).sum();
                rec["rows"] = json!(rows);
                rec["build_s"] = json!(build_s);
                rec["compile_s"] = json!(compile_s);
                rec["execute_s"] = json!(execute_s);
                Ok(())
            })();
            if let Err(e) = r {
                failed = true;
                eprintln!("{name} {q}: {e}");
                rec["error"] = json!(e.to_string());
            }
            if writeln!(stdout, "{rec}").is_err() {
                return ExitCode::FAILURE;
            }
        }
    }
    if failed { ExitCode::FAILURE } else { ExitCode::SUCCESS }
}

fn usage() -> ExitCode {
    eprintln!("{USAGE}");
    ExitCode::from(2)
}
