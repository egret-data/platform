//! Convert a `Subject` + its `Grant`s + a `Resource` into Cedar `Entities`,
//! request UIDs, and per-grant template-linked policies.

use cedar_policy::{
    Entities, Entity, EntityUid, PolicyId, PolicySet, RestrictedExpression, SlotId,
};
use egret_core::authz::{Resource, Subject};
use std::collections::HashMap;

use crate::grants::Grant;

/// Map a role_id -> role name (resolved once per request from the DB by the caller).
pub type RoleNames = HashMap<uuid::Uuid, String>;

fn user_uid(s: &Subject) -> EntityUid {
    format!("Egret::User::\"{}\"", s.user_id.0)
        .parse()
        .expect("user uid")
}
fn resource_uid(r: &Resource) -> EntityUid {
    match r {
        Resource::Platform => "Egret::Platform::\"root\"".parse().unwrap(),
        Resource::Workspace(w) => format!("Egret::Workspace::\"{}\"", w.0).parse().unwrap(),
        Resource::Object(o) => format!("Egret::Object::\"{}:{}\"", o.kind, o.id)
            .parse()
            .unwrap(),
    }
}
fn scope_uid(
    scope_type: &str,
    scope_id: Option<uuid::Uuid>,
    object_type: Option<&str>,
) -> EntityUid {
    match scope_type {
        "platform" => "Egret::Platform::\"root\"".parse().unwrap(),
        "workspace" => format!(
            "Egret::Workspace::\"{}\"",
            scope_id.expect("ws scope has id")
        )
        .parse()
        .unwrap(),
        "object" => format!(
            "Egret::Object::\"{}:{}\"",
            object_type.expect("object scope has object_type"),
            scope_id.expect("object scope has id")
        )
        .parse()
        .unwrap(),
        other => panic!("unsupported scope_type: {other}"),
    }
}

/// Build entities: the user, its group memberships, and the resource hierarchy
/// (Workspace in Platform). Grants' scope entities are included so `resource in
/// ?resource` resolves.
///
/// `object_tags` are the classification tag KEYS carried by the object under
/// decision; they become the Object entity's `tags` Set attribute so a
/// classification `forbid` policy can gate on `resource.tags.contains("<key>")`.
/// For non-Object resources the slice is ignored (pass `&[]`).
pub fn build_entities(subject: &Subject, resource: &Resource, object_tags: &[String]) -> Entities {
    let mut es: Vec<Entity> = Vec::new();
    let groups: std::collections::HashSet<EntityUid> = subject
        .group_ids
        .iter()
        .map(|g| format!("Egret::Group::\"{}\"", g.0).parse().unwrap())
        .collect();
    es.push(Entity::new_no_attrs(user_uid(subject), groups));
    // Platform is the root of the resource hierarchy.
    let platform: EntityUid = "Egret::Platform::\"root\"".parse().unwrap();
    es.push(Entity::new_no_attrs(platform.clone(), Default::default()));
    match resource {
        Resource::Platform => {}
        Resource::Workspace(w) => {
            let ws: EntityUid = format!("Egret::Workspace::\"{}\"", w.0).parse().unwrap();
            es.push(Entity::new_no_attrs(
                ws,
                std::iter::once(platform).collect(),
            ));
        }
        Resource::Object(o) => {
            // Workspace in Platform.
            let ws: EntityUid = format!("Egret::Workspace::\"{}\"", o.workspace_id.0)
                .parse()
                .unwrap();
            es.push(Entity::new_no_attrs(
                ws.clone(),
                std::iter::once(platform).collect(),
            ));
            // The object's parent is its folder (if any), else the workspace.
            let object_parent: EntityUid = match o.folder_id {
                Some(fid) => {
                    let folder: EntityUid =
                        format!("Egret::Object::\"folder:{fid}\"").parse().unwrap();
                    // Folder in Workspace.
                    es.push(Entity::new_no_attrs(
                        folder.clone(),
                        std::iter::once(ws).collect(),
                    ));
                    folder
                }
                None => ws,
            };
            // Object in (folder | workspace), carrying its classification tag keys
            // as a `tags` Set attribute so a classification `forbid` can gate on
            // `resource.tags.contains("<key>")`.
            let obj: EntityUid = format!("Egret::Object::\"{}:{}\"", o.kind, o.id)
                .parse()
                .unwrap();
            let mut attrs = HashMap::new();
            attrs.insert(
                "tags".to_string(),
                RestrictedExpression::new_set(
                    object_tags
                        .iter()
                        .cloned()
                        .map(RestrictedExpression::new_string),
                ),
            );
            es.push(
                Entity::new(obj, attrs, std::iter::once(object_parent).collect())
                    .expect("object entity"),
            );
        }
    }
    Entities::from_entities(es, None).expect("entities build")
}

