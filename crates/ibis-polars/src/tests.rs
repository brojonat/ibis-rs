use arrow_array::types::ArrowPrimitiveType;
use ibis_core::convert::{batch_to_rows, rows_to_batch};
use ibis_core::prelude::*;
use ibis_core::{Bound, Error, Field, Frame, Result, Schema, Value};
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

/// `id`, an array `xs`, a second array `ys`, a struct `s` and a map `m`,
/// with a NULL in each nested column.
fn nested() -> Result<(Polars, Table)> {
    let t = |s: &str| s.parse::<DataType>();
    let schema = Schema::new(vec![
        Field::new("id", DataType::Int64),
        Field::new("xs", t("array<int64>")?),
        Field::new("ys", t("array<string>")?),
        Field::new("s", t("struct<a: int64, b: string>")?),
        Field::new("m", t("map<string, int64>")?),
        Field::new("k", DataType::String),
    ]);
    let ints = |v: &[i64]| Value::Array(v.iter().map(|i| Value::Int(*i)).collect());
    let strs = |v: &[&str]| Value::Array(v.iter().map(|s| Value::from(*s)).collect());
    let pair = |a: i64, b: &str| Value::Struct(vec![("a".into(), a.into()), ("b".into(), b.into())]);
    let map = |v: &[(&str, i64)]| Value::Map(v.iter().map(|(k, v)| (Value::from(*k), Value::Int(*v))).collect());
    let rows = vec![
        vec![1.into(), ints(&[3, 1, 2]), strs(&["a", "b"]), pair(1, "x"), map(&[("a", 1), ("b", 2)]), "b".into()],
        vec![2.into(), ints(&[]), strs(&["c"]), Value::Null, map(&[]), "a".into()],
        vec![3.into(), Value::Null, Value::Null, pair(3, "z"), Value::Null, "a".into()],
        vec![4.into(), ints(&[7]), strs(&["d", "e", "f"]), pair(4, "w"), map(&[("a", 9)]), Value::Null],
    ];
    let con = Polars::new();
    con.create_table("nested", &[rows_to_batch(&schema, &rows)?])?;
    let t = con.table("nested")?;
    Ok((con, t))
}

fn rows(con: &Polars, t: &Table) -> Result<Vec<Vec<Value>>> {
    let mut out = vec![];
    for batch in con.execute(t)? {
        out.extend(batch_to_rows(&batch)?);
    }
    Ok(out)
}

/// One column of a query, each value printed as Rust prints it.
fn column(con: &Polars, t: &Table) -> Result<Vec<String>> {
    Ok(rows(con, t)?.into_iter().map(|mut r| format!("{:?}", r.remove(0))).collect())
}

#[test]
fn nested_columns_are_stored_and_read_back() -> Result<()> {
    let (con, t) = nested()?;
    assert_eq!(
        t.schema().to_string(),
        "id: int64, xs: array<int64>, ys: array<string>, s: struct<a: int64, b: string>, m: map<string, int64>, k: string"
    );
    let got = rows(&con, &t)?;
    assert_eq!(got[0][1], Value::Array(vec![3.into(), 1.into(), 2.into()]));
    assert_eq!(got[0][3], Value::Struct(vec![("a".into(), 1.into()), ("b".into(), "x".into())]));
    assert_eq!(got[0][4], Value::Map(vec![("a".into(), 1.into()), ("b".into(), 2.into())]));
    assert_eq!(got[1][1], Value::Array(vec![]));
    assert_eq!(got[1][3], Value::Null);
    assert_eq!(got[1][4], Value::Map(vec![]));
    assert_eq!((&got[2][1], &got[2][2], &got[2][4]), (&Value::Null, &Value::Null, &Value::Null));
    Ok(())
}

#[test]
fn array_ops_polars_has_expressions_for() -> Result<()> {
    let (con, t) = nested()?;
    let q = t.select([
        col("xs").length().name("n"),
        col("xs").at(-1).name("last"),
        col("xs").sort().name("sorted"),
        col("xs").sums().name("total"),
        col("xs").contains(7).name("has7"),
        col("s").field("b").name("b"),
    ])?;
    let got = rows(&con, &q)?;
    assert_eq!(
        got[0],
        vec![3.into(), 2.into(), Value::Array(vec![1.into(), 2.into(), 3.into()]), 6.into(), false.into(), "x".into()]
    );
    // an empty array: no last element, nothing to sum
    assert_eq!(got[1], vec![0.into(), Value::Null, Value::Array(vec![]), Value::Null, false.into(), Value::Null]);
    assert!(got[2].iter().take(5).all(Value::is_null));
    Ok(())
}

