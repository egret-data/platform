//! Governance SP2b: what a profile may not hold while egret answers its
//! engines over OPA (C8). The OPA evaluator reads the lowered permission IR,
//! never the stored Ranger policies, so a stored row the IR does not produce
//! would be silently ignored: the `all - …` default set (migration 0180's
//! shape) and every hand-written (`managed_by = 'operator'`) policy of the
//! profile or of its tag service.
//!
//! One copy, two callers: the snapshot refresher re-checks on every build
//! (`egret_perm_compile::opa::cache`), and the switch to OPA refuses up front.

use crate::PolicyStoreError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoredStateRefusal {
    AllDefaults { count: i64 },
    HandWritten { service: String, count: i64 },
}

impl StoredStateRefusal {
    pub fn message(&self) -> String {
        match self {
            Self::AllDefaults { count } => format!(
                "the profile holds the `all - …` default policies ({count}); delete them first"
            ),
            Self::HandWritten { service, count } => format!(
                "{service} holds {count} hand-written policies, which OPA would ignore; \
                 delete them first"
            ),
        }
    }
}

async fn count_via(
    conn: &mut sqlx::PgConnection,
    sql: &'static str,
    service_id: i64,
) -> Result<i64, PolicyStoreError> {
    Ok(sqlx::query_scalar::<_, i64>(sql)
        .bind(service_id)
        .fetch_one(&mut *conn)
        .await?)
}

/// The `all - …` rows of the profile, then the `managed_by = 'operator'` rows of the profile and of
/// the tag service its `config_jsonb->>'tagService'` names. Empty: nothing stored the evaluator ignores.
///
/// `LIKE 'all - %'` is exact: neither `_` nor `%` occurs in `'all - '`
/// (migration 0180 writes the same predicate). The tag service is resolved
/// through [`crate::usable_tag_services_via`], the one spelling of that link.
pub async fn stored_state_refusals_via(
    conn: &mut sqlx::PgConnection,
    service_id: i64,
) -> Result<Vec<StoredStateRefusal>, PolicyStoreError> {
    const ALL_DEFAULTS: &str = "SELECT count(*) FROM policy.authz_policies \
                                 WHERE service_id = $1 AND name LIKE 'all - %'";
    const OPERATOR: &str = "SELECT count(*) FROM policy.authz_policies \
                             WHERE service_id = $1 AND managed_by = 'operator'";
    let mut out = Vec::new();
    let count = count_via(conn, ALL_DEFAULTS, service_id).await?;
    if count > 0 {
        out.push(StoredStateRefusal::AllDefaults { count });
    }
    let profile = crate::get_service_by_id_via(conn, service_id).await?;
    let mut services = vec![(service_id, profile.name)];
    for (_, tag_name) in crate::usable_tag_services_via(conn, &[service_id]).await? {
        if let Some(tag_id) = crate::service_id_by_name_via(conn, &tag_name).await? {
            services.push((tag_id, tag_name));
        }
    }
    for (id, service) in services {
        let count = count_via(conn, OPERATOR, id).await?;
        if count > 0 {
            out.push(StoredStateRefusal::HandWritten { service, count });
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_messages_name_what_to_delete() {
        // Arrange
        let defaults = StoredStateRefusal::AllDefaults { count: 3 };
        let hand = StoredStateRefusal::HandWritten {
            service: "lake_tag".into(),
            count: 2,
        };

        // Act / Assert
        assert_eq!(
            defaults.message(),
            "the profile holds the `all - …` default policies (3); delete them first"
        );
        assert_eq!(
            hand.message(),
            "lake_tag holds 2 hand-written policies, which OPA would ignore; delete them first"
        );
    }
}
