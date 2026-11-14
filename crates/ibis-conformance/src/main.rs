//! ibis-conformance [--backend duckdb,sqlite] [ID_SUBSTRING]
//!
//! stdout: one JSON object per (case, backend).
//! stderr: a pass matrix of non-passing cases and per-backend totals.
//! Exit 1 if any outcome is a problem (fail, error, xpass, no_golden).

use std::collections::BTreeMap;
use std::io::Write;
use std::process::ExitCode;

use ibis_conformance::{ALL_BACKENDS, Paths, Status, backends, run};

const USAGE: &str = "usage: ibis-conformance [--backend duckdb,sqlite] [ID_SUBSTRING]";

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let mut names: Vec<String> = ALL_BACKENDS.iter().map(|s| s.to_string()).collect();
    let mut filter = String::new();
    while let Some(a) = args.next() {
        match a.as_str() {
            "-h" | "--help" => {
                eprintln!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            "--backend" => match args.next() {
                Some(v) => names = v.split(',').map(String::from).collect(),
                None => {
                    eprintln!("{USAGE}");
                    return ExitCode::from(2);
                }
            },
            _ => filter = a,
        }
    }
    match go(&names, &filter) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(e) => {
            eprintln!("ibis-conformance: {e}");
            ExitCode::from(2)
        }
    }
}

fn go(names: &[String], filter: &str) -> ibis_core::Result<bool> {
    let paths = Paths::from_repo_root(&Paths::default_root());
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    let bs = backends(&paths, &names)?;
    let outcomes = run(&paths, &bs, filter)?;

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
    let _ = writeln!(
        err,
        "{:<56}{}",
        "case (non-passing only)",
        names.iter().map(|n| format!("{n:>11}")).collect::<String>()
    );
    for (id, row) in &by_id {
        if row.values().all(|s| *s == Status::Pass) {
            continue;
        }
        let cells: String = names.iter().map(|n| format!("{:>11}", row.get(n).map_or("-", |s| s.as_str()))).collect();
        let _ = writeln!(err, "{id:<56}{cells}");
    }
    for (b, counts) in &totals {
        let pass = counts.get("pass").copied().unwrap_or(0);
        let total: usize = counts.values().sum();
        let _ = writeln!(err, "{b:<8} {pass}/{total} pass  {counts:?}");
    }
    Ok(!outcomes.iter().any(|o| o.status.is_problem()))
}
