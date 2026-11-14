//! The README example, compiled so it can't drift. Run from the repo root
//! after `mise run setup`:  cargo run -p ibis-duckdb --example readme

use ibis_core::prelude::*;

fn main() -> ibis_core::Result<()> {
    let con = ibis_duckdb::DuckDb::open_in_memory()?;
    con.raw_sql("CREATE TABLE functional_alltypes AS FROM 'ci/ibis-testing-data/parquet/functional_alltypes.parquet'")?;
    let t = con.table("functional_alltypes")?;

    let expr = t
        .filter([col("int_col").gt(lit(2))])?
        .group_by(["string_col"])
        .agg([col("double_col").mean().name("avg"), count().name("n")])?
        .order_by([desc("avg")])?
        .limit(10, 0);

    println!("{}", con.compile(&expr)?); // the SQL, like ibis.to_sql()
    let batches = con.execute(&expr)?; // Vec<arrow RecordBatch>
    println!("{} rows", batches.iter().map(|b| b.num_rows()).sum::<usize>());
    Ok(())
}
