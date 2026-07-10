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
    }
}
fn scope_uid(scope_type: &str, scope_id: Option<uuid::Uuid>) -> EntityUid {
    match scope_type {
        "platform" => "Egret::Platform::\"root\"".parse().unwrap(),
        "workspace" => format!(
            "Egret::Workspace::\"{}\"",
            scope_id.expect("ws scope has id")
        )
        .parse()
        .unwrap(),
        other => panic!("unsupported scope_type in B1a: {other}"),
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
    if let Resource::Workspace(w) = resource {
        let ws: EntityUid = format!("Egret::Workspace::\"{}\"", w.0).parse().unwrap();
        es.push(Entity::new_no_attrs(
            ws,
            std::iter::once(platform).collect(),
        ));
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
        let Some(role) = role_names.get(&g.role_id) else {
            continue;
        };
        let template_id: PolicyId = format!("role:{role}").parse().unwrap();
        let link_id: PolicyId = format!("grant-{i}").parse().unwrap();
        let mut vals = HashMap::new();
        vals.insert(SlotId::principal(), user_uid(subject));
        vals.insert(SlotId::resource(), scope_uid(&g.scope_type, g.scope_id));
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
        // set explicitly so `link_grants` can find it by `role:<name>`.
        let tmpl = Template::parse(
            Some("role:ws_editor".parse().unwrap()),
            template_for_role("ws_editor", &["dashboard:read".into()]),
        )
        .unwrap();
        ps.add_template(tmpl).unwrap();
        let names: RoleNames = std::iter::once((role_id, "ws_editor".to_string())).collect();
        let grants = vec![Grant {
            role_id,
            scope_type: "workspace".into(),
            scope_id: Some(ws.0),
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
}