#[test]
fn ops_without_a_polars_expression_run_row_by_row_on_columns() -> Result<()> {
    let (con, t) = nested()?;
    // every argument is a column, none a literal
    let zipped = column(&con, &t.select([col("xs").zip([col("ys")]).name("z")])?)?;
    assert_eq!(
        zipped[3],
        r#"Array([Struct([("f1", Int(7)), ("f2", String("d"))]), Struct([("f1", Null), ("f2", String("e"))]), Struct([("f1", Null), ("f2", String("f"))])])"#
    );
    assert_eq!(zipped[2], "Null");
    let looked_up = column(&con, &t.select([col("m").get(col("k")).name("v")])?)?;
    assert_eq!(looked_up, ["Int(2)", "Null", "Null", "Null"]);
    let has = column(&con, &t.select([col("m").contains(col("k")).name("v")])?)?;
    assert_eq!(has, ["Bool(true)", "Bool(false)", "Null", "Null"]);
    let position = column(&con, &t.select([col("xs").index(col("id") + 1).name("p")])?)?;
    assert_eq!(position, ["Int(2)", "Int(-1)", "Null", "Int(-1)"]);
    // no rows: nothing to compute, and no error
    let none = t.filter([col("id").gt(lit(100))])?.select([col("xs").zip([col("ys")]).name("z")])?;
    assert!(rows(&con, &none)?.is_empty());
    Ok(())
}

#[test]
fn lambdas_see_the_element_and_its_position() -> Result<()> {
    let (con, t) = nested()?;
    let q = t.select([
        col("xs").map(|x| x * 10).name("tens"),
        col("xs").filter(|x| x.gt(lit(1))).name("big"),
        col("xs").map_with_index(|x, i| x + i).name("shifted"),
    ])?;
    let got = rows(&con, &q)?;
    let ints = |v: &[i64]| Value::Array(v.iter().map(|i| Value::Int(*i)).collect());
    assert_eq!(got[0], vec![ints(&[30, 10, 20]), ints(&[3, 2]), ints(&[3, 2, 4])]);
    assert_eq!(got[1], vec![ints(&[]), ints(&[]), ints(&[])]);
    assert_eq!(got[2], vec![Value::Null, Value::Null, Value::Null]);
    // Polars evaluates the function inside the list, where no column is visible
    let captured = t.select([col("xs").map(|x| x + col("id")).name("y")])?;
    assert!(matches!(con.execute(&captured), Err(Error::Unsupported { .. })));
    Ok(())
}

#[test]
fn unnest_explodes_rows() -> Result<()> {
    let (con, t) = nested()?;
    let elements = column(&con, &t.select([col("xs").unnest().name("x")])?)?;
    assert_eq!(elements, ["Int(3)", "Int(1)", "Int(2)", "Int(7)"]);
    // inside an expression, and next to another column
    let q = t.select([col("id"), (col("xs").unnest() * 2).name("x")])?;
    assert_eq!(rows(&con, &q)?.last(), Some(&vec![4.into(), 14.into()]));
    let kept = t.select([col("id"), col("xs")])?.unnest("xs", Some("pos"), true)?;
    let got = rows(&con, &kept)?;
    assert_eq!(got.len(), 6);
    assert_eq!(got[1], vec![1.into(), 1.into(), 1.into()]);
    assert_eq!(got[3], vec![2.into(), Value::Null, Value::Null]);
    assert_eq!(got[4], vec![3.into(), Value::Null, Value::Null]);
    Ok(())
}

