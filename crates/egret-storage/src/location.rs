//! Warehouse layout: <warehouse>/<ns-levels...>/<table>-<uuid>/(metadata|data)/...

/// Base location for a new table, e.g. `s3://wh/sales/orders-<uuid>`.
pub fn table_base_location(
    warehouse: &str,
    namespace: &[String],
    table: &str,
    table_uuid: &uuid::Uuid,
) -> String {
    let wh = warehouse.trim_end_matches('/');
    let ns = namespace.join("/");
    if ns.is_empty() {
        format!("{wh}/{table}-{table_uuid}")
    } else {
        format!("{wh}/{ns}/{table}-{table_uuid}")
    }
}

/// Metadata file location for version N, e.g. `<base>/metadata/00003-<uuid>.metadata.json`.
pub fn table_metadata_location(
    base_location: &str,
    version: i32,
    file_uuid: &uuid::Uuid,
) -> String {
    let base = base_location.trim_end_matches('/');
    format!("{base}/metadata/{version:05}-{file_uuid}.metadata.json")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn base_location_joins_namespace_levels() {
        let id = uuid::Uuid::nil();
        assert_eq!(
            table_base_location("s3://wh/", &["sales".into()], "orders", &id),
            "s3://wh/sales/orders-00000000-0000-0000-0000-000000000000"
        );
    }
    #[test]
    fn metadata_location_zero_pads_version() {
        let id = uuid::Uuid::nil();
        let m = table_metadata_location("s3://wh/sales/orders-x", 3, &id);
        assert!(
            m.ends_with("/metadata/00003-00000000-0000-0000-0000-000000000000.metadata.json"),
            "{m}"
        );
    }
}
