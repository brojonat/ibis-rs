#!/bin/bash
# Fetch the Ibis testing-data parquet files the conformance suite uses.
set -euo pipefail
out="$(cd "$(dirname "$0")/.." && pwd)/ci/ibis-testing-data/parquet"
mkdir -p "$out"
base="https://raw.githubusercontent.com/ibis-project/testing-data/master/parquet"
for t in functional_alltypes awards_players batting diamonds astronauts; do
  [ -s "$out/$t.parquet" ] || curl -fsSL "$base/$t.parquet" -o "$out/$t.parquet"
done
