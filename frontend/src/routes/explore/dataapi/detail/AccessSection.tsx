import { useCallback, useState } from "react";
import type { ReactNode } from "react";
import { ChevronRight } from "lucide-react";
import { Link } from "react-router-dom";
import { useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
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

/** Why the effective-invoke question has no answer yet. */
type UncheckedReason = "not-live" | "loading" | "error";

/**
 * The backend's answer to "may this account invoke this endpoint?", or the
 * reason there isn't one.
 *
 * A three-valued type rather than a `boolean`, because the absence of an answer
 * is a state the UI has to render, not a `false` to fall back to. Reviewer
 * finding (Important #1) was exactly this collapse in the other direction: the
 * `granted` arm used to hard-code `alsoInherited: false` on all three unchecked
 * paths, so a granted row whose probe 500ed rendered the negative claim BY
 * OMISSION — no caveat, live Revoke button — and an admin clicking Revoke on an
 * account that also holds `ws_admin` would delete the grant and change nothing.
 * `can_invoke` propagates PDP errors as errors by design (`invoke.rs`), so that
 * is a real path, not a hypothetical one.
 */
type InheritedCheck = { known: true; inherited: boolean } | { known: false; reason: UncheckedReason };

/**
 * Condition 1 for one service account, with the two states that must never
 * collapse into a single ✓.
 *
 * `granted` is a row in `access_grants` scoped to THIS endpoint — revocable
 * here. `inherited` is the same principal reaching invoke some other way
 * (today: a workspace membership whose role carries `data_api_endpoint:invoke`,
 * applied to every object in the workspace by the Cedar cascade) — there is no
 * object-scoped row, so revoking here would delete nothing and change nothing.
 *
 * `granted` carries the WHOLE [`InheritedCheck`], not a boolean: being granted
 * here says nothing about whether the account also reaches invoke another way,
 * and that second question can be unanswered on a granted row just as easily as
 * on an ungranted one.
 */
type InvokeStatus =
  | { kind: "granted"; also: InheritedCheck }
  | { kind: "inherited" }
  | { kind: "none" }
  /** The effective-invoke question was not answered. NEVER rendered as "no". */
  | { kind: "unchecked"; reason: UncheckedReason };

/**
 * `Access` — one row per service account, two conditions each (spec §6.2).
 *
 * # The shape: a plain `Table`, not cards and not a `ConfigTable`
 *
 * `docs/en/ui-conventions.md` §4. This is LIST-shaped — one row per principal,
 * an unbounded list read down rather than a fixed set of settings read across —
 * so `ConfigTable`'s `Setting | Details | Value` is the wrong frame: there is
 * no Value to edit, only a per-row action.
 *
 * Invoke used to be a `<dt>` inside each row's `<dl>`, which put the word on
 * every row and left each row's answer at a different vertical offset from the
 * one above it. As a column the label is written once and the answers line up
 * down the page — comparing accounts is the whole point of this screen. The
 * `Card` around the list went for §4.1's reason: on a workspace with no service
 * accounts it framed a single sentence.
 *
 * # Condition 2 is NOT a column, and that is the measurement talking
 *
 * It was one, briefly, and every cell in it held the same 143-character
 * sentence. Measured in the browser at a 786px content box: two rows came to
 * 89px and 293px, ~382px for two accounts — at ten this is not a list anyone
 * can scan, and 288px of that width was spent saying one thing N times.
 *
 * The sentence is a statement about egret and Ranger **in general**, not about
 * the account in the row — a section-level fact wearing a per-row costume. So
 * it is section-level copy now, above the table, and it says why the column is
 * absent. The genuinely per-row part of condition 2 was never the sentence: it
 * is the `Try it` link, which carries `?service_account=<id>` so the trial runs
 * as THAT account. That is an action, and it moved to the actions column where
 * the other per-row action already lives.
 *
 * The old guard asserted the sentence was byte-identical across two rows with
 * opposite invoke status. Its replacement is strictly stronger and much simpler
 * to satisfy honestly: the sentence must appear **exactly once** in the whole
 * section. Rendering it outside the row loop makes "varies with invoke status"
 * unrepresentable rather than merely tested.
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
  const { ws, id, endpoint, trashed } = useEndpointDetail();

  // The generated document describes LIVE endpoints only (`openapi_gen.rs`
  // filters `state = 'live'` with a live version), so on a draft it answers
  // "no paths" for EVERY account. Probing anyway would render a confident
  // "not granted" for a principal that will invoke it happily the moment it is
  // published — a wrong answer dressed as a checked one.
  const isLive = endpoint.state === "live" && !!endpoint.live_version_id;
  // `state` is one of draft/live/disabled (migration 0121's CHECK), and the two
  // non-live values got here by opposite routes: a draft has never been
  // published, a disabled endpoint WAS published and then switched off. Telling
  // the second one "publish it" is false, so the sentence branches even though
  // the access claim above it does not.
  const wasDisabled = endpoint.state === "disabled";

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

  const loading = accountsQuery.isLoading || grantsQuery.isLoading;
  const failed = accountsQuery.isError || grantsQuery.isError;
  const empty = accounts.length === 0 && orphanGrants.length === 0;

  return (
    // Capped here, not by the route (§5.1) — this tab is a note, a short account
    // list and two conditions per row. Uncapped, the explanatory note above sets
    // as one very long line and each row's action ends up a monitor's width from
    // the account it acts on. `max-w-5xl` is still well past the `max-w-3xl` the
    // whole route used to sit under.
    <div className="max-w-5xl space-y-6">
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

      <div className="space-y-2">
        <h3 className="text-sm font-medium">Service accounts</h3>

        {/* Above the table, not in it: this is a statement about the whole
            section — every row's Invoke answer is qualified by it — and it is
            the only place that distinguishes a draft from a disabled endpoint.
            The rows deliberately stay state-neutral (see `AccessRow`). */}
        {!isLive && (
          <p className="text-sm text-muted-foreground">
            {wasDisabled
              ? "This endpoint is disabled, so it is serving nothing and egret cannot check workspace-inherited invoke rights against it. Grants recorded here are real and apply again when it is re-enabled."
              : "This endpoint is not live, so nothing is being served yet and egret cannot check workspace-inherited invoke rights against it. Grants recorded here are real and take effect when it is published."}
          </p>
        )}

        {/* The reason the two mutations below are dead, said ONCE — not once per
            row, because it is a fact about the ENDPOINT and not about any
            account (§4.3: a sentence true of every row is section-level copy).
            The shell's `<fieldset disabled={trashed}>` already greys the
            buttons; what it cannot do is say why, and what the shell's BANNER
            says is over-broad for this section in particular — "everything
            below is read-only" is false of the `Try it` link, which is
            deliberately still live here. Only the section knows which of its
            own controls are dead and which are not, which is why the
            explanation belongs here rather than one level up.

            The second sentence is the part that is genuinely new information:
            an admin deciding whether to restore is asking exactly "could this
            account read the data", and a trial is still the only thing that
            answers it. */}
        {trashed && (
          <p className="text-sm text-muted-foreground">
            This endpoint is in the trash, so invoke cannot be granted or revoked here — restore it
            first. Grants already recorded are untouched and still apply. Try it still runs, so an
            account&apos;s data access can be checked before deciding whether to restore.
          </p>
        )}

        {/* Condition 2, said ONCE. It was a column, and every cell in it held
            this same sentence — 288px of width to repeat a fact about egret on
            every row (see the module doc for the measurement). It also explains
            the absent column, so a reader who expects a second condition here
            is told where it went rather than left to assume it was dropped. */}
        <p className="text-sm text-muted-foreground">
          egret cannot check whether these accounts may read the underlying tables — that is
          Ranger&apos;s answer, not egret&apos;s, and it is the same unknown for every account here,
          which is why it is not a column. Run <strong className="font-medium">Try it</strong> on a row
          to find out for real.
        </p>

        {/* `table-fixed` so the Invoke column keeps a fixed measure instead of
            being resized by whichever row happens to hold the longest caveat
            (§5.2's rule, applied to a plain table). Invoke is the unsized
            column — it is the one this section is about. */}
        <Table className="table-fixed">
          <TableHeader>
            <TableRow>
              <TableHead className="w-48">Service account</TableHead>
              <TableHead>Invoke</TableHead>
              {/* Named for a screen reader, invisible on screen: a visible
                  heading over a column of one small button reads as louder than
                  what it labels. */}
              <TableHead className="w-36 text-right">
                <span className="sr-only">Actions</span>
              </TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {/* Three states as spanning rows rather than a branch that replaces
                the table, so the header survives and the column is named even
                before there is a row to answer it for.
                `whitespace-normal` because the shared `TableCell` is
                `whitespace-nowrap` and these are sentences. */}
            {loading ? (
              <TableRow className="hover:bg-transparent">
                <TableCell colSpan={3} className="whitespace-normal text-sm text-muted-foreground">
                  Loading…
                </TableCell>
              </TableRow>
            ) : failed ? (
              // Checked BEFORE the empty branch (§2.1): react-query clears
              // `isLoading` on any settle, so a failed fetch falling into the
              // empty branch would tell an admin their workspace has no service
              // accounts and send them off to create one that already exists.
              <TableRow className="hover:bg-transparent">
                <TableCell colSpan={3} className="whitespace-normal">
                  <Alert variant="destructive">
                    <AlertDescription>
                      {(accountsQuery.error as unknown as Error)?.message ??
                        (grantsQuery.error as unknown as Error)?.message ??
                        "Could not load who can call this endpoint."}
                    </AlertDescription>
                  </Alert>
                </TableCell>
              </TableRow>
            ) : empty ? (
              <TableRow className="hover:bg-transparent">
                <TableCell colSpan={3} className="whitespace-normal text-sm text-muted-foreground">
                  This workspace has no service accounts yet. A consumer needs one — with an OAuth2
                  client — before it can call anything.
                </TableCell>
              </TableRow>
            ) : (
              <>
                {accounts.map((account) => (
                  <AccessRow
                    key={account.id}
                    ws={ws}
                    endpointId={id}
                    account={account}
                    grantedHere={grantedIds.has(account.id)}
                    probeEnabled={isLive}
                    trashed={trashed}
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
                    trashed={trashed}
                    busy={busyId === g.service_account_id}
                    onGrant={onGrant}
                    onRevoke={onRevoke}
                  />
                ))}
              </>
            )}
          </TableBody>
        </Table>
      </div>
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
  trashed,
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
  /**
   * The endpoint is in the trash. Both mutations below are refused for one,
   * so both say so themselves rather than leaving it to the shell's
   * `fieldset` — which greys them identically to a pending mutation and gives
   * the reader no way to tell the two apart. The section states the reason
   * once, above the table.
   */
  trashed: boolean;
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

  // Computed ONCE, ahead of `grantedHere`, and consumed by both arms below.
  // The previous shape asked "granted?" first and re-derived the check inside
  // each branch, which is what let three of those branches quietly answer
  // `false` for a question nobody had asked the PDP (reviewer Important #1).
  // With the check hoisted there is no branch left that can drop it.
  const check: InheritedCheck = !probeEnabled
    ? { known: false, reason: "not-live" }
    : probe.isError
      ? { known: false, reason: "error" }
      : effective === undefined
        ? { known: false, reason: "loading" }
        : { known: true, inherited: effective };

  const status: InvokeStatus = grantedHere
    ? { kind: "granted", also: check }
    : check.known
      ? check.inherited
        ? { kind: "inherited" }
        : { kind: "none" }
      : { kind: "unchecked", reason: check.reason };

  const membershipHref = `/explore/${ws}/settings/members`;
  // `Try it` reads `service_account` to preselect the account this row is
  // about, so the author lands on the one question this row could not answer
  // with that account already chosen.
  const tryItHref = `/explore/${ws}/data-api/endpoints/${endpointId}/try?service_account=${encodeURIComponent(account.id)}`;

  return (
    <TableRow className="align-top" data-testid={`access-row-${account.id}`}>
      <TableCell className="py-3 whitespace-normal">
        <div className="truncate text-sm font-medium">{account.name}</div>
        <div className="truncate font-mono text-xs text-muted-foreground">
          {orphaned ? "no longer a service account in this workspace" : account.email}
        </div>
      </TableCell>

      <TableCell className="py-3 whitespace-normal text-xs">
        <div className="space-y-1.5">
          {/* Deliberately NOT `success` (green). A green tick on the invoke
              line reads as "this call will work", and condition 2 — stated
              above the table — is the one egret cannot answer at all. The
              wording carries the state; the colour does not overstate it. */}
          <div>
            {status.kind === "granted" && <Badge variant="secondary">Granted on this endpoint</Badge>}
            {status.kind === "inherited" && <Badge variant="outline">Inherited from the workspace</Badge>}
            {status.kind === "none" && <Badge variant="outline">Not granted</Badge>}
            {status.kind === "unchecked" && <Badge variant="outline">No grant on this endpoint</Badge>}
          </div>

          {status.kind === "granted" && status.also.known && status.also.inherited && (
            // The consequence is the summary; the reason is behind it. Measured
            // at a 786px content box this sentence alone made the row 293px.
            <Caveat summary="Revoking will not remove access">
              This account also reaches invoke through the workspace, so revoking here removes the grant
              but not the access.{" "}
              <Link className="underline" to={membershipHref}>
                Workspace membership
              </Link>
            </Caveat>
          )}
          {/* The caveat that used to be missing. A granted row whose check did
              not land must not present Revoke as if it were known to be the
              only thing holding the access up.

              Skipped for an orphaned grant: there the probe was never going to
              run (the principal is not a listed account, so the document route
              404s for it by design), and "revoking may not remove access" would
              imply a workspace membership for a principal this workspace does
              not list. That row carries its own explanation instead.

              The `loading` arm is NOT a `Caveat`: it is transient, six words
              long, and has no explanation to hold — collapsing it would cost a
              click to reveal nothing. §11.2's rule, applied per caveat rather
              than per section: collapse a thing because it is big. */}
          {status.kind === "granted" && !status.also.known && !orphaned && (
            status.also.reason === "loading" ? (
              <span className="block text-muted-foreground">Checking workspace-inherited invoke…</span>
            ) : (
              <Caveat summary="Revoking may not remove access">
                {status.also.reason === "error"
                  ? "egret could not check whether this account also reaches invoke through the workspace, so revoking here may not remove access."
                  : "egret cannot check whether this account also reaches invoke through the workspace until this endpoint is live, so revoking here may not remove access once it is."}
              </Caveat>
            )
          )}
          {status.kind === "inherited" && (
            // The row offers no Revoke at all, so the headline is what the
            // reader is looking for when they wonder why.
            <Caveat summary="Nothing here to revoke">
              There is no grant on this endpoint to revoke — this account reaches invoke through its
              workspace role, which applies to every endpoint here. Change it in{" "}
              <Link className="underline" to={membershipHref}>
                Workspace membership
              </Link>
            </Caveat>
          )}
          {status.kind === "unchecked" && status.reason === "not-live" && (
            <span className="block text-muted-foreground">
              {/* State-neutral on purpose: the row does not know whether the
                  endpoint is a draft or a disabled one, and "until it is
                  published" is false for the second. The section-level
                  sentence above the table carries that distinction.

                  One line, and it IS the whole fact — nothing to put behind a
                  summary. */}
              Workspace-inherited invoke is unchecked while this endpoint is not live.
            </span>
          )}
          {status.kind === "unchecked" && status.reason === "loading" && (
            <span className="block text-muted-foreground">Checking workspace-inherited invoke…</span>
          )}
          {status.kind === "unchecked" && status.reason === "error" && (
            <span className="block text-muted-foreground">
              egret could not check whether this account inherits invoke from the workspace.
            </span>
          )}
        </div>
      </TableCell>

      <TableCell className="py-3 text-right">
        <div className="flex flex-col items-end gap-1.5">
          {status.kind === "granted" && (
            <Button
              type="button"
              size="sm"
              variant="outline"
              disabled={busy || trashed}
              onClick={() => onRevoke(account.id)}
            >
              {busy ? "Revoking…" : "Revoke"}
            </Button>
          )}
          {/* No action while the check is still in flight: offering Grant to an
              account that turns out to already be granted, one render later, is
              a button that appears and then changes meaning under the cursor. */}
          {(status.kind === "none" || (status.kind === "unchecked" && status.reason !== "loading")) && (
            <Button
              type="button"
              size="sm"
              variant="outline"
              disabled={busy || trashed}
              onClick={() => onGrant(account.id)}
            >
              {busy ? "Granting…" : "Grant invoke"}
            </Button>
          )}
          {/* Condition 2's only per-row part: `?service_account=<id>` preselects
              THIS account, so the trial answers the Ranger question for it
              rather than for whichever account the picker happened to hold.
              `TryIt.tsx` reads that exact key and says so on its side too.

              A `<Link>`, not a `<Button>`: it navigates, and it must stay
              reachable on a TRASHED endpoint — the shell's
              `<fieldset disabled={trashed}>` would grey a button, and a trial
              is the only way to answer condition 2 for an endpoint an admin is
              deciding whether to restore.

              Not rendered for an orphaned grant: there is no account left to
              run a trial as, so the link would send the reader to a screen that
              cannot answer. That row's Revoke is the useful action. */}
          {!orphaned && (
            <Link className="text-xs underline text-muted-foreground" to={tryItHref}>
              Try it
            </Link>
          )}
        </div>
      </TableCell>
    </TableRow>
  );
}