#[test]
fn collect_gathers_a_group_into_an_array() -> Result<()> {
    let (con, t) = nested()?;
    let grouped = t.group_by(["k"]).agg([col("id").collect().name("ids")])?.order_by([asc("k")])?;
    let ints = |v: &[i64]| Value::Array(v.iter().map(|i| Value::Int(*i)).collect());
    let got = rows(&con, &grouped)?;
    assert!(got.contains(&vec!["a".into(), ints(&[2, 3])]));
    assert!(got.contains(&vec!["b".into(), ints(&[1])]));
    let whole = rows(&con, &t.aggregate([col("id").collect().name("ids")])?)?;
    assert_eq!(whole, [vec![ints(&[1, 2, 3, 4])]]);
    // SQL: an aggregate of no rows is NULL
    let none = rows(&con, &t.filter([col("id").gt(lit(100))])?.aggregate([col("id").collect().name("ids")])?)?;
    assert_eq!(none, [vec![Value::Null]]);
    // in an order, each value once: a sort Polars must not optimize away
    let ordered = col("k").collect().distinct().order_by([col("k").desc()]);
    let strings = |v: &[&str]| Value::Array(v.iter().map(|s| Value::from(*s)).collect());
    assert_eq!(rows(&con, &t.aggregate([ordered.name("ks")])?)?, [vec![strings(&["b", "a"])]]);
    let ordered = col("id").collect().order_by([col("id").desc()]).include_null();
    assert_eq!(rows(&con, &t.aggregate([ordered.name("ids")])?)?, [vec![ints(&[4, 3, 2, 1])]]);
    let last = col("id").last().order_by([col("id").desc()]);
    assert_eq!(rows(&con, &t.aggregate([last.name("id"), nunique().name("n")])?)?, [vec![1.into(), 4.into()]]);
    Ok(())
}

#[test]
fn a_view_is_a_named_query() -> Result<()> {
    let (con, t) = orders()?;
    con.create_view("big", &t.filter([col("amount").gt(lit(100))])?, false)?;
    assert_eq!(con.list_tables()?, ["big", "orders"]);
    assert_eq!(rows(&con, &con.table("big")?)?.len(), 3);
    // a view reads its table when it is queried
    con.insert("orders", &con.execute(&t)?, false)?;
    assert_eq!(rows(&con, &con.table("big")?)?.len(), 6);
    assert!(con.create_view("big", &t, false).is_err());
    assert!(con.create_view("orders", &t, true).is_err());
    con.create_view("big", &t, true)?;
    assert_eq!(rows(&con, &con.table("big")?)?.len(), 8);
    con.drop_view("big", false)?;
    assert!(con.drop_view("big", false).is_err());
    con.drop_view("big", true)?;
    assert_eq!(con.list_tables()?, ["orders"]);
    Ok(())
}

#[test]
fn create_table_without_overwrite_keeps_a_table_that_is_there() -> Result<()> {
    let (con, t) = orders()?;
    let data = con.execute(&t)?;
    assert!(con.create_table_with("orders", t.schema(), &data, CreateOptions::default()).is_err());
    con.create_table_with("orders", t.schema(), &[], CreateOptions::REPLACE)?;
    assert!(rows(&con, &con.table("orders")?)?.is_empty());
    // a query that reads the table it replaces
    con.insert("orders", &data, false)?;
    con.create_table_from("orders", &t.filter([col("amount").gt(lit(100))])?, CreateOptions::REPLACE)?;
    assert_eq!(rows(&con, &con.table("orders")?)?.len(), 3);
    Ok(())
}

#[test]
fn sql_is_a_table_other_operations_run_on() -> Result<()> {
    let (con, t) = orders()?;
    con.create_view("big", &t.filter([col("amount").gt(lit(100))])?, false)?;
    let totals = con.sql("SELECT region, SUM(amount) AS total FROM orders GROUP BY region;")?;
    assert_eq!(totals.schema().to_string(), "region: string, total: int32");
    let top = totals.filter([col("total").gt(lit(200))])?.select([col("region")])?;
    assert_eq!(rows(&con, &top)?, [vec![Value::from("us")]]);
    // a view is a table to the query, and a query that does not parse is an error
    assert_eq!(rows(&con, &con.sql("SELECT amount FROM big")?)?.len(), 3);
    assert!(con.sql("SELECT nope FROM orders").is_err());
    assert!(con.sql("not sql").is_err());
    // an aliased expression is a table to the query run over it
    let eu = t.filter([col("region").eq(lit("eu"))])?.alias("eu");
    let n = con.sql_over(&eu, "SELECT COUNT(*) AS n FROM eu")?;
    assert_eq!(rows(&con, &n)?, rows(&con, &eu.aggregate([count().name("n")])?)?);
    assert!(con.sql("SELECT * FROM eu").is_err());
    Ok(())
}
