//! Rebuild this crate whenever `migrations/` changes.
//!
//! `sqlx::migrate!` embeds the directory **at compile time**, and cargo has no
//! idea that macro read anything outside the crate. So adding a migration file
//! does not invalidate this crate, the old set stays baked into the binary, and
//! the migration silently never runs.
//!
//! That is not hypothetical. Package 6 added `0131_dataapi_version_breaking_changes.sql`
//! without touching this crate; the deployed binary carried migrations through 130,
//! and every Data API Versions request answered
//! `500 — column v.compared_against does not exist`. The repo's previous remedy was
//! "remember to touch `src/lib.rs` when you add a migration", which is exactly the
//! kind of thing nobody remembers on the one commit where it matters.
//!
//! `rerun-if-changed` on a *directory* covers files added, removed and modified.
fn main() {
    println!("cargo:rerun-if-changed=../../migrations");
}
