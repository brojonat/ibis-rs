# /// script
# requires-python = ">=3.11"
# dependencies = ["duckdb==1.4.*", "psycopg[binary]"]
# ///
"""Generate TPC-H data with DuckDB's dbgen and load it into every benchmark DB.

Writes ci/tpch/sf<SF>/:
  <table>.parquet   source of truth
  tpch.duckdb       DuckDB database
  tpch.sqlite       SQLite database (DECIMAL stored as REAL)
and, if IBIS_TESTING_POSTGRES_URL is set, database tpch_sf<SF> on that server.
Each of the three is left alone if it is already there.

Usage: tpch_data.py [SF]   (default 0.01)
"""

from __future__ import annotations

import os
import sqlite3
import sys
from pathlib import Path

import duckdb
import psycopg

TABLES = ["customer", "lineitem", "nation", "orders", "part", "partsupp", "region", "supplier"]
ROOT = Path(__file__).resolve().parent.parent


def tag(sf: str) -> str:
    return "sf" + sf.replace(".", "_")


def main(sf: str) -> None:
    out = ROOT / "ci" / "tpch" / tag(sf)
    out.mkdir(parents=True, exist_ok=True)
    duck, sqlite = out / "tpch.duckdb", out / "tpch.sqlite"
    url, db = os.environ.get("IBIS_TESTING_POSTGRES_URL"), f"tpch_{tag(sf)}"
    postgres = bool(url) and not postgres_loaded(url, db)
    if duck.exists() and sqlite.exists() and not postgres:
        return

    con = duckdb.connect(str(duck))
    if not set(TABLES) <= {name for (name,) in con.execute("SHOW TABLES").fetchall()}:
        if (out / "lineitem.parquet").exists():
            for t in TABLES:
                con.execute(f"CREATE OR REPLACE TABLE {t} AS FROM '{out / t}.parquet'")
        else:
            print(f"dbgen sf={sf}", file=sys.stderr)
            con.install_extension("tpch")
            con.load_extension("tpch")
            con.execute(f"CALL dbgen(sf={float(sf)})")
            for t in TABLES:
                con.execute(f"COPY {t} TO '{out / t}.parquet' (FORMAT parquet)")
    if not sqlite.exists():
        tmp = sqlite.with_suffix(".tmp")
        tmp.unlink(missing_ok=True)
        load_sqlite(con, tmp)
        tmp.rename(sqlite)
    if postgres:
        print(f"load postgres {db}", file=sys.stderr)
        con.execute(f"ATTACH '{url} dbname={db}' AS pg (TYPE postgres)")
        for t in TABLES:
            con.execute(f"DROP TABLE IF EXISTS pg.{t}")
            con.execute(f"CREATE TABLE pg.{t} AS FROM {t}")
        con.execute("DETACH pg")


def postgres_loaded(url: str, db: str) -> bool:
    """Whether `db` has the TPC-H tables; creates the database if it is missing."""
    # CREATE DATABASE can't run in a transaction, hence autocommit.
    with psycopg.connect(f"{url} dbname=postgres", autocommit=True) as admin:
        if not admin.execute("SELECT 1 FROM pg_database WHERE datname = %s", [db]).fetchone():
            admin.execute(f"CREATE DATABASE {db}")
            return False
    with psycopg.connect(f"{url} dbname={db}") as pg:
        have = {name for (name,) in pg.execute("SELECT tablename FROM pg_tables WHERE schemaname = 'public'")}
    return set(TABLES) <= have


def load_sqlite(con: duckdb.DuckDBPyConnection, sqlite: Path) -> None:
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


if __name__ == "__main__":
    main(sys.argv[1] if len(sys.argv) > 1 else "0.01")
