use super::*;

#[test]
fn urls() {
    let u = parse_url("clickhouse://ibis:secret@localhost:8123/analytics").unwrap();
    assert_eq!(
        (u.scheme, u.user, u.password, u.host, u.database),
        ("http", "ibis", "secret", "localhost:8123", "analytics")
    );
    let u = parse_url("clickhouses://example.com").unwrap();
    assert_eq!((u.scheme, u.user, u.password, u.host, u.database), ("https", "", "", "example.com", ""));
    assert!(parse_url("postgres://localhost").is_err());
    assert!(parse_url("clickhouse://").is_err());
}

#[test]
fn tab_separated_round_trip() {
    let mut out = String::new();
    tsv_field(&Value::from("a\tb\\c\nd"), &mut out);
    assert_eq!(out, "a\\tb\\\\c\\nd");
    assert_eq!(unescape(&out), "a\tb\\c\nd");
    assert_eq!(cell("\\N", &DataType::String), Value::Null);
    // the text "\N" itself arrives escaped
    assert_eq!(cell("\\\\N", &DataType::String), Value::from("\\N"));
}

#[test]
fn column_types() {
    assert_eq!(parse_type("Nullable(Int32)"), DataType::Int32);
    assert_eq!(parse_type("LowCardinality(Nullable(String))"), DataType::String);
    assert_eq!(parse_type("Nullable(Decimal(15, 2))"), DataType::Decimal { precision: 15, scale: 2 });
    assert_eq!(parse_type("DateTime64(6, 'UTC')"), DataType::Timestamp { tz: Some("UTC".into()) });
    assert_eq!(parse_type("Nullable(DateTime64(6))"), DataType::Timestamp { tz: None });
    assert_eq!(parse_type("Date32"), DataType::Date);
}

#[test]
fn cells_take_the_column_type() {
    assert_eq!(cell("42", &DataType::Int64), Value::Int(42));
    assert!(matches!(cell("nan", &DataType::Float64), Value::Float(f) if f.is_nan()));
    assert_eq!(cell("true", &DataType::Boolean), Value::Bool(true));
    assert_eq!(cell("2010-01-02", &DataType::Date), Value::Date(14611));
    // too big for i64: left as text for the output conversion
    assert_eq!(cell("18446744073709551615", &DataType::UInt64), Value::from("18446744073709551615"));
}
