import type { DatabaseConn } from "@/api/databases";

/**
 * The two connection flags that decide whether the ENGINE runs a query as the
 * caller — the frontend's reading of `lifecycle::runs_per_principal`.
 *
 * OR, not AND, and both halves are load-bearing. `options.impersonate` makes
 * Trino send `X-Trino-User: <caller>` and StarRocks issue
 * `EXECUTE AS <caller> WITH NO REVERT` **independently of `delegated`**, so a
 * connection can run per-principal at the engine while `delegated` reads false.
 * Treating `delegated` as the whole condition was a real cross-identity leak on
 * the server side once; the copy of the rule that lives here must not
 * re-introduce it in the advice it gives an author.
 */
export function runsPerPrincipal(
  db: Pick<DatabaseConn, "delegated"> & Partial<Pick<DatabaseConn, "options">>,
): boolean {
  return db.delegated || db.options?.impersonate === true;
}

/**
 * Would publishing an endpoint onto this connection be refused?
 *
 * The server is the authority — `check_governed_connection` refuses the publish
 * and `governance_allows_serving` refuses the request — and this is deliberately
 * the same predicate rather than an approximation of it. Its job is to tell the
 * author BEFORE they hit Publish, not to decide anything: nothing here is
 * allowed to gate a request, because a client-side rule that gates is a rule
 * that can be skipped.
 *
 * A connection that has not loaded yet answers `false`, and so does the
 * redacted row a non-admin lists (no `governed`, no `options`). An author is not
 * warned about a database the page does not know about, which is right for a
 * hint and would be wrong for a gate.
 */
export function governedWithoutPerPrincipal(
  db: (Pick<DatabaseConn, "delegated"> & Partial<Pick<DatabaseConn, "options" | "governed">>) | undefined,
): boolean {
  return !!db?.governed && !runsPerPrincipal(db);
}

/**
 * The one sentence every surface shows for it, so an author reads the same
 * explanation on the create page, in the Query tab's picker, and — with the
 * server's own wording — in the refusal itself.
 *
 * It names **Run as well as Publish**, and that is a correction rather than an
 * addition. The first version of this string said only that publishing would be
 * refused, on the belief that a preview "runs as the author, under the author's
 * own engine policy". It does not: `preview::run` executes through the same
 * `execute_as` path as a served request, so on a connection that is neither
 * delegated nor impersonating the ENGINE sees the service account and the author
 * reads unmasked rows. The server now refuses the preview too — see
 * `preview::run`'s doc comment — and an author who was told Run still worked
 * would find out otherwise on the first press.
 */
export const GOVERNED_BLOCKED_MESSAGE =
  "This connection is governed: its row filters and column masks are applied per user at the engine. " +
  "A query here would run on the shared service credential, so every consumer would read what that " +
  "account can read. Running and publishing are both refused until the connection has per-user " +
  "delegation (or impersonation) turned on.";
