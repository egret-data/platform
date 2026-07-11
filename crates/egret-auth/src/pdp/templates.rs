//! One Cedar policy template per role, body = the role's permitted actions.
//! Regenerated whenever `role_permissions` changes.

/// Build a Cedar template string for a role.
/// The *platform-scoped* `platform_admin` role gets an unrestricted body; every
/// other role (including a workspace role that merely happens to be *named*
/// `platform_admin`) lists its actions.
///
/// The template's PolicyId is set explicitly by the caller via
/// `Template::parse(Some(id), ..)` (keyed by role_id), so this function emits no
/// `@id(..)` annotation — Cedar's `@id` is not honored as the PolicyId and a
/// name-based one would be misleading now that ids are UUID-keyed.
pub fn template_for_role(role_name: &str, scope: &str, perms: &[String]) -> String {
    if scope == "platform" && role_name == "platform_admin" {
        // Admin bypass: `resource in ?resource` with ?resource = Platform::"root"
        // deliberately cascades down the hierarchy so admins reach every
        // workspace. Only the platform-scoped platform_admin gets this — gating
        // on scope AND name prevents a workspace role named `platform_admin`
        // from inheriting the unrestricted body.
        return "permit(principal in ?principal, action, resource in ?resource);".to_string();
    }
    let actions = perms
        .iter()
        .map(|p| format!("Egret::Action::\"{p}\""))
        .collect::<Vec<_>>()
        .join(", ");
    let op = if scope == "workspace" { "in" } else { "==" };
    // workspace roles cascade to objects (folder→item, ws→object, object→itself via `in`);
    // platform roles keep `==` so a platform grant never matches a workspace/object resource
    // (anti-leak from B1b-2). Object grants use workspace roles, so they cascade correctly.
    format!("permit(principal in ?principal, action in [{actions}], resource {op} ?resource);")
}

/// The `forbid` counterpart of `template_for_role`: same effective action list
/// and same scope operator, but forbids. A deny grant links this so Cedar's
/// forbid-beats-permit gives deny-wins. The platform-scoped `platform_admin`
/// gets an unrestricted deny-all (mirroring its unrestricted permit).
pub fn deny_template_for_role(role_name: &str, scope: &str, perms: &[String]) -> String {
    if scope == "platform" && role_name == "platform_admin" {
        return "forbid(principal in ?principal, action, resource in ?resource);".to_string();
    }
    let actions = perms
        .iter()
        .map(|p| format!("Egret::Action::\"{p}\""))
        .collect::<Vec<_>>()
        .join(", ");
    let op = if scope == "workspace" { "in" } else { "==" };
    format!("forbid(principal in ?principal, action in [{actions}], resource {op} ?resource);")
}

#[cfg(test)]
mod tests {
    use super::*;
    use cedar_policy::{PolicySet, Template};

    #[test]
    fn admin_template_parses_as_a_template() {
        let t = template_for_role("platform_admin", "platform", &[]);
        // The platform-scoped platform_admin gets the unrestricted body.
        assert!(t.contains("action, resource in ?resource"));
        let tmpl: Template = t.parse().expect("admin template parses");
        let mut ps = PolicySet::new();
        ps.add_template(tmpl).expect("template added");
    }

    #[test]
    fn role_template_lists_actions_and_parses() {
        let t = template_for_role(
            "ws_editor",
            "workspace",
            &["dashboard:read".into(), "dashboard:manage".into()],
        );
        assert!(t.contains("Egret::Action::\"dashboard:read\""));
        // Workspace roles cascade to objects (folder→item, ws→object): use `in`.
        assert!(
            t.contains("action in ["),
            "workspace role must list its perms, got: {t}"
        );
        assert!(
            t.contains("resource in ?resource"),
            "workspace role must use cascading `in`, got: {t}"
        );
        let tmpl: Template = t.parse().expect("role template parses");
        let mut ps = PolicySet::new();
        ps.add_template(tmpl).unwrap();
    }

    #[test]
    fn platform_scope_non_admin_role_uses_exact_match() {
        // A platform-scoped non-admin role keeps `==` so a platform grant never
        // matches a workspace/object resource (anti-leak from B1b-2).
        let t = template_for_role("platform_viewer", "platform", &["dashboard:read".into()]);
        assert!(
            t.contains("action in [") && t.contains("Egret::Action::\"dashboard:read\""),
            "platform role must list its perms, got: {t}"
        );
        assert!(
            t.contains("resource == ?resource"),
            "platform non-admin role must use exact-match `==`, got: {t}"
        );
        let tmpl: Template = t.parse().expect("platform role template parses");
        let mut ps = PolicySet::new();
        ps.add_template(tmpl).unwrap();
    }

    #[test]
    fn empty_actions_non_admin_parses() {
        let t = template_for_role("ws_viewer", "workspace", &[]);
        let tmpl: Template = t.parse().expect("empty-actions template parses");
        let mut ps = PolicySet::new();
        ps.add_template(tmpl).expect("empty-actions template added");
    }

    #[test]
    fn deny_template_forbids_and_parses() {
        let t = deny_template_for_role("ws_editor", "workspace", &["dashboard:read".into()]);
        assert!(t.starts_with("forbid("));
        assert!(t.contains("resource in ?resource"));
        let tmpl: Template = t.parse().expect("deny template parses");
        let mut ps = PolicySet::new();
        ps.add_template(tmpl).unwrap();
    }

    #[test]
    fn workspace_role_named_platform_admin_is_not_unrestricted() {
        // Security review #2b: a workspace-scoped role that merely shares the name
        // `platform_admin` must NOT inherit the unrestricted admin body — it must
        // list its own permissions. Being workspace-scoped it uses cascading `in`,
        // but with its own `action in [...]` perm list, NOT the bare-`action` bypass.
        let t = template_for_role("platform_admin", "workspace", &["dashboard:read".into()]);
        assert!(
            t.contains("action in [") && t.contains("Egret::Action::\"dashboard:read\""),
            "workspace platform_admin must list its perms, got: {t}"
        );
        assert!(
            t.contains("resource in ?resource"),
            "workspace platform_admin (workspace scope) uses cascading `in`, got: {t}"
        );
        assert!(
            !t.contains("action, resource in ?resource"),
            "workspace platform_admin must NOT get the unrestricted bypass body, got: {t}"
        );
    }
}
