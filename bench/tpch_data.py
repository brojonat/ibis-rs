# /// script
# requires-python = ">=3.11"
# dependencies = ["duckdb==1.4.*"]
# ///
"""Generate TPC-H data with DuckDB's dbgen and load it into every benchmark DB.

Writes ci/tpch/sf<SF>/:
  <table>.parquet   source of truth
  tpch.duckdb       DuckDB database
  tpch.sqlite       SQLite database (DECIMAL stored as REAL)
and, if IBIS_TESTING_POSTGRES_URL is set, database tpch_sf<SF> on that server.

Usage: tpch_data.py [SF]   (default 0.01)
"""

from __future__ import annotations

import os
import sqlite3
import subprocess
import sys
from pathlib import Path

import duckdb

TABLES = ["customer", "lineitem", "nation", "orders", "part", "partsupp", "region", "supplier"]
ROOT = Path(__file__).resolve().parent.parent


def tag(sf: str) -> str:
    return "sf" + sf.replace(".", "_")


def main(sf: str) -> None:
    out = ROOT / "ci" / "tpch" / tag(sf)
    out.mkdir(parents=True, exist_ok=True)
    con = duckdb.connect(str(out / "tpch.duckdb"))
    con.install_extension("tpch")
    con.load_extension("tpch")
    if not (out / "lineitem.parquet").exists():
        print(f"dbgen sf={sf}", file=sys.stderr)
        con.execute(f"CALL dbgen(sf={float(sf)})")
        for t in TABLES:
            con.execute(f"COPY {t} TO '{out / t}.parquet' (FORMAT parquet)")
    else:
        for t in TABLES:
            con.execute(f"CREATE OR REPLACE TABLE {t} AS FROM '{out / t}.parquet'")

    sqlite = out / "tpch.sqlite"
    sqlite.unlink(missing_ok=True)
    print("load sqlite", file=sys.stderr)
    # Declared types matter: they're how SQLite clients (Ibis, ibis-rs) recover
    # DATE etc. SQLite has no DECIMAL; store as DOUBLE, dates as ISO text.
    lite = sqlite3.connect(sqlite)
    for t in TABLES:
        cols = con.execute(f"DESCRIBE {t}").fetchall()
        decl = {"VARCHAR": "TEXT"}
        ddl = ", ".join(
            f'"{c}" {"DOUBLE" if ty.startswith("DECIMAL") else decl.get(ty, ty)}' for c, ty, *_ in cols
        )
        lite.execute(f'CREATE TABLE "{t}" ({ddl})')
        sel = ", ".join(
            f'CAST("{c}" AS DOUBLE)' if ty.startswith("DECIMAL")
            else f'CAST("{c}" AS VARCHAR)' if ty == "DATE"
            else f'"{c}"'
            for c, ty, *_ in cols
        )
        cur = con.execute(f"SELECT {sel} FROM {t}")
        marks = ", ".join("?" * len(cols))
        while rows := cur.fetchmany(100_000):
            lite.executemany(f'INSERT INTO "{t}" VALUES ({marks})', rows)
    lite.commit()
    lite.close()

    url = os.environ.get("IBIS_TESTING_POSTGRES_URL")
    if url:
        db = f"tpch_{tag(sf)}"
        print(f"load postgres {db}", file=sys.stderr)
        # CREATE DATABASE can't run in the transaction DuckDB's postgres_execute opens.
        admin = f"{url} dbname=postgres"
        exists = subprocess.run(
            ["psql", admin, "-qtAc", f"SELECT 1 FROM pg_database WHERE datname = '{db}'"],
            check=True, capture_output=True, text=True,
        ).stdout.strip()
        if not exists:
            subprocess.run(["psql", admin, "-qc", f"CREATE DATABASE {db}"], check=True)
        con.execute(f"ATTACH '{url} dbname={db}' AS pg (TYPE postgres)")
        for t in TABLES:
            con.execute(f"DROP TABLE IF EXISTS pg.{t}")
            con.execute(f"CREATE TABLE pg.{t} AS FROM {t}")
        con.execute("DETACH pg")


if __name__ == "__main__":
    main(sys.argv[1] if len(sys.argv) > 1 else "0.01")
