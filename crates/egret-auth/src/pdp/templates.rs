//! One Cedar policy template per role, body = the role's permitted actions.
//! Regenerated whenever `role_permissions` changes.

/// Build a Cedar template string for a role.
/// `platform_admin` gets an unrestricted body; every other role lists its actions.
pub fn template_for_role(role_name: &str, perms: &[String]) -> String {
    if role_name == "platform_admin" {
        return format!(
            "@id(\"role:{role_name}\")\npermit(principal in ?principal, action, resource in ?resource);"
        );
    }
    let actions = perms
        .iter()
        .map(|p| format!("Egret::Action::\"{p}\""))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "@id(\"role:{role_name}\")\npermit(principal in ?principal, action in [{actions}], resource in ?resource);"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use cedar_policy::{PolicySet, Template};

    #[test]
    fn admin_template_parses_as_a_template() {
        let t = template_for_role("platform_admin", &[]);
        let tmpl: Template = t.parse().expect("admin template parses");
        let mut ps = PolicySet::new();
        ps.add_template(tmpl).expect("template added");
    }

    #[test]
    fn role_template_lists_actions_and_parses() {
        let t = template_for_role("ws_editor", &["dashboard:read".into(), "dashboard:manage".into()]);
        assert!(t.contains("Egret::Action::\"dashboard:read\""));
        let tmpl: Template = t.parse().expect("role template parses");
        let mut ps = PolicySet::new();
        ps.add_template(tmpl).unwrap();
    }

    #[test]
    fn empty_actions_non_admin_parses() {
        let t = template_for_role("ws_viewer", &[]);
        let tmpl: Template = t.parse().expect("empty-actions template parses");
        let mut ps = PolicySet::new();
        ps.add_template(tmpl).expect("empty-actions template added");
    }
}
