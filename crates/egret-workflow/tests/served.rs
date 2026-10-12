//! `runtime::served()` without an install, then the install (no-silent-stall
//! spec §4.1). ONE test in its own binary: the set is process-wide, so the
//! order matters and no other test may install one first.

#[test]
fn served_panics_until_installed_and_the_first_install_stays() {
    // No permissive default: before an install, `served()` panics and
    // names the function that installs one.
    let panic = std::panic::catch_unwind(egret_workflow::runtime::served)
        .expect_err("no Served is installed yet");
    let msg = panic
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| panic.downcast_ref::<&str>().map(|s| s.to_string()))
        .expect("a message");
    assert!(msg.contains("install_served"), "{msg}");

    let none = egret_workflow::runtime::served_for(&egret_config::Config::default().workflow);
    egret_workflow::runtime::install_served(none.clone()).expect("the first install");
    assert_eq!(egret_workflow::runtime::served(), &none);

    let other = egret_workflow::runtime::Served::from_type_ids(["egret.sql"]);
    assert_eq!(
        egret_workflow::runtime::install_served(other.clone()),
        Err(other.clone()),
        "a second install is handed back"
    );
    assert_eq!(egret_workflow::runtime::served(), &none, "the first stays");

    // The test helper: idempotent for the installed set, loud for another.
    assert_eq!(
        egret_workflow::runtime::install_served_for_tests(none.clone()),
        &none
    );
    let panic = std::panic::catch_unwind(|| {
        egret_workflow::runtime::install_served_for_tests(other.clone())
    })
    .expect_err("a binary serves one set");
    let msg = panic.downcast_ref::<String>().cloned().unwrap_or_default();
    assert!(msg.contains("one set per binary"), "{msg}");
}
