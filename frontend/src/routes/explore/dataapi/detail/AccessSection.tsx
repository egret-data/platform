import { useCallback, useState } from "react";
import { Link } from "react-router-dom";
import { useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { SettingsNote } from "@/routes/explore/settings/SettingsNote";
import {
  getDataApiGetOpenapiQueryKey,
  getDataApiListEndpointGrantsQueryKey,
  useDataApiGetOpenapi,
  useDataApiGrantEndpointInvoke,
  useDataApiListEndpointGrants,
  useDataApiListServiceAccounts,
  useDataApiRevokeEndpointInvoke,
} from "@/api/generated/data-api/data-api";
import type { DataApiEndpointGrantDto, DataApiServiceAccountDto } from "@/api/generated/model";
import { useEndpointDetail } from "../EndpointDetail";

/**
 * Just enough of the generated OpenAPI document to answer one question. The
 * generated hook types the 200 body as `unknown` (it is assembled server-side
 * as `serde_json::Value`, not through utoipa — see `openapi_gen.rs`), so this
 * is a local, minimal shape rather than a generated model.
 */
interface OpenApiInvokeProbe {
  paths?: Record<string, unknown>;
}

/**
 * Condition 1 for one service account, with the two states that must never
 * collapse into a single ✓.
 *
 * `granted` is a row in `access_grants` scoped to THIS endpoint — revocable
 * here. `inherited` is the same principal reaching invoke some other way
 * (today: a workspace membership whose role carries `data_api_endpoint:invoke`,
 * applied to every object in the workspace by the Cedar cascade) — there is no
 * object-scoped row, so revoking here would delete nothing and change nothing.
 */
type InvokeStatus =
  | { kind: "granted"; alsoInherited: boolean }
  | { kind: "inherited" }
  | { kind: "none" }
  /** The effective-invoke question was not answered. NEVER rendered as "no". */
  | { kind: "unchecked"; reason: "not-live" | "loading" | "error" };

/**
 * `Access` — one row per service account, two conditions each (spec §6.2).
 *
 * # Where "can this account invoke?" is read from, and why
 *
 * `/grants` lists ONLY the object-scoped rows. It cannot see a principal that
 * reaches invoke through the Cedar workspace→object cascade, and `invoke.rs`'s
 * module comment names this section as the third caller that has to agree with
 * the serving path: *"the only way they can is by being the same expression"*.
 * Spec §6.2 puts it as a requirement — condition 1 is "read truthfully through
 * Cedar (not a raw `access_grants` scan)".
 *
 * So the effective answer comes from the backend, not from a rule re-derived
 * here: `GET …/data-api/openapi?service_account_id=&endpoint_id=` runs
 * `can_invoke` — literally the expression the serving path evaluates — for
 * that account and emits the path only if it says yes. A non-empty `paths` is
 * therefore "this account may invoke this endpoint", cascade included.
 *
 * The alternative considered and rejected was reading `useWorkspaceMembers` and
 * treating `ws_admin` as invoke. That is one query for the whole page instead of
 * one per account, but it copies a rule the backend owns (migration 0124) into
 * the frontend — and copies it WRONG the moment an operator removes the seed
 * from `ws_admin`, adds `data_api_endpoint:invoke` to a custom role, or adds a
 * service account as a member with any other invoke-carrying role, all of which
 * 0124 explicitly contemplates. A checklist that is confidently wrong about
 * access is worse than one that costs an extra request.
 *
 * **The cost accepted:** one request per service account, each of which runs a
 * Cedar authorize server-side. Twenty service accounts is twenty requests on
 * opening this section. They are react-query-cached and only fire when the
 * endpoint is live (below), but nothing hides that this is N calls.
 *
 * # What is NOT inferred
 *
 * Condition 2 (Ranger) is never guessed. Answering it would mean parsing the
 * endpoint's SQL for its tables and projecting Ranger policy onto them, and a
 * half-right answer is worse than none — so the UI says egret does not know and
 * points at Try-it, which traverses the real serving router and is the only
 * thing that can answer it.
 */
export default function AccessSection() {
  const { ws, id, endpoint } = useEndpointDetail();

  // The generated document describes LIVE endpoints only (`openapi_gen.rs`
  // filters `state = 'live'` with a live version), so on a draft it answers
  // "no paths" for EVERY account. Probing anyway would render a confident
  // "not granted" for a principal that will invoke it happily the moment it is
  // published — a wrong answer dressed as a checked one.
  const isLive = endpoint.state === "live" && !!endpoint.live_version_id;

  const accountsQuery = useDataApiListServiceAccounts(ws);
  const accountsEnvelope = accountsQuery.data;
  const accounts: DataApiServiceAccountDto[] =
    accountsEnvelope?.status === 200 ? accountsEnvelope.data : [];

  const grantsQuery = useDataApiListEndpointGrants(ws, id, { query: { enabled: !!ws && !!id } });
  const grantsEnvelope = grantsQuery.data;
  const grants: DataApiEndpointGrantDto[] = grantsEnvelope?.status === 200 ? grantsEnvelope.data : [];
  const grantedIds = new Set(grants.map((g) => g.service_account_id));

  // A grant whose principal is not in the service-account list — the account was
  // deleted, or the row was written for a principal this workspace no longer
  // lists. Driving the rows purely from `accounts` would make such a grant
  // INVISIBLE while it is still in `access_grants` and still consulted by
  // Cedar: a real invoke right nobody on this page can see, let alone revoke.
  // `list_endpoint_grants` already LEFT JOINs for exactly this case (it returns
  // `service_account_name: null`), so the shape is expected, not defensive.
  const accountIds = new Set(accounts.map((a) => a.id));
  const orphanGrants = grants.filter((g) => !accountIds.has(g.service_account_id));

  const qc = useQueryClient();
  // Both halves of condition 1 move when a grant does: the `/grants` list, and
  // every account's generated document (the prefix key, without params, covers
  // all of them). Skipping the second would leave a just-revoked row claiming
  // "also reaches invoke through the workspace" from a cached probe.
  const invalidate = useCallback(() => {
    qc.invalidateQueries({ queryKey: getDataApiListEndpointGrantsQueryKey(ws, id) });
    qc.invalidateQueries({ queryKey: getDataApiGetOpenapiQueryKey(ws) });
  }, [qc, ws, id]);

  const grantMutation = useDataApiGrantEndpointInvoke();
  const revokeMutation = useDataApiRevokeEndpointInvoke();
  // Which account's button is in flight — not `mutation.isPending`, which is
  // shared and would spin every row's button at once.
  const [busyId, setBusyId] = useState<string | null>(null);

  const onGrant = useCallback(
    async (accountId: string) => {
      setBusyId(accountId);
      try {
        await grantMutation.mutateAsync({ workspace: ws, id, data: { service_account_id: accountId } });
        invalidate();
        toast.success("Granted invoke");
      } catch (e) {
        toast.error(String(e));
      } finally {
        setBusyId(null);
      }
    },
    [grantMutation, ws, id, invalidate],
  );

  const onRevoke = useCallback(
    async (accountId: string) => {
      setBusyId(accountId);
      try {
        await revokeMutation.mutateAsync({ workspace: ws, id, serviceAccountId: accountId });
        invalidate();
        toast.success("Revoked invoke");
      } catch (e) {
        toast.error(String(e));
      } finally {
        setBusyId(null);
      }
    },
    [revokeMutation, ws, id, invalidate],
  );

  return (
    <div className="space-y-6">
      <div>
        <h2 className="text-lg font-semibold">Access</h2>
        <p className="text-sm text-muted-foreground">
          What each service account can actually do with this endpoint — the invoke right egret owns, and
          the data access it does not.
        </p>
      </div>

      <SettingsNote>
        Publishing an endpoint does not grant data access. A call runs as the service account against the
        workspace connection, under whatever the engine enforces — so an admin must also grant that
        account on the underlying tables in Ranger. Invoke and data access are two separate permissions
        and only the first one is on this page.
      </SettingsNote>

      <Card>
        <CardHeader>
          <CardTitle className="text-base">Service accounts</CardTitle>
        </CardHeader>
        <CardContent className="space-y-3">
          {!isLive && (
            <p className="text-sm text-muted-foreground">
              This endpoint is not live, so nothing is being served yet and egret cannot check
              workspace-inherited invoke rights against it. Grants recorded here are real and take effect
              when it is published.
            </p>
          )}

          {accountsQuery.isLoading || grantsQuery.isLoading ? (
            <p className="text-sm text-muted-foreground">Loading…</p>
          ) : accountsQuery.isError || grantsQuery.isError ? (
            <Alert variant="destructive">
              <AlertDescription>
                {(accountsQuery.error as unknown as Error)?.message ??
                  (grantsQuery.error as unknown as Error)?.message ??
                  "Could not load who can call this endpoint."}
              </AlertDescription>
            </Alert>
          ) : accounts.length === 0 && orphanGrants.length === 0 ? (
            <p className="text-sm text-muted-foreground">
              This workspace has no service accounts yet. A consumer needs one — with an OAuth2 client —
              before it can call anything.
            </p>
          ) : (
            <ul className="divide-y divide-border">
              {accounts.map((account) => (
                <AccessRow
                  key={account.id}
                  ws={ws}
                  endpointId={id}
                  account={account}
                  grantedHere={grantedIds.has(account.id)}
                  probeEnabled={isLive}
                  busy={busyId === account.id}
                  onGrant={onGrant}
                  onRevoke={onRevoke}
                />
              ))}
              {orphanGrants.map((g) => (
                <AccessRow
                  key={g.service_account_id}
                  ws={ws}
                  endpointId={id}
                  account={{
                    id: g.service_account_id,
                    name: g.service_account_name ?? g.service_account_id,
                    description: "",
                    email: "",
                    created_at: g.created_at,
                  }}
                  grantedHere
                  orphaned
                  // Nothing to probe: the account is not one this workspace
                  // lists, so the document route answers 404 for it by design.
                  probeEnabled={false}
                  busy={busyId === g.service_account_id}
                  onGrant={onGrant}
                  onRevoke={onRevoke}
                />
              ))}
            </ul>
          )}
        </CardContent>
      </Card>
    </div>
  );
}

/**
 * One service account: its two conditions, and the one action (if any) that
 * changes something here.
 *
 * The effective-invoke probe lives in the ROW rather than the section because
 * it is one request per account and hooks cannot be called in a loop. It runs
 * even for an account that is already granted here: an account can hold both an
 * object-scoped grant AND workspace-inherited invoke, and in that case Revoke
 * removes the row without removing the access — the same "clicked revoke,
 * nothing happened" surprise the two-state split exists to prevent, just one
 * step later.
 */
function AccessRow({
  ws,
  endpointId,
  account,
  grantedHere,
  probeEnabled,
  orphaned = false,
  busy,
  onGrant,
  onRevoke,
}: {
  ws: string;
  endpointId: string;
  account: DataApiServiceAccountDto;
  grantedHere: boolean;
  probeEnabled: boolean;
  /** The grant names a principal this workspace no longer lists as an account. */
  orphaned?: boolean;
  busy: boolean;
  onGrant: (accountId: string) => void;
  onRevoke: (accountId: string) => void;
}) {
  const probe = useDataApiGetOpenapi(
    ws,
    { service_account_id: account.id, endpoint_id: endpointId },
    { query: { enabled: probeEnabled && !!ws && !!account.id } },
  );
  const probeEnvelope = probe.data;
  const doc = probeEnvelope?.status === 200 ? (probeEnvelope.data as OpenApiInvokeProbe) : undefined;
  // The document names this endpoint's path exactly when `can_invoke` said yes
  // for this account — the same expression the serving path evaluates.
  const effective = doc ? Object.keys(doc.paths ?? {}).length > 0 : undefined;

  const status: InvokeStatus = !probeEnabled
    ? grantedHere
      ? { kind: "granted", alsoInherited: false }
      : { kind: "unchecked", reason: "not-live" }
    : probe.isError
      ? grantedHere
        ? { kind: "granted", alsoInherited: false }
        : { kind: "unchecked", reason: "error" }
      : effective === undefined
        ? grantedHere
          ? { kind: "granted", alsoInherited: false }
          : { kind: "unchecked", reason: "loading" }
        : grantedHere
          ? { kind: "granted", alsoInherited: effective }
          : effective
            ? { kind: "inherited" }
            : { kind: "none" };

  const membershipHref = `/explore/${ws}/settings/members`;
  // Task 6 (`Try it`) reads `service_account` to preselect the account this row
  // is about, so the author lands on the one question this row could not answer
  // with that account already chosen.
  const tryItHref = `/explore/${ws}/data-api/endpoints/${endpointId}/try?service_account=${encodeURIComponent(account.id)}`;

  return (
    <li className="py-3" data-testid={`access-row-${account.id}`}>
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div className="min-w-0">
          <div className="truncate text-sm font-medium">{account.name}</div>
          <div className="truncate font-mono text-xs text-muted-foreground">
            {orphaned ? "no longer a service account in this workspace" : account.email}
          </div>
        </div>
        <div className="flex items-center gap-2">
          {status.kind === "granted" && (
            <Button
              type="button"
              size="sm"
              variant="outline"
              disabled={busy}
              onClick={() => onRevoke(account.id)}
            >
              {busy ? "Revoking…" : "Revoke"}
            </Button>
          )}
          {/* No action while the check is still in flight: offering Grant to an
              account that turns out to already be granted, one render later, is
              a button that appears and then changes meaning under the cursor. */}
          {(status.kind === "none" ||
            (status.kind === "unchecked" && status.reason !== "loading")) && (
            <Button
              type="button"
              size="sm"
              variant="outline"
              disabled={busy}
              onClick={() => onGrant(account.id)}
            >
              {busy ? "Granting…" : "Grant invoke"}
            </Button>
          )}
        </div>
      </div>

      <dl className="mt-2 space-y-1.5 text-xs">
        <div className="flex flex-wrap items-center gap-2">
          <dt className="w-16 shrink-0 text-muted-foreground">Invoke</dt>
          <dd className="flex flex-wrap items-center gap-2">
            {/* Deliberately NOT `success` (green). A green tick on the invoke
                line reads as "this call will work", and the line directly below
                it is the one egret cannot answer at all — the whole reason this
                section is a two-condition checklist rather than one ✓. The
                wording carries the state; the colour does not overstate it. */}
            {status.kind === "granted" && <Badge variant="secondary">Granted on this endpoint</Badge>}
            {status.kind === "inherited" && <Badge variant="outline">Inherited from the workspace</Badge>}
            {status.kind === "none" && <Badge variant="outline">Not granted</Badge>}
            {status.kind === "unchecked" && <Badge variant="outline">No grant on this endpoint</Badge>}

            {status.kind === "granted" && status.alsoInherited && (
              <span className="text-muted-foreground">
                This account also reaches invoke through the workspace, so revoking here removes the
                grant but not the access.{" "}
                <Link className="underline" to={membershipHref}>
                  Workspace membership
                </Link>
              </span>
            )}
            {status.kind === "inherited" && (
              <span className="text-muted-foreground">
                There is no grant on this endpoint to revoke — this account reaches invoke through its
                workspace role, which applies to every endpoint here. Change it in{" "}
                <Link className="underline" to={membershipHref}>
                  Workspace membership
                </Link>
              </span>
            )}
            {status.kind === "unchecked" && status.reason === "not-live" && (
              <span className="text-muted-foreground">
                Workspace-inherited invoke is unchecked until this endpoint is published.
              </span>
            )}
            {status.kind === "unchecked" && status.reason === "loading" && (
              <span className="text-muted-foreground">Checking workspace-inherited invoke…</span>
            )}
            {status.kind === "unchecked" && status.reason === "error" && (
              <span className="text-muted-foreground">
                egret could not check whether this account inherits invoke from the workspace.
              </span>
            )}
          </dd>
        </div>

        {/* No Ranger line for an orphaned grant: there is no account left to
            run Try-it as, so a "run it and see" prompt would send the reader to
            a screen that cannot answer. The grant itself is still real, and the
            Revoke button above is the useful action. */}
        {!orphaned && (
          <div className="flex flex-wrap items-center gap-2">
            <dt className="w-16 shrink-0 text-muted-foreground">Data</dt>
            <dd>
              <span className="text-muted-foreground">
                egret cannot check whether this account may read the underlying tables — that is
                Ranger&apos;s answer, not egret&apos;s.{" "}
                <Link className="underline" to={tryItHref}>
                  Run Try it
                </Link>{" "}
                to find out for real.
              </span>
            </dd>
          </div>
        )}
      </dl>
    </li>
  );
}
