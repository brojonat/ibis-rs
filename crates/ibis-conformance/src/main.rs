//! ibis-conformance [--backend duckdb,sqlite] [ID_SUBSTRING]
//!
//! Without --backend, every backend that is up is run and the rest are skipped
//! with a note; a backend named with --backend must be up.
//!
//! stdout: one JSON object per (case, backend).
//! stderr: a pass matrix of non-passing cases and per-backend totals.
//! Exit 1 if any outcome is a problem (fail, error, xpass, no_golden) on a
//! backend that is not experimental.

use std::collections::BTreeMap;
use std::io::Write;
use std::process::ExitCode;

use ibis_conformance::{
    ALL_BACKENDS, EXPERIMENTAL, FixtureData, Paths, SCHEMA, Status, available, python_ibis_agreement, run, run_tpch,
    targets, unported,
};

const USAGE: &str = "usage: ibis-conformance [--backend duckdb,sqlite] [ID_SUBSTRING]";

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let mut names: Option<Vec<String>> = None;
    let mut filter = String::new();
    while let Some(a) = args.next() {
        match a.as_str() {
            "-h" | "--help" => {
                eprintln!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            "--backend" => match args.next() {
                Some(v) => names = Some(v.split(',').map(String::from).collect()),
                None => {
                    eprintln!("{USAGE}");
                    return ExitCode::from(2);
                }
            },
            _ => filter = a,
        }
    }
    let strict = names.is_some();
    let names = names.unwrap_or_else(|| ALL_BACKENDS.iter().map(|s| s.to_string()).collect());
    match go(&names, strict, &filter) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(e) => {
            eprintln!("ibis-conformance: {e}");
            ExitCode::from(2)
        }
    }
}

fn go(names: &[String], strict: bool, filter: &str) -> ibis_core::Result<bool> {
    let paths = Paths::from_repo_root(&Paths::default_root());
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    let names = available(&names, strict)?;
    let data = FixtureData::read(&paths)?;
    let ts = targets(&data, &names)?;
    let mut outcomes = run(&paths, &data, &ts, filter)?;
    outcomes.extend(run_tpch(&paths, &names, filter)?);

    let mut stdout = std::io::stdout().lock();
    for o in &outcomes {
        // a closed pipe (| head) is not an error worth panicking over
        if writeln!(stdout, "{}", o.to_json()).is_err() {
            break;
        }
    }

    let mut by_id: BTreeMap<&str, BTreeMap<&str, Status>> = BTreeMap::new();
    let mut totals: BTreeMap<&str, BTreeMap<&str, usize>> = BTreeMap::new();
    for o in &outcomes {
        by_id.entry(&o.id).or_default().insert(&o.backend, o.status);
        *totals.entry(&o.backend).or_default().entry(o.status.as_str()).or_default() += 1;
    }
    let mut err = std::io::stderr().lock();
    let names: Vec<&str> = std::iter::once(SCHEMA).chain(names.iter().copied()).collect();
    let _ = writeln!(
        err,
        "{:<64}{}",
        "case (non-passing only)",
        names.iter().map(|n| format!("{n:>10}")).collect::<String>()
    );
    for (id, row) in &by_id {
        if row.values().all(|s| *s == Status::Pass) {
            continue;
        }
        let cells: String = names.iter().map(|n| format!("{:>10}", row.get(n).map_or("-", |s| s.as_str()))).collect();
        let _ = writeln!(err, "{id:<64}{cells}");
    }
    for (b, counts) in &totals {
        let pass = counts.get("pass").copied().unwrap_or(0);
        let total: usize = counts.values().sum();
        let _ = writeln!(err, "{b:<8} {pass}/{total} pass  {counts:?}");
    }
    // How Python Ibis itself does against its own DuckDB results, for scale.
    for (b, (same, total)) in python_ibis_agreement(&paths)? {
        let _ = writeln!(err, "python ibis on {b}: {same}/{total} cases match its duckdb result");
    }
    let missing = unported(&paths)?;
    if !missing.is_empty() && filter.is_empty() {
        let _ = writeln!(err, "{} cases have goldens but no Rust port: {}", missing.len(), missing.join(", "));
    }
    let gated = |o: &&ibis_conformance::Outcome| !EXPERIMENTAL.contains(&o.backend.as_str());
    Ok(!outcomes.iter().filter(gated).any(|o| o.status.is_problem()) && (missing.is_empty() || !filter.is_empty()))
}
