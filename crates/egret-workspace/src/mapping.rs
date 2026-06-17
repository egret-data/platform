//! Map OIDC claims to platform role names per configured rules. Consumed by 1a-3.

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct IdpRoleMapping {
    pub claim: String,
    pub value: String,
    pub role: String,
    pub scope: String, // "platform" (workspace mappings handled in a later phase)
}

/// Return the role names whose mapping rule matches the claims. A rule matches when
/// `claims[rule.claim]` equals `rule.value` (string) or contains it (array of strings).
pub fn map_idp_claims_to_roles(
    claims: &serde_json::Value,
    rules: &[IdpRoleMapping],
) -> Vec<String> {
    let mut out = Vec::new();
    for rule in rules {
        let matched = match claims.get(&rule.claim) {
            Some(serde_json::Value::String(s)) => s == &rule.value,
            Some(serde_json::Value::Array(arr)) => {
                arr.iter().any(|v| v.as_str() == Some(rule.value.as_str()))
            }
            _ => false,
        };
        if matched && !out.contains(&rule.role) {
            out.push(rule.role.clone());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(claim: &str, value: &str, role: &str) -> IdpRoleMapping {
        IdpRoleMapping {
            claim: claim.into(),
            value: value.into(),
            role: role.into(),
            scope: "platform".into(),
        }
    }

    #[test]
    fn matches_array_claim() {
        let claims = serde_json::json!({ "groups": ["a", "data-admins"] });
        let roles =
            map_idp_claims_to_roles(&claims, &[rule("groups", "data-admins", "platform_admin")]);
        assert_eq!(roles, vec!["platform_admin"]);
    }

    #[test]
    fn matches_string_claim_and_dedupes() {
        let claims = serde_json::json!({ "role": "steward" });
        let rules = [
            rule("role", "steward", "data_steward"),
            rule("role", "steward", "data_steward"),
        ];
        assert_eq!(
            map_idp_claims_to_roles(&claims, &rules),
            vec!["data_steward"]
        );
    }

    #[test]
    fn no_match_returns_empty() {
        let claims = serde_json::json!({ "groups": ["x"] });
        assert!(
            map_idp_claims_to_roles(&claims, &[rule("groups", "y", "platform_admin")]).is_empty()
        );
    }
}
