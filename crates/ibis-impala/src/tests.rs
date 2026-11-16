use super::*;

#[test]
fn column_types() {
    assert_eq!(parse_type("bigint"), DataType::Int64);
    assert_eq!(parse_type("string"), DataType::String);
    assert_eq!(parse_type("varchar(10)"), DataType::String);
    assert_eq!(parse_type("decimal(15,2)"), DataType::Decimal { precision: 15, scale: 2 });
    assert_eq!(parse_type("timestamp"), DataType::Timestamp { tz: None });
}

#[test]
fn only_impala_urls_connect() {
    assert!(Impala::connect("hive2://localhost").is_err());
    assert!(Impala::connect("impala://localhost?tls=1").is_err());
}

#[test]
fn inserts_send_the_rows_as_one_string() {
    let schema = Schema::of(&[("s", DataType::String), ("n", DataType::Int16), ("b", DataType::Boolean)]);
    let rows =
        [vec![Value::from("it's"), Value::Null, Value::Bool(true)], vec![Value::Null, Value::Int(2), Value::Null]];
    let batch = rows_to_batch(&schema, &rows).unwrap();
    let statements = insert_statements(&ImpalaDialect, "`t`", &schema, &[batch]).unwrap();
    // \x01 between cells, \x02 between rows, \x03 for NULL
    let digits = "SELECT 0 AS d UNION ALL SELECT 1 UNION ALL SELECT 2 UNION ALL SELECT 3 UNION ALL SELECT 4 \
                  UNION ALL SELECT 5 UNION ALL SELECT 6 UNION ALL SELECT 7 UNION ALL SELECT 8 UNION ALL SELECT 9";
    assert_eq!(
        statements,
        [format!(
            "WITH d AS ({digits}) INSERT INTO `t` SELECT NULLIF(SPLIT_PART(r, '\x01', 1), '\x03'), \
             CAST(NULLIF(SPLIT_PART(r, '\x01', 2), '\x03') AS SMALLINT), (NULLIF(SPLIT_PART(r, '\x01', 3), '\x03') = 'true') \
             FROM (SELECT n.n, SPLIT_PART(v.rows_text, '\x02', n.n) AS r \
             FROM (SELECT 'it\\'s\x01\x03\x01true\x02\x03\x012\x01\x03' AS rows_text) v \
             CROSS JOIN (SELECT a.d * 1000 + b.d * 100 + c.d * 10 + e.d + 1 AS n \
             FROM d a CROSS JOIN d b CROSS JOIN d c CROSS JOIN d e) n WHERE n.n <= 2 GROUP BY 1, 2) t"
        )]
    );
}

#[test]
fn separators_avoid_what_the_data_holds() {
    let schema = Schema::of(&[("s", DataType::String)]);
    let batch = rows_to_batch(&schema, &[vec![Value::from("a\x01b\x03c")]]).unwrap();
    let statement = insert_statements(&ImpalaDialect, "`t`", &schema, &[batch]).unwrap().remove(0);
    // \x02 is the first free one: cells; then \x04 rows, \x05 NULL
    assert!(statement.contains("NULLIF(SPLIT_PART(r, '\x02', 1), '\x05')"), "{statement:?}");
    assert!(statement.contains("SPLIT_PART(v.rows_text, '\x04', n.n)"), "{statement:?}");
    let every: String = SEPARATORS.iter().collect();
    let batch = rows_to_batch(&schema, &[vec![Value::from(every.as_str())]]).unwrap();
    assert!(insert_statements(&ImpalaDialect, "`t`", &schema, &[batch]).is_err());
}
