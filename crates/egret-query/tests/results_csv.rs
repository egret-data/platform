//! Unit test for the JSONL→CSV conversion helper.
use egret_query::rows_to_csv;

#[test]
fn csv_escapes_and_headers() {
    let columns = serde_json::json!([{"name":"n","type":"bigint"},{"name":"s","type":"varchar"}]);
    let rows = vec![
        serde_json::json!([1, "a,b"]),
        serde_json::json!([2, "x\"y"]),
    ];
    let csv = rows_to_csv(&columns, &rows);
    assert_eq!(csv, "n,s\n1,\"a,b\"\n2,\"x\"\"y\"\n");
}

#[test]
fn csv_handles_nulls_and_no_columns() {
    let rows = vec![serde_json::json!([serde_json::Value::Null, "plain"])];
    let csv = rows_to_csv(&serde_json::json!([]), &rows);
    assert_eq!(csv, ",plain\n"); // no header row when columns empty; null → empty field
}
