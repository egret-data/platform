//! Renders an engine privilege's [`Statement`] into the one Ranger access
//! policy it stands for, `perm-engine-{id}`, in its engine's profile
//! (governance sub-project 1, Task 9). Every decision — which grants, which
//! skips, the catalogue and target checks, the grantee checks — was made by
//! `crate::engine_grants::check_engine_grant` and
//! `egret_perm_ir::lower::engine_privileges`; this module renders shape only.
//! The resource keys and the access type come from the catalogue entry the
//! statement names (`crate::engine_capabilities`).

use std::collections::BTreeMap;

use egret_perm::{CompileError, Effect, EngineType};
use egret_perm_ir::statement::{Source, Statement, Target};
use egret_policy::admin::{AccessPolicyInput, ResourceLevel};

use crate::engine_capabilities::capability;
use crate::engine_grants::engine_grant_policy_name;

fn malformed(what: &str) -> CompileError {
    CompileError::Db(format!(
        "ranger::engine_privileges::render was handed {what}"
    ))
}

/// Render `s` for `profile` on `engine`: one allow item for the audience's
/// include with the capability's access type, on the capability's resource
/// keys — each keyed value from the statement's values in order, each fixed
/// key `*`. A statement of another family, an unknown capability or one of
/// another engine, or a value count that does not fit the catalogue is a
/// malformed statement (only reachable by building one by hand) and refused.
pub(crate) fn render(
    s: &Statement,
    profile: &str,
    engine: EngineType,
) -> Result<AccessPolicyInput, CompileError> {
    let Source::EnginePrivilege { engine_grant_id } = s.source else {
        return Err(malformed("a statement of another family"));
    };
    let Target::EngineObject {
        capability: id,
        values,
    } = &s.target
    else {
        return Err(malformed("a statement with no engine object"));
    };
    let cap = capability(id).ok_or_else(|| malformed("an unknown capability"))?;
    if cap.engine != engine {
        return Err(malformed("a capability of another engine"));
    }

    let mut values_iter = values.iter();
    let mut resources: BTreeMap<String, ResourceLevel> = BTreeMap::new();
    for (key, kind) in cap.resources {
        let value = match kind {
            Some(_) => values_iter
                .next()
                .ok_or_else(|| malformed("too few values"))?
                .clone(),
            None => "*".to_string(),
        };
        resources.insert(
            (*key).to_string(),
            ResourceLevel {
                values: vec![value],
                is_excludes: false,
                is_recursive: false,
            },
        );
    }
    if values_iter.next().is_some() {
        return Err(malformed("too many values"));
    }

    let item = super::grants::item_from_members(
        &s.audience.include,
        Effect::Allow,
        &[cap.access.to_string()],
    )?;

    Ok(AccessPolicyInput {
        name: engine_grant_policy_name(engine_grant_id),
        service: profile.to_string(),
        is_enabled: true,
        resources,
        items: vec![item],
        allow_exceptions: Vec::new(),
        deny_exceptions: Vec::new(),
        is_deny_all_else: false,
        description: String::new(),
        labels: Vec::new(),
        is_override: false,
        additional: Vec::new(),
    })
}
