use super::thrift::{self, I32, STRING, STRUCT, string};
use super::*;

#[test]
fn a_call_is_a_header_and_an_argument_struct() {
    let message = thrift::call("Ping", 7, T::Struct(vec![(1, T::I32(5))]));
    let expected: &[u8] = &[
        0x80, 0x01, 0x00, 0x01, // version 1, CALL
        0, 0, 0, 4, b'P', b'i', b'n', b'g', // method
        0, 0, 0, 7, // sequence number
        STRUCT, 0, 1, // field 1: the request
        I32, 0, 1, 0, 0, 0, 5, // its field 1
        0, // end of the request
        0, // end of the arguments
    ];
    assert_eq!(message, expected);
}

#[test]
fn values_read_back_as_written() {
    let value = T::Struct(vec![
        (1, T::Bool(true)),
        (2, T::Double(1.5)),
        (3, T::I64(-2)),
        (4, string("héllo")),
        (5, T::Map(STRING, I32, vec![(string("k"), T::I32(1))])),
        (6, T::List(STRUCT, vec![T::Struct(vec![(1, T::I16(3)), (2, T::Byte(-1))])])),
    ]);
    let mut bytes = vec![];
    value.write(&mut bytes);
    assert_eq!(T::read(&mut bytes.as_slice(), STRUCT).unwrap(), value);
}

/// The reply to a call: `body` as the result struct's field 0, or as an exception.
fn message(kind: u8, body: T) -> Vec<u8> {
    let mut bytes = vec![0x80, 0x01, 0x00, kind];
    string("Ping").write(&mut bytes);
    bytes.extend([0, 0, 0, 1]);
    body.write(&mut bytes);
    bytes
}

#[test]
fn a_reply_is_its_success_field() {
    let bytes = message(2, T::Struct(vec![(0, T::Struct(vec![(1, T::I32(9))]))]));
    assert_eq!(thrift::reply(&mut bytes.as_slice()).unwrap(), T::Struct(vec![(1, T::I32(9))]));
}

#[test]
fn an_exception_is_an_error_with_its_message() {
    let bytes = message(3, T::Struct(vec![(1, string("no such method")), (2, T::I32(1))]));
    let error = thrift::reply(&mut bytes.as_slice()).unwrap_err();
    assert_eq!(error.to_string(), "Ping: no such method");
}

#[test]
fn something_else_on_the_wire_is_an_error() {
    assert!(thrift::reply(&mut b"HTTP/1.1 400 Bad Request\r\n".as_slice()).is_err());
    // a length that cannot be right
    assert!(T::read(&mut [0x7f, 0xff, 0xff, 0xff].as_slice(), STRING).is_err());
}

#[test]
fn statuses() {
    let status = |code, message: &str| T::Struct(vec![(1, T::Struct(vec![(1, T::I32(code)), (5, string(message))]))]);
    assert!(check(&status(0, "")).is_ok());
    assert!(check(&status(1, "a warning")).is_ok());
    assert_eq!(check(&status(3, "table not found")).unwrap_err().to_string(), "table not found");
}

#[test]
fn column_types_are_spelled_like_describe() {
    let desc = |id, qualifiers: Vec<(&str, i32)>| {
        let qualifiers =
            qualifiers.into_iter().map(|(k, v)| (string(k), T::Struct(vec![(1, T::I32(v))]))).collect::<Vec<_>>();
        let primitive = T::Struct(vec![(1, T::I32(id)), (2, T::Struct(vec![(1, T::Map(STRING, STRUCT, qualifiers))]))]);
        T::Struct(vec![
            (1, string("c")),
            (2, T::Struct(vec![(1, T::List(STRUCT, vec![T::Struct(vec![(1, primitive)])]))])),
        ])
    };
    assert_eq!(column_desc(&desc(4, vec![])), ("c".to_string(), "bigint".to_string()));
    assert_eq!(column_desc(&desc(15, vec![("precision", 15), ("scale", 2)])).1, "decimal(15,2)");
    assert_eq!(column_desc(&desc(18, vec![("characterMaximumLength", 10)])).1, "varchar(10)");
    assert_eq!(column_desc(&desc(8, vec![])).1, "timestamp");
}

#[test]
fn columns_carry_a_null_bitmap() {
    // i64Val: three values, the second NULL
    let column = T::Struct(vec![(
        5,
        T::Struct(vec![(1, T::List(thrift::I64, vec![T::I64(1), T::I64(0), T::I64(3)])), (2, T::Binary(vec![0b010]))]),
    )]);
    assert_eq!(column_values(column).unwrap(), Values::Int(vec![Some(1), None, Some(3)]));
    let column = T::Struct(vec![(
        7,
        T::Struct(vec![(1, T::List(STRING, vec![string("a"), string("")])), (2, T::Binary(vec![]))]),
    )]);
    assert_eq!(column_values(column).unwrap(), Values::String(vec![Some("a".into()), Some("".into())]));
    // ten rows need two bytes
    assert!(is_null(&[0, 0b10], 9));
    assert!(!is_null(&[0, 0b10], 8));
}

#[test]
fn urls() {
    let o = Options::from_url("spark://me:secret@example.com:10001/sales", 10000, true).unwrap();
    assert_eq!(
        (o.user.as_str(), o.password.as_str(), o.host.as_str(), o.port, o.database.as_str(), o.sasl),
        ("me", "secret", "example.com", 10001, "sales", true)
    );
    let o = Options::from_url("impala://localhost", 21050, false).unwrap();
    assert_eq!(
        (o.user.as_str(), o.host.as_str(), o.port, o.database.as_str(), o.sasl),
        ("", "localhost", 21050, "", false)
    );
    assert!(Options::from_url("impala://localhost/db?auth=plain", 21050, false).unwrap().sasl);
    assert!(!Options::from_url("spark://localhost?auth=nosasl", 10000, true).unwrap().sasl);
    assert!(Options::from_url("spark://localhost?tls=yes", 10000, true).is_err());
    assert!(Options::from_url("spark://localhost:port", 10000, true).is_err());
    assert!(Options::from_url("localhost", 10000, true).is_err());
}
