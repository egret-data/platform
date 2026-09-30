use egret_workspace::features::{gated_feature, FEATURES, FEATURE_KEYS};

#[test]
fn apps_is_a_feature_and_gates_its_prefix() {
    assert!(FEATURE_KEYS.contains(&"apps"));
    assert!(FEATURES.iter().any(|f| f.key == "apps"));
    assert_eq!(gated_feature("apps/123", "GET"), Some("apps"));
}
