# Changelog

All notable changes to this project are documented here. Format: [Keep a Changelog](https://keepachangelog.com/).

## [Unreleased]

### Added
- `ibis-core`: Ibis datatypes/schema with Arrow mapping; IR (`Rel`/`RelOp`, `Expr` with closed op enums documenting Ibis semantics); `Table`/`Expr` builder API with eager schema validation and type inference; `Backend` trait; Arrow <-> row conversions that conform results to the Ibis output schema.
- `ibis-sql`: IR -> SQL compiler (merge-or-wrap `Select` lowering) and the `Dialect` trait with per-op override hooks.
- `ibis-duckdb`: DuckDB backend (bundled, Arrow-native results).
- `ibis-sqlite`: SQLite backend with Rust polyfills (regex, padding, reverse, math, variance/stddev aggregates), LIKE -> GLOB for case-sensitive LIKE, declared-type round-tripping.
- Conformance suite: `conformance/cases.py` (108 ids ported from Ibis's test_string, test_numeric, test_aggregation), `gen_goldens.py` (goldens from Ibis 12.0.0 on DuckDB), `ibis-conformance` runner + `cargo test` gate. 108/108 pass on DuckDB and SQLite.
- `mise` tasks: setup, build, test, lint, conformance, goldens, dev.

- Spike `spikes/unparser`: DataFusion 55 Unparser conformance across Postgres 18, DuckDB, SQLite (62 cases; findings in `spikes/unparser/FINDINGS.md`).
- Design doc (`docs/design.md`): owned IR, backend plugins, Ibis-test-suite parity as the north star.
- Cargo workspace scaffold, `mise.toml` front door, `install-skills.sh`.