/// One template-linked policy per grant, binding ?principal (the granted
/// principal) and ?resource (the grant's scope). Returns the augmented policy set.
pub fn link_grants(
    mut policies: PolicySet,
    subject: &Subject,
    grants: &[Grant],
    role_names: &RoleNames,
) -> PolicySet {
    for (i, g) in grants.iter().enumerate() {
        // `role_names` is the known-role set; skip grants whose role_id has no
        // compiled template (e.g. a role created after the PDP was built).
        if !role_names.contains_key(&g.role_id) {
            continue;
        }
        // Templates are keyed by role_id (UUID), matching `CedarPdp::build`, so a
        // workspace role sharing a platform role's name links its OWN template.
        // A deny grant links the `forbid` counterpart (`role-deny:{id}`) so Cedar's
        // forbid-beats-permit yields deny-wins.
        let template_id: PolicyId = if g.effect == "deny" {
            format!("role-deny:{}", g.role_id).parse().unwrap()
        } else {
            format!("role:{}", g.role_id).parse().unwrap()
        };
        let link_id: PolicyId = format!("grant-{i}").parse().unwrap();
        let mut vals = HashMap::new();
        vals.insert(SlotId::principal(), user_uid(subject));
        vals.insert(
            SlotId::resource(),
            scope_uid(&g.scope_type, g.scope_id, g.object_type.as_deref()),
        );
        policies
            .link(template_id, link_id, vals)
            .expect("link grant");
    }
    policies
}

