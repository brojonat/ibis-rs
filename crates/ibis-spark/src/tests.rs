use super::dialect::java_regex;
use super::*;

#[test]
fn column_types() {
    assert_eq!(parse_type("bigint"), DataType::Int64);
    assert_eq!(parse_type("string"), DataType::String);
    assert_eq!(parse_type("varchar(10)"), DataType::String);
    assert_eq!(parse_type("decimal(15,2)"), DataType::Decimal { precision: 15, scale: 2 });
    assert_eq!(parse_type("timestamp_ntz"), DataType::Timestamp { tz: None });
    assert_eq!(parse_type("timestamp"), DataType::Timestamp { tz: Some("UTC".into()) });
}

#[test]
fn string_literals_escape_with_backslashes() {
    assert_eq!(quote_str("it's"), r"'it\'s'");
    assert_eq!(quote_str(r"\d+"), r"'\\d+'");
}

#[test]
fn only_spark_urls_connect() {
    assert!(Spark::connect("hive2://localhost").is_err());
    assert!(Spark::connect("spark://localhost?tls=1").is_err());
}

#[test]
fn posix_classes_become_java_ones() {
    assert_eq!(java_regex("[[:digit:]]+"), r"[\p{Digit}]+");
    assert_eq!(java_regex("[^[:alpha:][:space:]_]"), r"[^\p{Alpha}\p{Space}_]");
    assert_eq!(java_regex(r"\d+"), r"\d+");
}
