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

/// The Cedar `forbid` body for a **classification (clearance) policy** (Access
/// P3.3 + P3.4): a resource carrying `tag_key` is FORBIDDEN unless the request
/// context either lists one of `required_roles`, belongs to one of
/// `required_groups`, OR carries the scope-correct `is_platform_admin` flag (so a
/// platform superuser is never locked out). Deny-wins holds (Cedar forbid >
/// permit), so this overrides any permit that would otherwise grant access to a
/// tagged object.
///
/// CLEARANCE model (P3.4): in egret, OIDC groups model departments. A principal
/// is cleared if they hold a required ROLE **OR** belong to a required GROUP —
/// the two disjuncts are OR-ed. If `required_groups` is empty its disjunct is
/// OMITTED entirely (never `containsAny([])`, which would be a vacuous `false`
/// and — being harmless — is skipped for clarity); likewise `required_roles`.
///
/// The platform-admin exemption is keyed on `context.is_platform_admin` — a flag
/// the PDP computes scope-correctly (platform scope + name `platform_admin`) —
/// NOT on a bare `"platform_admin"` role NAME in `context.roles`. A workspace
/// role that merely happens to be named `platform_admin` therefore CANNOT bypass
/// the gate (its name would land in `context.roles`, but never sets the flag).
///
/// `has` guards (`resource has tags`, `context has roles`, `context has groups`,
/// `context has is_platform_admin`) keep the policy inert on a resource/context
/// lacking the attribute rather than erroring. `tag_key` and the role names are
/// admin-defined **validated** identifiers (lowercase alnum/`:`/`_`/`-`); the
/// group ids are `Uuid`-typed (read from a `uuid[]` column, inherently valid) —
/// all are emitted as quoted Cedar string literals.
pub fn classification_forbid(
    tag_key: &str,
    required_roles: &[String],
    required_groups: &[uuid::Uuid],
) -> String {
    // Each clearance disjunct is a `context.X.containsAny([..])` guarded by a
    // `context has X`. Emit a disjunct ONLY for a non-empty list.
    let mut disjuncts: Vec<String> = Vec::new();
    if !required_roles.is_empty() {
        let roles = required_roles
            .iter()
            .map(|r| format!("\"{r}\""))
            .collect::<Vec<_>>()
            .join(", ");
        disjuncts.push(format!(
            "(context has roles && context.roles.containsAny([{roles}]))"
        ));
    }
    if !required_groups.is_empty() {
        // Group ids are `Uuid`-typed (not free text) so splicing them as quoted
        // string literals is injection-safe by construction.
        let groups = required_groups
            .iter()
            .map(|g| format!("\"{g}\""))
            .collect::<Vec<_>>()
            .join(", ");
        disjuncts.push(format!(
            "(context has groups && context.groups.containsAny([{groups}]))"
        ));
    }
    // The scope-correct platform_admin exemption is ALWAYS present.
    disjuncts.push("(context has is_platform_admin && context.is_platform_admin)".to_string());
    let unless = disjuncts.join(" || ");
    format!(
        "forbid(principal, action, resource) \
         when {{ resource has tags && resource.tags.contains(\"{tag_key}\") }} \
         unless {{ {unless} }};"
    )
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
    fn classification_forbid_uses_scoped_admin_flag_not_bare_name() {
        // FIX 3: the exemption is keyed on `context.is_platform_admin` (computed
        // scope-correctly by the PDP), NOT on a bare `"platform_admin"` role name
        // in `context.roles`. So the emitted string must (a) reference the flag in
        // a disjunction and (b) NOT append the bare name literal.
        let s = classification_forbid("confidential", &["finance_cleared".into()], &[]);
        assert!(
            s.contains("context has is_platform_admin && context.is_platform_admin"),
            "forbid must gate on the scope-correct is_platform_admin flag, got: {s}"
        );
        assert!(
            s.contains("context.roles.containsAny([\"finance_cleared\"])"),
            "forbid must still honor the required clearance role, got: {s}"
        );
        assert!(
            !s.contains("\"platform_admin\""),
            "forbid must NOT append a bare platform_admin role literal, got: {s}"
        );
        // With no required_groups, the group disjunct is omitted entirely.
        assert!(
            !s.contains("context.groups"),
            "empty required_groups must omit the group disjunct, got: {s}"
        );
        // It must parse as a Cedar policy.
        cedar_policy::Policy::parse(None, &s).expect("classification forbid parses");
    }

    #[test]
    fn classification_forbid_emits_group_disjunct_and_parses() {
        // P3.4: a group-only gate emits a `context.groups.containsAny([<uuid>])`
        // disjunct (guarded by `context has groups`), NO roles disjunct, and still
        // carries the platform_admin flag exemption.
        let g1 = uuid::Uuid::new_v4();
        let g2 = uuid::Uuid::new_v4();
        let s = classification_forbid("confidential", &[], &[g1, g2]);
        assert!(
            s.contains(&format!("context.groups.containsAny([\"{g1}\", \"{g2}\"])")),
            "forbid must honor the required clearance groups as quoted uuids, got: {s}"
        );
        assert!(
            s.contains("context has groups"),
            "group disjunct must be `has`-guarded, got: {s}"
        );
        assert!(
            !s.contains("context.roles"),
            "an empty required_roles must omit the roles disjunct, got: {s}"
        );
        assert!(
            s.contains("context has is_platform_admin && context.is_platform_admin"),
            "the platform_admin exemption stays, got: {s}"
        );
        cedar_policy::Policy::parse(None, &s).expect("group classification forbid parses");
    }

    #[test]
    fn classification_forbid_both_roles_and_groups_disjoined() {
        // Both lists non-empty → roles OR groups OR admin-flag.
        let g1 = uuid::Uuid::new_v4();
        let s = classification_forbid("confidential", &["finance_cleared".into()], &[g1]);
        assert!(s.contains("context.roles.containsAny([\"finance_cleared\"])"));
        assert!(s.contains(&format!("context.groups.containsAny([\"{g1}\"])")));
        // The two clearance disjuncts must be OR-ed (not AND-ed).
        assert!(
            s.contains(")) || ((context has groups") || s.contains(")) || (context has groups"),
            "roles and groups disjuncts must be OR-ed, got: {s}"
        );
        cedar_policy::Policy::parse(None, &s).expect("combined classification forbid parses");
    }

    #[test]
    fn ws_role_named_platform_admin_does_not_bypass_classification() {
        use cedar_policy::{Authorizer, Context, Decision, Entities, Entity, Request};
        // Build a minimal tagged-object decision against ONLY the classification
        // forbid (no permit needed — absent any permit Cedar denies by default, so
        // we assert on the forbid firing by comparing tagged vs untagged and
        // bare-name vs flag).
        let policy = cedar_policy::Policy::parse(
            Some("classification:test".parse().unwrap()),
            classification_forbid("confidential", &["finance_cleared".into()], &[]),
        )
        .unwrap();
        let mut ps = PolicySet::new();
        // A blanket permit so the ONLY thing that can deny is the forbid.
        ps.add(
            cedar_policy::Policy::parse(
                Some("p".parse().unwrap()),
                "permit(principal, action, resource);",
            )
            .unwrap(),
        )
        .unwrap();
        ps.add(policy).unwrap();

        let user: cedar_policy::EntityUid = "Egret::User::\"u\"".parse().unwrap();
        let obj: cedar_policy::EntityUid = "Egret::Object::\"dashboard:d1\"".parse().unwrap();
        let action: cedar_policy::EntityUid = "Egret::Action::\"dashboard:read\"".parse().unwrap();
        let mut attrs = std::collections::HashMap::new();
        attrs.insert(
            "tags".to_string(),
            cedar_policy::RestrictedExpression::new_set(std::iter::once(
                cedar_policy::RestrictedExpression::new_string("confidential".into()),
            )),
        );
        let ents = Entities::from_entities(
            vec![
                Entity::new_no_attrs(user.clone(), Default::default()),
                Entity::new(obj.clone(), attrs, Default::default()).unwrap(),
            ],
            None,
        )
        .unwrap();

        let decide = |roles: &[&str], is_admin: bool| -> Decision {
            let ctx = Context::from_json_value(
                serde_json::json!({ "is_platform_admin": is_admin, "roles": roles }),
                None,
            )
            .unwrap();
            let req = Request::new(user.clone(), action.clone(), obj.clone(), ctx, None).unwrap();
            Authorizer::new().is_authorized(&req, &ps, &ents).decision()
        };

        // Bare role name `platform_admin` WITHOUT the flag → still DENIED.
        assert_eq!(
            decide(&["platform_admin"], false),
            Decision::Deny,
            "a bare platform_admin role name must NOT bypass the gate"
        );
        // Only the scope-correct flag allows.
        assert_eq!(
            decide(&[], true),
            Decision::Allow,
            "the scope-correct is_platform_admin flag must exempt"
        );
        // The genuine clearance role also allows.
        assert_eq!(decide(&["finance_cleared"], false), Decision::Allow);
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
