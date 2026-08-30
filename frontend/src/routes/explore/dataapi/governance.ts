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
export function runsPerPrincipal(db: Pick<DatabaseConn, "delegated" | "options">): boolean {
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
 * A connection that has not loaded yet answers `false`. An author is not warned
 * about a database the page does not know about, which is right for a hint and
 * would be wrong for a gate.
 */
export function governedWithoutPerPrincipal(
  db: Pick<DatabaseConn, "delegated" | "options" | "governed"> | undefined,
): boolean {
  return !!db?.governed && !runsPerPrincipal(db);
}

/**
 * The one sentence every surface shows for it, so an author reads the same
 * explanation on the create page, in the Query tab's picker, and — with the
 * server's own wording — in the refusal itself.
 */
export const GOVERNED_BLOCKED_MESSAGE =
  "This database is governed: its row filters and column masks are applied per user at the engine. " +
  "A query here would run on the shared service credential, so every consumer would read what that " +
  "account can read. Publishing will be refused until the connection has per-user delegation (or " +
  "impersonation) turned on.";