/**
 * A row-scoped caveat: **the consequence in the summary, the explanation behind
 * it.**
 *
 * The browser pass measured the cost of not doing this. At a 786px content box
 * one granted row carrying 155 characters of caveat was 293px tall against a
 * 89px neighbour — the rows disagreed visually for a reason that was not about
 * importance, and ten accounts would not have been a list anyone could scan.
 *
 * **Why this is not the "drawer closed by default" mistake.** `TryIt`'s module
 * doc records the opposite ruling for its own warning copy: a reader who has to
 * open something to learn a call is real has already pressed the button. That
 * rule holds here too, and the split is what satisfies it — the load-bearing
 * fact ("Revoking will not remove access") is the SUMMARY and is on screen
 * without any interaction. What is behind the disclosure is the *why* and the
 * *where to go instead*, which is what a reader goes looking for once the
 * headline has stopped them. Collapsing the headline itself would be the
 * mistake; collapsing the paragraph under it is the point.
 *
 * **Native `<details>`/`<summary>`, never `ui/collapsible`** (§11.2). The shell
 * wraps every section in `<fieldset disabled={trashed}>`, which disables every
 * BUTTON under it — a Radix trigger included — so on a trashed endpoint a Radix
 * collapsible could not be opened and the caveat would be unreachable. A
 * `<summary>` is not a form control and the fieldset does not touch it.
 *
 * It is also not a hover affordance: `<summary>` is focusable and toggles on
 * Enter/Space, so keyboard and touch reach it the same way a pointer does. A
 * `title` tooltip would have been cheaper and would have hidden the caveat from
 * both.
 */
function Caveat({ summary, children }: { summary: string; children: ReactNode }) {
  return (
    <details className="group">
      {/* `list-none` plus an explicit chevron rather than the UA marker, which
          renders at a different size and offset per browser and cannot be
          aligned with the badge above it. Same idiom as `Query.tsx`'s strip. */}
      <summary className="flex cursor-pointer list-none items-start gap-1 text-muted-foreground hover:text-foreground">
        <ChevronRight
          className="mt-px h-3 w-3 shrink-0 transition-transform group-open:rotate-90"
          aria-hidden="true"
        />
        <span className="underline decoration-dotted underline-offset-2">{summary}</span>
      </summary>
      <div className="mt-1 pl-4 text-muted-foreground">{children}</div>
    </details>
  );
}
