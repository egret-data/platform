//! Assemble the Cedar template set the PDP's `load_inner` builds, from a neutral
//! input. Same PolicyIds, same formatters (EO2). No DB, no clock: pure.
use std::collections::HashMap;

use cedar_policy::{Policy, PolicyId, PolicySet, Template};
use uuid::Uuid;

use crate::text;

#[derive(Debug, Clone)]
pub struct RoleInput {
    pub id: Uuid,
    pub name: String,
    pub scope: String,
    pub effective_perms: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct ClassificationInput {
    pub tag_id: Uuid,
    pub key: String,
    pub required_roles: Vec<String>,
    pub required_groups: Vec<Uuid>,
}

#[derive(Debug, Clone, Default)]
pub struct CompileInputs {
    pub roles: Vec<RoleInput>,
    pub classifications: Vec<ClassificationInput>,
}

pub struct Compiled {
    pub policy_set: PolicySet,
    pub role_names: HashMap<Uuid, String>,
    /// tag_ids whose classification forbid failed to parse and was skipped —
    /// mirrors `load_inner`'s log-and-skip so a malformed row does not fail the
    /// whole compile (and the differential stays equivalent to the PDP).
    pub skipped_classifications: Vec<Uuid>,
}

#[derive(Debug)]
pub enum CompileError {
    Policy(String),
}

/// Build the template set: per role a permit + forbid template (keyed
/// `role:{id}` / `role-deny:{id}`), the one static admin-graph forbid, and one
/// classification forbid per gated tag (both-empty skipped; malformed skipped,
/// exactly as `load_inner`).
pub fn compile(inputs: &CompileInputs) -> Result<Compiled, CompileError> {
    let mut policy_set = PolicySet::new();
    let mut role_names = HashMap::new();

    for r in &inputs.roles {
        let tid: PolicyId = text::role_template_id(r.id)
            .parse()
            .map_err(|e| CompileError::Policy(format!("{e:?}")))?;
        let tmpl = Template::parse(
            Some(tid),
            text::template_for_role(&r.name, &r.scope, &r.effective_perms),
        )
        .map_err(|e| CompileError::Policy(format!("{e:?}")))?;
        policy_set
            .add_template(tmpl)
            .map_err(|e| CompileError::Policy(format!("{e:?}")))?;

        let did: PolicyId = text::role_deny_id(r.id)
            .parse()
            .map_err(|e| CompileError::Policy(format!("{e:?}")))?;
        let dtmpl = Template::parse(
            Some(did),
            text::deny_template_for_role(&r.name, &r.scope, &r.effective_perms),
        )
        .map_err(|e| CompileError::Policy(format!("{e:?}")))?;
        policy_set
            .add_template(dtmpl)
            .map_err(|e| CompileError::Policy(format!("{e:?}")))?;

        role_names.insert(r.id, r.name.clone());
    }

    let forbid: Policy = text::admin_graph_forbid()
        .parse()
        .map_err(|e| CompileError::Policy(format!("admin forbid: {e:?}")))?;
    policy_set
        .add(forbid)
        .map_err(|e| CompileError::Policy(format!("add admin forbid: {e:?}")))?;

    let mut skipped_classifications = Vec::new();
    for c in &inputs.classifications {
        // Both empty ⇒ informational tag, no gate ⇒ emit nothing (as load_inner).
        if c.required_roles.is_empty() && c.required_groups.is_empty() {
            continue;
        }
        let pid: PolicyId = match text::classification_id(c.tag_id).parse() {
            Ok(p) => p,
            Err(_) => {
                skipped_classifications.push(c.tag_id);
                continue;
            }
        };
        match Policy::parse(
            Some(pid),
            text::classification_forbid(&c.key, &c.required_roles, &c.required_groups),
        ) {
            Ok(p) => {
                if policy_set.add(p).is_err() {
                    skipped_classifications.push(c.tag_id);
                }
            }
            Err(_) => skipped_classifications.push(c.tag_id),
        }
    }

    Ok(Compiled {
        policy_set,
        role_names,
        skipped_classifications,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn role(id: Uuid, name: &str, scope: &str, perms: &[&str]) -> RoleInput {
        RoleInput {
            id,
            name: name.into(),
            scope: scope.into(),
            effective_perms: perms.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn compile_emits_role_templates_admin_forbid_and_classification() {
        let r = Uuid::from_u128(1);
        let tag = Uuid::from_u128(2);
        let inputs = CompileInputs {
            roles: vec![role(
                r,
                "ws_editor",
                "workspace",
                &["dashboard:read", "dashboard:manage"],
            )],
            classifications: vec![
                ClassificationInput {
                    tag_id: tag,
                    key: "confidential".into(),
                    required_roles: vec!["finance_cleared".into()],
                    required_groups: vec![],
                },
                // both-empty → must be skipped, emits nothing.
                ClassificationInput {
                    tag_id: Uuid::from_u128(3),
                    key: "public".into(),
                    required_roles: vec![],
                    required_groups: vec![],
                },
            ],
        };
        let out = compile(&inputs).expect("compile");
        // The role's permit + forbid templates are present by PolicyId.
        let ids: Vec<String> = out
            .policy_set
            .templates()
            .map(|t| t.id().to_string())
            .collect();
        assert!(
            ids.contains(&format!("role:{r}")),
            "permit template, got {ids:?}"
        );
        assert!(
            ids.contains(&format!("role-deny:{r}")),
            "forbid template, got {ids:?}"
        );
        // Static policies: the admin-graph forbid + one classification forbid.
        let static_ids: Vec<String> = out
            .policy_set
            .policies()
            .map(|p| p.id().to_string())
            .collect();
        assert!(
            static_ids
                .iter()
                .any(|i| i == &format!("classification:{tag}")),
            "classification forbid present, got {static_ids:?}"
        );
        // The both-empty classification was skipped (no policy, not in skipped list
        // because it was never a gate).
        assert!(!static_ids
            .iter()
            .any(|i| i == &format!("classification:{}", Uuid::from_u128(3))));
        assert_eq!(
            out.role_names.get(&r).map(String::as_str),
            Some("ws_editor")
        );
    }
}
