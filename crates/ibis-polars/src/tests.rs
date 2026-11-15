use arrow_array::types::ArrowPrimitiveType;
use ibis_core::prelude::*;
use ibis_core::{Bound, Error, Frame, Result};
use polars::prelude as pl;
use polars::prelude::NamedFrom;

use crate::Polars;

fn orders() -> Result<(Polars, Table)> {
    let df = pl::DataFrame::new(
        4,
        vec![
            pl::Series::new("region".into(), ["eu", "eu", "us", "us"]).into(),
            pl::Series::new("amount".into(), [50i32, 150, 200, 300]).into(),
        ],
    )
    .unwrap();
    let con = Polars::new();
    con.register("orders", df)?;
    let t = con.table("orders")?;
    Ok((con, t))
}

#[test]
fn registered_dataframe_has_the_ibis_schema() -> Result<()> {
    let (con, t) = orders()?;
    assert_eq!(t.schema().to_string(), "region: string, amount: int32");
    assert_eq!(con.list_tables()?, ["orders"]);
    Ok(())
}

#[test]
fn results_come_back_in_ibis_types() -> Result<()> {
    let (con, t) = orders()?;
    // Ibis: sum(int32) is int64 and int / int is float64, whatever Polars would infer
    let expr = t
        .group_by(["region"])
        .agg([col("amount").sum().name("total"), (col("amount").max() / 2).name("half")])?
        .order_by([desc("total")])?;
    assert_eq!(expr.schema().to_string(), "region: string, total: int64, half: float64");
    let batch = &con.execute(&expr)?[0];
    assert_eq!(batch.schema().field(1).data_type(), &arrow_array::types::Int64Type::DATA_TYPE);
    let total = batch.column(1).as_any().downcast_ref::<arrow_array::Int64Array>().unwrap();
    assert_eq!(total.values(), &[500, 200]);
    let half = batch.column(2).as_any().downcast_ref::<arrow_array::Float64Array>().unwrap();
    assert_eq!(half.values(), &[150.0, 75.0]);
    Ok(())
}

#[test]
fn lazy_hands_back_a_polars_query() -> Result<()> {
    let (con, t) = orders()?;
    let lf = con.lazy(&t.filter([col("amount").gt(lit(100))])?)?;
    assert_eq!(lf.collect().unwrap().height(), 3);
    Ok(())
}

#[test]
fn an_op_polars_cannot_run_is_reported_as_unsupported() -> Result<()> {
    let (con, t) = orders()?;
    // a RANGE frame, which the backend does not translate
    let mut w = WindowSpec::new().order_by([asc("amount")]);
    w.frame = Some(Frame { rows: false, start: Bound::Preceding(1), end: Bound::CurrentRow });
    let expr = t.select([col("amount").sum().over(w).name("s")])?;
    match con.execute(&expr) {
        Err(Error::Unsupported { backend, .. }) => assert_eq!(backend, "polars"),
        other => panic!("expected Unsupported, got {other:?}"),
    }
    Ok(())
}