pub fn request_uids(
    subject: &Subject,
    action: &str,
    resource: &Resource,
) -> (EntityUid, EntityUid, EntityUid) {
    (
        user_uid(subject),
        format!("Egret::Action::\"{action}\"")
            .parse()
            .expect("action uid"),
        resource_uid(resource),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pdp::templates::template_for_role;
    use cedar_policy::{Authorizer, Context, Decision, Request, Template};
    use egret_core::ids::{UserId, WorkspaceId};

    #[test]
    fn workspace_grant_allows_scoped_action() {
        let uid = UserId::new();
        let ws = WorkspaceId::new();
        let subject = Subject {
            user_id: uid,
            group_ids: vec![],
        };
        let role_id = uuid::Uuid::new_v4();

        let mut ps = PolicySet::new();
        // Cedar's `@id(...)` annotation is NOT used as the template PolicyId by
        // `FromStr`/`parse(None, ..)` (it defaults to "policy0"); the id must be
        // set explicitly so `link_grants` can find it by `role:<role_id>`.
        let tmpl = Template::parse(
            Some(format!("role:{role_id}").parse().unwrap()),
            template_for_role("ws_editor", "workspace", &["dashboard:read".into()]),
        )
        .unwrap();
        ps.add_template(tmpl).unwrap();
        let names: RoleNames = std::iter::once((role_id, "ws_editor".to_string())).collect();
        let grants = vec![Grant {
            role_id,
            scope_type: "workspace".into(),
            scope_id: Some(ws.0),
            object_type: None,
            effect: "allow".into(),
        }];
        let ps = link_grants(ps, &subject, &grants, &names);

        let resource = Resource::Workspace(ws);
        let entities = build_entities(&subject, &resource, &[]);
        let (p, a, r) = request_uids(&subject, "dashboard:read", &resource);
        let req = Request::new(p, a, r, Context::empty(), None).unwrap();
        let resp = Authorizer::new().is_authorized(&req, &ps, &entities);
        assert_eq!(resp.decision(), Decision::Allow);

        // A different workspace is denied.
        let other = Resource::Workspace(WorkspaceId::new());
        let entities2 = build_entities(&subject, &other, &[]);
        let (p2, a2, r2) = request_uids(&subject, "dashboard:read", &other);
        let req2 = Request::new(p2, a2, r2, Context::empty(), None).unwrap();
        assert_eq!(
            Authorizer::new()
                .is_authorized(&req2, &ps, &entities2)
                .decision(),
            Decision::Deny
        );
    }

    /// Build a linked policy set for a ws_viewer template scoped to a single
    /// object-scoped grant, plus the (role_id) used.
    fn viewer_ps_for_grant(subject: &Subject, grant: Grant) -> PolicySet {
        let role_id = grant.role_id;
        let mut ps = PolicySet::new();
        let tmpl = Template::parse(
            Some(format!("role:{role_id}").parse().unwrap()),
            template_for_role("ws_viewer", "workspace", &["dashboard:read".into()]),
        )
        .unwrap();
        ps.add_template(tmpl).unwrap();
        let names: RoleNames = std::iter::once((role_id, "ws_viewer".to_string())).collect();
        link_grants(ps, subject, &[grant], &names)
    }

    fn decide(ps: &PolicySet, subject: &Subject, resource: &Resource) -> Decision {
        let entities = build_entities(subject, resource, &[]);
        let (p, a, r) = request_uids(subject, "dashboard:read", resource);
        let req = Request::new(p, a, r, Context::empty(), None).unwrap();
        Authorizer::new()
            .is_authorized(&req, ps, &entities)
            .decision()
    }

    fn object(kind: &str, id: uuid::Uuid, folder: Option<uuid::Uuid>, ws: WorkspaceId) -> Resource {
        Resource::Object(egret_core::authz::ObjectRef {
            kind: kind.into(),
            id,
            folder_id: folder,
            workspace_id: ws,
        })
    }

    /// Access P3.3: a classification `forbid` gates a tagged object on the
    /// request context's `roles`. Deny-wins (forbid > permit) so a tagged object
    /// is DENIED unless the context lists a cleared role (or platform_admin).
    #[test]
    fn classification_forbid_gates_tagged_object_on_context_roles() {
        use crate::pdp::templates::classification_forbid;

        let subject = Subject {
            user_id: UserId::new(),
            group_ids: vec![],
        };
        let ws = WorkspaceId::new();
        let d1 = uuid::Uuid::new_v4();
        let role_id = uuid::Uuid::new_v4();

        // A ws_viewer template linked to a workspace grant → permits dashboard:read.
        let mut base = PolicySet::new();
        base.add_template(
            Template::parse(
                Some(format!("role:{role_id}").parse().unwrap()),
                template_for_role("ws_viewer", "workspace", &["dashboard:read".into()]),
            )
            .unwrap(),
        )
        .unwrap();
        let names: RoleNames = std::iter::once((role_id, "ws_viewer".to_string())).collect();
        let grants = vec![Grant {
            role_id,
            scope_type: "workspace".into(),
            scope_id: Some(ws.0),
            object_type: None,
            effect: "allow".into(),
        }];
        let permit_ps = link_grants(base, &subject, &grants, &names);

        // + a classification forbid: tag `confidential` requires `finance_cleared`.
        let mut gated = permit_ps.clone();
        gated
            .add(
                cedar_policy::Policy::parse(
                    Some("classification:test".parse().unwrap()),
                    classification_forbid("confidential", &["finance_cleared".into()]),
                )
                .unwrap(),
            )
            .unwrap();

        // Decide a dashboard-read on `d1` with the given context.roles, tag keys,
        // and scope-correct `is_platform_admin` flag.
        let decide_ctx =
            |ps: &PolicySet, tags: &[String], roles: &[&str], is_admin: bool| -> Decision {
                let resource = object("dashboard", d1, None, ws);
                let entities = build_entities(&subject, &resource, tags);
                let (p, a, r) = request_uids(&subject, "dashboard:read", &resource);
                let ctx = Context::from_json_value(
                    serde_json::json!({ "is_platform_admin": is_admin, "roles": roles }),
                    None,
                )
                .unwrap();
                let req = Request::new(p, a, r, ctx, None).unwrap();
                Authorizer::new()
                    .is_authorized(&req, ps, &entities)
                    .decision()
            };

        let confidential = vec!["confidential".to_string()];

        // (a) tagged + NOT cleared (only ws_viewer) → Deny.
        assert_eq!(
            decide_ctx(&gated, &confidential, &["ws_viewer"], false),
            Decision::Deny,
            "(a) tagged object without a cleared role must be denied"
        );
        // (b) tagged + cleared (finance_cleared) → Allow.
        assert_eq!(
            decide_ctx(
                &gated,
                &confidential,
                &["ws_viewer", "finance_cleared"],
                false
            ),
            Decision::Allow,
            "(b) a cleared role must satisfy the forbid's unless"
        );
        // (c) tagged + scope-correct is_platform_admin flag → Allow (never locked out).
        assert_eq!(
            decide_ctx(&gated, &confidential, &[], true),
            Decision::Allow,
            "(c) a platform_admin (scope-correct flag) is never locked out of a tagged object"
        );
        // (c2) FIX 3: a WORKSPACE role merely NAMED `platform_admin` (bare name in
        // context.roles, WITHOUT the is_platform_admin flag) must NOT bypass — the
        // exemption keys on the scope-correct flag, not the role name.
        assert_eq!(
            decide_ctx(&gated, &confidential, &["platform_admin"], false),
            Decision::Deny,
            "(c2) a ws role named platform_admin must NOT bypass without the flag"
        );
        // (d) SAME forbid in the set but object UNtagged (tags:[]) → Allow (when is false).
        assert_eq!(
            decide_ctx(&gated, &[], &["ws_viewer"], false),
            Decision::Allow,
            "(d) an untagged object is unaffected by the classification forbid"
        );
        // (e) object tagged confidential but NO classification forbid in the set → Allow.
        assert_eq!(
            decide_ctx(&permit_ps, &confidential, &["ws_viewer"], false),
            Decision::Allow,
            "(e) a tag with no policy is informational only — no gate"
        );
        // (f) deny-grant-not-clearance (FIX 2, at the Cedar layer): `authorize`
        // builds context.roles from ALLOW grants only, so a subject whose ONLY
        // source of `finance_cleared` is a DENY grant arrives here with that role
        // ABSENT from context.roles → the gate still denies.
        assert_eq!(
            decide_ctx(&gated, &confidential, &["ws_viewer"], false),
            Decision::Deny,
            "(f) a deny-effect clearance grant is filtered out of context.roles → still denied"
        );
    }

    #[test]
    fn folder_grant_reaches_items_in_that_folder_only() {
        let subject = Subject {
            user_id: UserId::new(),
            group_ids: vec![],
        };
        let ws = WorkspaceId::new();
        let f1 = uuid::Uuid::new_v4();
        let f2 = uuid::Uuid::new_v4();
        let d1 = uuid::Uuid::new_v4();

        // A folder grant on F1: ?resource = Egret::Object::"folder:<F1>".
        let ps = viewer_ps_for_grant(
            &subject,
            Grant {
                role_id: uuid::Uuid::new_v4(),
                scope_type: "object".into(),
                scope_id: Some(f1),
                object_type: Some("folder".into()),
                effect: "allow".into(),
            },
        );

        // A dashboard IN folder F1 is allowed.
        assert_eq!(
            decide(&ps, &subject, &object("dashboard", d1, Some(f1), ws)),
            Decision::Allow,
            "folder grant must reach a dashboard inside that folder"
        );
        // The SAME dashboard in a DIFFERENT folder F2 is denied.
        assert_eq!(
            decide(&ps, &subject, &object("dashboard", d1, Some(f2), ws)),
            Decision::Deny,
            "folder grant must NOT reach an item in a different folder"
        );
    }

    #[test]
    fn item_grant_isolates_a_single_object() {
        let subject = Subject {
            user_id: UserId::new(),
            group_ids: vec![],
        };
        let ws = WorkspaceId::new();
        let f1 = uuid::Uuid::new_v4();
        let d1 = uuid::Uuid::new_v4();
        let d3 = uuid::Uuid::new_v4();

        // An item grant on D1: ?resource = Egret::Object::"dashboard:<D1>".
        let ps = viewer_ps_for_grant(
            &subject,
            Grant {
                role_id: uuid::Uuid::new_v4(),
                scope_type: "object".into(),
                scope_id: Some(d1),
                object_type: Some("dashboard".into()),
                effect: "allow".into(),
            },
        );

        // D1 itself is allowed.
        assert_eq!(
            decide(&ps, &subject, &object("dashboard", d1, Some(f1), ws)),
            Decision::Allow,
            "item grant must authorize the granted object"
        );
        // A sibling D3 in the SAME folder is denied.
        assert_eq!(
            decide(&ps, &subject, &object("dashboard", d3, Some(f1), ws)),
            Decision::Deny,
            "item grant must NOT authorize a sibling in the same folder"
        );
    }
}
