#[test]
fn copy_name_increments() {
    use egret_bi::chart::next_copy_name;
    let mut have = vec!["Sales".to_string()];
    assert_eq!(next_copy_name(&have, "Sales"), "Sales (copy)");
    have.push("Sales (copy)".to_string());
    assert_eq!(next_copy_name(&have, "Sales"), "Sales (copy 2)");
    have.push("Sales (copy 2)".to_string());
    assert_eq!(next_copy_name(&have, "Sales"), "Sales (copy 3)");
}
