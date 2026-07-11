//! Convert a `Subject` + its `Grant`s + a `Resource` into Cedar `Entities`,
//! request UIDs, and per-grant template-linked policies.

use cedar_policy::{Entities, Entity, EntityUid, PolicyId, PolicySet, SlotId};
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
pub fn build_entities(subject: &Subject, resource: &Resource) -> Entities {
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
            // Object in (folder | workspace).
            let obj: EntityUid = format!("Egret::Object::\"{}:{}\"", o.kind, o.id)
                .parse()
                .unwrap();
            es.push(Entity::new_no_attrs(
                obj,
                std::iter::once(object_parent).collect(),
            ));
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
        let template_id: PolicyId = format!("role:{}", g.role_id).parse().unwrap();
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
        let entities = build_entities(&subject, &resource);
        let (p, a, r) = request_uids(&subject, "dashboard:read", &resource);
        let req = Request::new(p, a, r, Context::empty(), None).unwrap();
        let resp = Authorizer::new().is_authorized(&req, &ps, &entities);
        assert_eq!(resp.decision(), Decision::Allow);

        // A different workspace is denied.
        let other = Resource::Workspace(WorkspaceId::new());
        let entities2 = build_entities(&subject, &other);
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
        let entities = build_entities(subject, resource);
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
