//! `cargo test` gate: every ported Ibis case passes on every backend (or is
//! marked notimpl and still fails). Needs `mise run setup` for the fixture data.

use ibis_conformance::{ALL_BACKENDS, Paths, backends, run};

#[test]
fn ibis_parity() {
    let paths = Paths::from_repo_root(&Paths::default_root());
    let bs = backends(&paths, &ALL_BACKENDS).expect("load fixtures (run `mise run setup`)");
    let problems: Vec<String> = run(&paths, &bs, "")
        .expect("run conformance")
        .iter()
        .filter(|o| o.status.is_problem())
        .map(|o| format!("{} [{}]: {} {}", o.id, o.backend, o.status.as_str(), o.detail))
        .collect();
    assert!(problems.is_empty(), "{} conformance problems:\n{}", problems.len(), problems.join("\n"));
}

#[test]
fn tpch_parity() {
    let paths = Paths::from_repo_root(&Paths::default_root());
    let problems: Vec<String> = ibis_conformance::run_tpch(&paths, &ALL_BACKENDS, "")
        .expect("run TPC-H (run `mise run setup`)")
        .iter()
        .filter(|o| o.status.is_problem())
        .map(|o| format!("{} [{}]: {} {}", o.id, o.backend, o.status.as_str(), o.detail))
        .collect();
    assert!(problems.is_empty(), "{} TPC-H problems:\n{}", problems.len(), problems.join("\n"));
}
