// What `golden/fixtures.rs` needs from the test binary that includes it, as
// `crate::…` (governance SP2b Task 6). A binary that builds state with the
// golden builders but never runs a golden scenario includes this at its crate
// root:
//
//     include!("opa_support/fixtures_host.rs");
//     #[path = "golden/fixtures.rs"]
//     mod fixtures;
//
// `Names` is copied from `tests/golden_compile.rs`; `capture` and
// `compare_or_bless` keep that file's signatures and are never called here.

/// Ids the fixture created, by role name; anything else in the output fails.
#[derive(Default)]
pub struct Names(pub std::collections::BTreeMap<String, String>);

impl Names {
    pub fn id(&mut self, role: &str, id: uuid::Uuid) -> uuid::Uuid {
        self.0.insert(id.to_string(), format!("{{{role}}}"));
        id
    }
    pub fn text(&mut self, role: &str, value: &str) {
        self.0.insert(value.to_string(), format!("{{{role}}}"));
    }
}

#[allow(dead_code)]
pub async fn capture(
    _tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    _profiles: &[(i64, Option<i64>)],
) -> serde_json::Value {
    unreachable!("golden scenarios are not run from here")
}

#[allow(dead_code)]
pub fn compare_or_bless(_: &str, _: serde_json::Value, _: &Names) {
    unreachable!("golden scenarios are not run from here")
}
