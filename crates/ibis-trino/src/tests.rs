use serde_json::json;

use super::*;

#[test]
fn urls() {
    let u = parse_url("trino://ibis@localhost:8080/memory/default").unwrap();
    assert_eq!(
        (u.scheme, u.user, u.password, u.host, u.catalog, u.schema),
        ("http", "ibis", "", "localhost:8080", "memory", "default")
    );
    let u = parse_url("trinos://ibis:secret@example.com/hive").unwrap();
    assert_eq!(
        (u.scheme, u.user, u.password, u.host, u.catalog, u.schema),
        ("https", "ibis", "secret", "example.com", "hive", "")
    );
    // a catalog is required
    assert!(parse_url("trino://localhost:8080").is_err());
    assert!(parse_url("postgres://localhost/db").is_err());
}

#[test]
fn basic_auth_encoding() {
    assert_eq!(base64(b"ibis:secret"), "aWJpczpzZWNyZXQ=");
    assert_eq!(base64(b"ab"), "YWI=");
    assert_eq!(base64(b"abc"), "YWJj");
}

#[test]
fn column_types() {
    assert_eq!(parse_type("bigint"), DataType::Int64);
    assert_eq!(parse_type("varchar(10)"), DataType::String);
    assert_eq!(parse_type("decimal(15, 2)"), DataType::Decimal { precision: 15, scale: 2 });
    assert_eq!(parse_type("decimal(15,2)"), DataType::Decimal { precision: 15, scale: 2 });
    assert_eq!(parse_type("timestamp(6)"), DataType::Timestamp { tz: None });
    assert_eq!(parse_type("timestamp(3) with time zone"), DataType::Timestamp { tz: Some("UTC".into()) });
}

#[test]
fn cells_take_the_column_type() {
    assert_eq!(cell(&json!(null), &DataType::Int64), Value::Null);
    assert_eq!(cell(&json!(42), &DataType::Int64), Value::Int(42));
    assert_eq!(cell(&json!(1.5), &DataType::Float64), Value::Float(1.5));
    assert!(matches!(cell(&json!("NaN"), &DataType::Float64), Value::Float(f) if f.is_nan()));
    assert_eq!(cell(&json!("2010-01-02"), &DataType::Date), Value::Date(14611));
    let ts = DataType::Timestamp { tz: Some("UTC".into()) };
    assert_eq!(cell(&json!("1970-01-01 00:00:01.500000 UTC"), &ts), Value::Timestamp(1_500_000));
    // exact decimals stay text for the output conversion
    assert_eq!(cell(&json!("12.50"), &DataType::Decimal { precision: 15, scale: 2 }), Value::from("12.50"));
}

#[test]
fn floats_are_double_literals() {
    use dialect::double_literal;
    assert_eq!(double_literal(1.5), "1.5E0");
    assert_eq!(double_literal(-0.001), "-1E-3");
    assert_eq!(double_literal(f64::NAN), "nan()");
    assert_eq!(double_literal(f64::NEG_INFINITY), "-infinity()");
}

#[test]
fn inserts_send_each_column_as_a_json_array() {
    let schema = Schema::of(&[("playerID", DataType::String), ("G", DataType::Int16)]);
    let rows = [vec![Value::from("a'b\"c"), Value::Int(1)], vec![Value::Null, Value::Null]];
    let batch = rows_to_batch(&schema, &rows).unwrap();
    let statements = insert_statements(&TrinoDialect, "\"t\"", &schema, &[batch]).unwrap();
    assert_eq!(
        statements,
        [concat!(
            r#"INSERT INTO "t" SELECT CAST(c0 AS VARCHAR), CAST(c1 AS SMALLINT) FROM UNNEST("#,
            r#"CAST(JSON_PARSE('["a''b\"c",null]') AS ARRAY(VARCHAR)), "#,
            r#"CAST(JSON_PARSE('["1",null]') AS ARRAY(VARCHAR))) AS v (c0, c1)"#
        )]
    );
}

#[test]
fn inserted_text_is_what_a_cast_reads() {
    let decimal = DataType::Decimal { precision: 15, scale: 2 };
    assert_eq!(insert_text(&Value::Float(0.000001), &decimal).unwrap(), "0.00");
    assert_eq!(insert_text(&Value::Float(12.5), &decimal).unwrap(), "12.50");
    assert_eq!(insert_text(&Value::Float(f64::INFINITY), &DataType::Float64).unwrap(), "Infinity");
    assert_eq!(insert_text(&Value::Binary(vec![0, 255]), &DataType::Binary).unwrap(), "00ff");
    assert_eq!(insert_text(&Value::Null, &DataType::Int64), None);
}

#[test]
fn nested_column_types() {
    let t = |name: &str| parse_type(name).to_string();
    assert_eq!(t("array(bigint)"), "array<int64>");
    assert_eq!(t("map(varchar, bigint)"), "map<string, int64>");
    assert_eq!(
        t("row(a double, b varchar, c array(decimal(10, 2)))"),
        "struct<a: float64, b: string, c: array<decimal(10, 2)>>"
    );
    assert_eq!(t("row(\"left join\" integer)"), "struct<left join: int32>");
    // unnamed fields get Ibis's names
    assert_eq!(t("row(bigint, varchar)"), "struct<f1: int64, f2: string>");
}

#[test]
fn nested_types_are_named_for_ddl() {
    let t = DataType::structure(&[("a", DataType::array(DataType::Int64))]);
    assert_eq!(TrinoDialect.type_name(&t).unwrap(), "ROW(\"a\" ARRAY(BIGINT))");
    let m = DataType::map(DataType::String, DataType::Float64);
    assert_eq!(TrinoDialect.type_name(&m).unwrap(), "MAP(VARCHAR, DOUBLE)");
}
