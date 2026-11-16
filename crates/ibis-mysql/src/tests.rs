use ibis_sql::Dialect;

use super::*;

#[test]
fn catalog_types() {
    assert_eq!(parse_type("tinyint", "tinyint(1)", Some(3), Some(0)), DataType::Boolean);
    assert_eq!(parse_type("tinyint", "tinyint", Some(3), Some(0)), DataType::Int8);
    assert_eq!(parse_type("int", "int unsigned", Some(10), Some(0)), DataType::UInt32);
    assert_eq!(
        parse_type("decimal", "decimal(15,2)", Some(15), Some(2)),
        DataType::Decimal { precision: 15, scale: 2 }
    );
    assert_eq!(parse_type("datetime", "datetime(6)", None, None), DataType::Timestamp { tz: None });
    assert_eq!(parse_type("text", "text", None, None), DataType::String);
}

#[test]
fn cells_take_the_column_type() {
    let text = |s: &str| mysql::Value::Bytes(s.as_bytes().to_vec());
    assert_eq!(cell(mysql::Value::NULL, ColumnType::MYSQL_TYPE_LONG), Value::Null);
    assert_eq!(cell(text("42"), ColumnType::MYSQL_TYPE_LONGLONG), Value::Int(42));
    assert_eq!(cell(text("1.5"), ColumnType::MYSQL_TYPE_DOUBLE), Value::Float(1.5));
    assert_eq!(cell(text("2010-01-02"), ColumnType::MYSQL_TYPE_DATE), Value::Date(14611));
    // exact decimals stay text for the output conversion
    assert_eq!(cell(text("12.50"), ColumnType::MYSQL_TYPE_NEWDECIMAL), Value::from("12.50"));
    // too big for i64
    assert_eq!(
        cell(text("18446744073709551615"), ColumnType::MYSQL_TYPE_LONGLONG),
        Value::from("18446744073709551615")
    );
}

#[test]
fn literals() {
    assert_eq!(quote_str("it's a\\b"), "'it''s a\\\\b'");
    let mut out = String::new();
    insert_literal(&Value::Float(1.5), &mut out).unwrap();
    assert_eq!(out, "1.5e0");
    assert!(insert_literal(&Value::Float(f64::NAN), &mut out).is_err());
}

#[test]
fn null_placement_is_spelled_as_a_key() {
    let d = MySqlDialect;
    // NULL is the smallest value: these two are the native order
    assert_eq!(d.sort_key("`x`", true, true), "`x` ASC");
    assert_eq!(d.sort_key("`x`", false, false), "`x` DESC");
    assert_eq!(d.sort_key("`x`", true, false), "`x` IS NULL ASC, `x` ASC");
    assert_eq!(d.sort_key("`x`", false, true), "`x` IS NULL DESC, `x` DESC");
}
