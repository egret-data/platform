//! Package 7f: the shared machinery behind the bulk execution operations.
//!
//! Every bulk route resolves a set of execution ids (listed by the caller, or
//! matched by a filter), runs a read-only check on each, and only when EVERY
//! id passes applies the operation to all of them.

use std::future::Future;

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use egret_core::Error;
use serde::Serialize;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::repo::{ExecutionFilter, ExecutionRepo, Page};

/// The most executions one bulk call may touch. Enforced by each handler
/// before [`bulk_apply`], and by [`resolve_ids_by_query`] for a filter.
pub const MAX_BULK: usize = 1000;

/// `200` body of a bulk operation: how many executions it was applied to.
#[derive(Debug, Serialize, ToSchema)]
pub struct BulkResponse {
    pub count: usize,
}

/// One id a bulk operation refused, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct Invalid {
    pub id: String,
    pub reason: String,
}

/// `400` body of a bulk operation: nothing was applied, and every id that
/// stopped it is listed.
#[derive(Debug, Serialize, ToSchema)]
pub struct BulkErrorResponse {
    pub message: String,
    pub invalids: Vec<Invalid>,
}

/// A refused bulk operation, rendered as `400` with a [`BulkErrorResponse`].
#[derive(Debug)]
pub struct BulkError(pub BulkErrorResponse);

impl IntoResponse for BulkError {
    fn into_response(self) -> Response {
        (StatusCode::BAD_REQUEST, Json(self.0)).into_response()
    }
}

/// Every execution id in `ws` matching `filter`; more than [`MAX_BULK`]
/// matches is `InvalidInput("too_many: ...")` rather than a silent truncation.
pub async fn resolve_ids_by_query(
    executions: &ExecutionRepo,
    ws: Uuid,
    filter: &ExecutionFilter,
) -> Result<Vec<Uuid>, Error> {
    let page = Page {
        limit: MAX_BULK as i64 + 1,
        offset: 0,
    };
    let found = executions.list(ws, filter, page).await?;
    if found.rows.len() > MAX_BULK {
        return Err(Error::InvalidInput(format!(
            "too_many: the filter matches more than {MAX_BULK} executions; narrow it"
        )));
    }
    Ok(found.rows.into_iter().map(|r| r.id).collect())
}

/// Validate-all, then apply-all.
///
/// `check` is read-only and runs for EVERY id (no short-circuit, so the
/// response names every bad id). Any `Invalid` refuses the whole call and
/// `apply` never runs. Otherwise `apply` runs for each id in order.
///
/// The check and the applies are not one transaction: an execution can change
/// between them. The common path is clean, so an `apply` failure is rare; when
/// it happens the call stops and the error propagates, with earlier ids
/// already applied.
pub async fn bulk_apply<Chk, ChkFut, Ap, ApFut>(
    ids: Vec<Uuid>,
    check: Chk,
    apply: Ap,
) -> Result<Json<BulkResponse>, BulkError>
where
    Chk: Fn(Uuid) -> ChkFut,
    ChkFut: Future<Output = Result<(), Invalid>>,
    Ap: Fn(Uuid) -> ApFut,
    ApFut: Future<Output = Result<(), Error>>,
{
    let mut invalids = Vec::new();
    for &id in &ids {
        if let Err(invalid) = check(id).await {
            invalids.push(invalid);
        }
    }
    if !invalids.is_empty() {
        return Err(BulkError(BulkErrorResponse {
            message: "invalid bulk operation".into(),
            invalids,
        }));
    }
    for &id in &ids {
        if let Err(e) = apply(id).await {
            return Err(BulkError(BulkErrorResponse {
                message: "invalid bulk operation".into(),
                invalids: vec![Invalid {
                    id: id.to_string(),
                    reason: e.to_string(),
                }],
            }));
        }
    }
    Ok(Json(BulkResponse { count: ids.len() }))
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    fn ids(n: usize) -> Vec<Uuid> {
        (0..n).map(|_| Uuid::new_v4()).collect()
    }

    #[tokio::test]
    async fn all_valid_applies_every_id() {
        let ran = AtomicUsize::new(0);
        let out = bulk_apply(
            ids(3),
            |_| async { Ok(()) },
            |_| async {
                ran.fetch_add(1, Ordering::SeqCst);
                Ok(())
            },
        )
        .await
        .expect("all valid");
        assert_eq!(out.0.count, 3);
        assert_eq!(ran.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn invalids_are_all_listed_and_nothing_is_applied() {
        let all = ids(3);
        let (bad1, bad2) = (all[0], all[2]);
        let ran = AtomicUsize::new(0);
        let err = bulk_apply(
            all,
            |id| async move {
                if id == bad1 || id == bad2 {
                    Err(Invalid {
                        id: id.to_string(),
                        reason: "nope".into(),
                    })
                } else {
                    Ok(())
                }
            },
            |_| async {
                ran.fetch_add(1, Ordering::SeqCst);
                Ok(())
            },
        )
        .await
        .expect_err("two invalid");
        let got: Vec<&str> = err.0.invalids.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(got, [bad1.to_string(), bad2.to_string()]);
        assert_eq!(ran.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn empty_ids_is_count_zero() {
        let ran = AtomicUsize::new(0);
        let out = bulk_apply(
            vec![],
            |_| async { Ok(()) },
            |_| async {
                ran.fetch_add(1, Ordering::SeqCst);
                Ok(())
            },
        )
        .await
        .expect("empty");
        assert_eq!(out.0.count, 0);
        assert_eq!(ran.load(Ordering::SeqCst), 0);
    }
}
