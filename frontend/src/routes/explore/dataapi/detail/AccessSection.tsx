import { useCallback, useState } from "react";
import { Link } from "react-router-dom";
import { useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { Caveat } from "./Caveat";
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
import { LeaveGuardLink, useEndpointDetail } from "../EndpointDetail";

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
 * `granted` carries whether the account ALSO reaches invoke through the
 * workspace, and that answer comes from the grant list (`inherits_invoke`),
 * which the server asks of the PDP at WORKSPACE scope where this grant cannot
 * count. It used to come from the per-account probe below, which asks "may it
 * invoke THIS endpoint?" — a question the grant itself answers yes — so every
 * granted row carried "Revoking will not remove access", including accounts
 * with no workspace role, for which it was false (KI-180).
 */
type InvokeStatus =
  | { kind: "granted"; inherits: boolean }
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
 * it is section-level copy now, and it says why the column is absent. It sits
 * in the Invoke column's HEADER, behind that column's `Caveat`: the audit found
 * it and the page-level note saying overlapping things about invoke-right
 * versus data-access, and one merged block attached to the column beats two
 * standing ones above it. The genuinely per-row part of condition 2 was never the sentence: it
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
 * That probe is asked only for an account with NO grant here. For a granted
 * account it says yes because of the grant, so it cannot tell whether revoking
 * would remove the access; the grant list answers that instead
 * (`inherits_invoke`, asked of the PDP at workspace scope — KI-180).
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
 * **The cost accepted:** one request per service account without a grant here,
 * each of which runs a Cedar authorize server-side. Twenty such accounts is twenty requests on
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
  const grantsById = new Map(grants.map((g) => [g.service_account_id, g]));

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
  // all of them). Skipping the second would leave a just-revoked row reading
  // its invoke status from a probe cached while it still held the grant.
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
    // Uncapped, like every other tab on this route (§5.1, as reversed). This
    // used to carry `max-w-5xl` on the argument that a note, a short account
    // list and two conditions per row read worse stretched across a wide
    // monitor. That argument was not wrong about THIS tab; it was wrong about
    // the tab set. Measured at 1600×1000 in a 1296px content box, Access
    // stopped at 1024 while Query, Config and Response took all 1296 — so a
    // reader clicking along the nav watched the content edge jump 272px, three
    // times, for reasons no single tab could explain. One measure across the
    // six beats a per-section optimum.
    //
    // What the removed cap was buying, checked rather than assumed (§5.2 —
    // removing a cap is only half a change, because the table below is
    // `table-fixed`): with nothing else changed, all 272 new pixels landed on
    // `Invoke`, the one unsized column — while `Actions` stayed `w-36` and
    // right-aligned, so a row's Revoke button sat 272px further from the
    // account it acts on than it had. That was recorded here as accepted and
    // not fixed. **It is fixed now**, and the two halves of the fix are the
    // column widths below and the row's action group at the bottom of this
    // file; the 272px are spent rather than absorbed.
    //
    // Measured in real Chromium at 1600×1000 with seven accounts, `Invoke`
    // never wanted them: its widest row content lays out at 183px, against the
    // 960px it was being handed. The columns that did want them are the two
    // that were pinned — the identity, which was cutting every address, and
    // the actions, which was too narrow to put two controls on one line.
    <div className="space-y-6">
      <div>
        <h2 className="text-lg font-semibold">Access</h2>
        <p className="max-w-prose text-sm text-muted-foreground">
          What each service account can actually do with this endpoint — the invoke right egret owns, and
          the data access it does not.
        </p>
      </div>

      <div className="space-y-2">
        <h3 className="text-sm font-medium">Service accounts</h3>

        {/* Above the table, not in it: this is a statement about the whole
            section — every row's Invoke answer is qualified by it — and it is
            the only place that distinguishes a draft from a disabled endpoint.
            The rows deliberately stay state-neutral (see `AccessRow`). */}
        {!isLive && (
          <p className="max-w-prose text-sm text-muted-foreground">
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
          <p className="max-w-prose text-sm text-muted-foreground">
            This endpoint is in the trash, so invoke cannot be granted or revoked here — restore it
            first. Grants already recorded are untouched and still apply. Try it still runs, so an
            account&apos;s data access can be checked before deciding whether to restore.
          </p>
        )}

        {/* `table-fixed` so the Invoke column keeps a fixed measure instead of
            being resized by whichever row happens to hold the longest caveat
            (§5.2's rule, applied to a plain table). Invoke is the unsized
            column — it is the one this section is about. */}
        <Table className="table-fixed">
          <TableHeader>
            <TableRow>
              {/* Wide enough to READ an address, above `xl`.

                  `w-48` gave 176px of content. The addresses this column holds
                  are synthesised by the backend as
                  `svc.<slug>@<ws-slug>.service.egret.local`
                  (`crates/egret-dataapi/src/routes.rs`), so 29 characters are
                  spent before the account is even named. Measured in Chromium
                  against the shipped `font-mono text-xs`, seven seeded
                  accounts laid out at 246 / 311 / 419 / 303 / 289 / 296 /
                  311px — all seven cut, on the page whose subject is which
                  identity may invoke. `w-96` gives 368px, which fits the
                  realistic band (a name up to ~48 characters); the outlier
                  keeps `title`, which is what a fallback is for.

                  Gated at `xl`, and the gate is measured rather than a habit
                  (§5.3 makes the same call at `lg` for a different table).
                  `table-fixed` does not shrink a sized column — it scrolls the
                  table — so this column has to leave `Invoke` enough at the
                  width it turns on. Available content box, by viewport:
                  1600→1296, 1280→976, 1024→720. At `xl` the two sized columns
                  take 592 and `Invoke` keeps 384, comfortably over the 183px it
                  wants. At `lg` it would keep 128, and the badges would wrap —
                  a truncated address traded for a wrapped answer. Below the
                  gate this column is `w-48` exactly as it was, and the address
                  is recovered by `title` exactly as it was. */}
              <TableHead className="w-48 xl:w-96">Service account</TableHead>
              {/* The one distinction this whole section turns on, beside the
                  column where it bites — and said ONCE.

                  It used to be TWO standing blocks: a page-level `SettingsNote`
                  ("publishing does not grant data access") and a paragraph over
                  the table ("egret cannot check whether these accounts may read
                  the underlying tables"). Measured on the deployed build that
                  was ~550 characters of prose above a table whose first row is
                  one line, and the two overlapped — both were saying that the
                  invoke right on this page is not the data access a call needs.
                  Merged, they are one paragraph; attached to the header, they
                  qualify the answers in the column rather than the page.

                  Nothing was cut, and the collapsed half is chosen the way §4.4
                  chooses it: the CONSEQUENCE is the summary and is on screen
                  with nothing clicked, the reasons are behind it. It also still
                  explains the absent second column, so a reader who expects one
                  is told where it went rather than left to assume it was
                  dropped.

                  A native `<details>` (the shared `Caveat`), never
                  `ui/collapsible`: the shell's `<fieldset disabled={trashed}>`
                  would kill a Radix trigger, and an admin deciding whether to
                  restore a trashed endpoint is exactly the reader who needs
                  this. */}
              {/* `aria-label`, and it is not decoration. A `<th>` names every
                  cell in its column, so without it a screen-reader user hears
                  the whole caveat before each row's answer — the audio form of
                  exactly the defect §4.3 measured when this sentence was a
                  column. The name stays the one word; the caveat is still in
                  the header row for anyone reading it, and `getAllByRole`
                  ("columnheader") still sees `Invoke`. */}
              <TableHead className="whitespace-normal align-top" aria-label="Invoke">
                <div className="space-y-1">
                  <div>Invoke</div>
                  <Caveat summary="Invoke is not data access" className="max-w-prose text-xs font-normal">
                    Publishing an endpoint does not grant data access. A call runs as the service
                    account against the workspace connection, under whatever the engine enforces — so
                    an admin must also grant that account on the underlying tables in Ranger. Invoke
                    and data access are two separate permissions and only the first one is on this
                    page. egret cannot check whether these accounts may read the underlying tables —
                    that is Ranger&apos;s answer, not egret&apos;s, and it is the same unknown for
                    every account here, which is why it is not a column. Run{" "}
                    <strong className="font-medium">Try it</strong> on a row to find out for real.
                  </Caveat>
                </div>
              </TableHead>
              {/* Named for a screen reader, invisible on screen: a visible
                  heading over a column of small buttons reads as louder than
                  what it labels.

                  `w-36` (128px of content) held ONE control, which is why the
                  row's two actions were stacked. Measured, the widest pair
                  (`Try it` + `Grant invoke`, gap included) needs 167px, so
                  `w-52` — 192px of content, and the same width `ConfigTable`'s
                  Value column already uses — clears it by 25px. `w-48` would
                  clear it by 9, which is inside the margin a different UI font
                  could eat.

                  **Unconditional, unlike its neighbour, and that is measured
                  too.** The obvious symmetry (`w-36 xl:w-52`, matching the
                  identity column's gate) is worse at every width between `lg`
                  and `xl`: the group wraps back onto two lines there, and two
                  32px controls stacked are TALLER than the 32px-plus-16px stack
                  this change removed. Seven-row table span, measured:
                  gated 651px at 1024, ungated 451px — against 543px before the
                  change. Widening only the identity column at `xl` and letting
                  this one keep its width everywhere is what makes the row
                  shorter at 900, 1024 and 1100 as well as at 1600. */}
              <TableHead className="w-52 text-right">
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
                    grant={grantsById.get(account.id)}
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
                    grant={g}
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
  grant,
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
  /** This account's grant on this endpoint, if it holds one. */
  grant: DataApiEndpointGrantDto | undefined;
  probeEnabled: boolean;
  /** The grant names a principal this workspace no longer lists as an account. */
  orphaned?: boolean;
  /**
   * The endpoint is in the trash. Both mutations below are refused for one,
   * so both say so themselves rather than leaving it to the shell's
   * `fieldset` — which greys them identically to a pending mutation and gives
   * the reader no way to tell the two apart. The section states the reason
   * once, above the table.
   *
   * (That trashed sentence is one of the two paragraphs still standing above
   * the table; both are conditional STATE explanations that exist only while
   * the thing they describe is true, which is why neither is collapsed.)
   */
  trashed: boolean;
  busy: boolean;
  onGrant: (accountId: string) => void;
  onRevoke: (accountId: string) => void;
}) {
  // Only an account WITHOUT a grant here needs the probe: for a granted one it
  // would answer yes because of the grant, which is the question it cannot
  // separate from the inherited one (KI-180). The grant carries that answer.
  const probe = useDataApiGetOpenapi(
    ws,
    { service_account_id: account.id, endpoint_id: endpointId },
    { query: { enabled: probeEnabled && !grant && !!ws && !!account.id } },
  );
  const probeEnvelope = probe.data;
  const doc = probeEnvelope?.status === 200 ? (probeEnvelope.data as OpenApiInvokeProbe) : undefined;
  // The document names this endpoint's path exactly when `can_invoke` said yes
  // for this account — the same expression the serving path evaluates.
  const effective = doc ? Object.keys(doc.paths ?? {}).length > 0 : undefined;

  // The probe's answer for an account with no grant here. Kept three-valued:
  // an unanswered probe must never render as "Not granted" (reviewer
  // Important #1).
  const check: InheritedCheck = !probeEnabled
    ? { known: false, reason: "not-live" }
    : probe.isError
      ? { known: false, reason: "error" }
      : effective === undefined
        ? { known: false, reason: "loading" }
        : { known: true, inherited: effective };

  const status: InvokeStatus = grant
    ? { kind: "granted", inherits: grant.inherits_invoke }
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

  // One binding for the second line, so the `title` and the visible text cannot
  // drift: an orphaned grant shows an explanation instead of an address, and a
  // tooltip repeating a DIFFERENT string than the one it is attached to would be
  // worse than no tooltip.
  const secondary = orphaned ? "no longer a service account in this workspace" : account.email;

  return (
    <TableRow className="align-top" data-testid={`access-row-${account.id}`}>
      {/* `title` on both lines — and it is now the SECOND half of the fix, not
          the whole of it. It used to be the whole of it: this column was `w-48`
          in a `table-fixed` table, so `truncate` cut at 176px of content no
          matter the viewport, and measured at seven accounts EVERY address was
          cut. Identity is this screen's subject — the whole job here is
          comparing accounts, and an address the reader cannot finish reading is
          one they cannot check — so a pointer-only tooltip was never the right
          answer to it.

          **The reason it was left at that, recorded here, was wrong.** It said
          the widths were deliberately not rebalanced because the Invoke
          neighbour's 688px "is not slack — on a draft or disabled endpoint it
          carries a sentence per row". Measured in Chromium rather than reasoned
          about (§10): the widest thing any Invoke cell wants is 183px, caveat
          rows included, because those sentences are `Caveat` SUMMARIES and the
          bodies are collapsed. The column was ~500px of genuine slack, and the
          argument for keeping it was a §9 recollection. The `<th>` above widens
          this column at `xl`; the addresses fit.

          `title` still earns its place for the outlier — a 33-character account
          name still overruns 368px — but it is a fallback now rather than the
          plan. It remains pointer-only, and the value still cannot be copied
          without selecting it by hand; a copy affordance beside the address is
          the follow-on, still not taken here because it adds a control to every
          row of a table whose height is the thing §4.4 exists to protect. */}
      <TableCell className="py-3 whitespace-normal">
        <div className="truncate text-sm font-medium" title={account.name}>
          {account.name}
        </div>
        <div className="truncate font-mono text-xs text-muted-foreground" title={secondary}>
          {secondary}
        </div>
      </TableCell>

      <TableCell className="py-3 whitespace-normal text-xs">
        <div className="space-y-1.5">
          {/* Deliberately NOT `success` (green). A green tick on the invoke
              line reads as "this call will work", and condition 2 — stated
              once, in this column's header — is the one egret cannot answer at
              all. The
              wording carries the state; the colour does not overstate it. */}
          <div>
            {status.kind === "granted" && <Badge variant="secondary">Granted on this endpoint</Badge>}
            {status.kind === "inherited" && <Badge variant="outline">Inherited from the workspace</Badge>}
            {status.kind === "none" && <Badge variant="outline">Not granted</Badge>}
            {status.kind === "unchecked" && <Badge variant="outline">No grant on this endpoint</Badge>}
          </div>

          {status.kind === "granted" && status.inherits && (
            // The consequence is the summary; the reason is behind it. Measured
            // at a 786px content box this sentence alone made the row 293px.
            <Caveat summary="Revoking will not remove access">
              This account also reaches invoke through the workspace, so revoking here removes the grant
              but not the access.{" "}
              <LeaveGuardLink className="underline" to={membershipHref}>
                Workspace membership
              </LeaveGuardLink>
            </Caveat>
          )}
          {status.kind === "inherited" && (
            // The row offers no Revoke at all, so the headline is what the
            // reader is looking for when they wonder why.
            <Caveat summary="Nothing here to revoke">
              There is no grant on this endpoint to revoke — this account reaches invoke through its
              workspace role, which applies to every endpoint here. Change it in{" "}
              <LeaveGuardLink className="underline" to={membershipHref}>
                Workspace membership
              </LeaveGuardLink>
            </Caveat>
          )}
          {/* NOTHING is rendered for `reason === "not-live"`, and that is the
              §4.3 rule this file already applies to the mutations two blocks
              up: a sentence true of every row is section-level copy in
              disguise.

              It used to say "Workspace-inherited invoke is unchecked while this
              endpoint is not live." once per row. `probeEnabled` IS `isLive`,
              so `reason === "not-live"` holds for exactly the rows on exactly
              the endpoints where the paragraph above the table renders — the
              two conditions are the same expression and can never disagree —
              and that paragraph already says "egret cannot check
              workspace-inherited invoke rights against it", plus the
              draft-versus-disabled distinction the row explicitly cannot make.
              A strict subset, repeated.

              Invisible at the one service account the audit ran against;
              measured in the browser at seven, it was seven identical lines
              directly under the paragraph that had just said it. A `granted`
              row needs no such line: its "Revoking will not remove access"
              answer comes from the grant list, which knows it on any endpoint
              state. */}
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
        {/* One ROW of actions, not a stack.

            Measured at seven accounts before this change: every row was 79px,
            and the height was the stack — a 32px button with a 16px link under
            it. Side by side the pair is one control's height, and the row is
            what the identity in it needs rather than what two lines of controls
            need.

            `flex-wrap`, not a plain `flex-row`: below the `xl` gate on this
            column the two do not fit on one line, and wrapping there is the
            right answer (the old layout, reached by width rather than by
            declaration) instead of a cell that overflows into its neighbour.

            `items-center` so the two share a baseline box; the old
            `items-end` was aligning a full-height button against a bare line of
            text, which only ever looked deliberate because they were stacked. */}
        <div className="flex flex-wrap items-center justify-end gap-2">
          {/* FIRST, and the order is §1.2's rule read into a row: view controls
              before the primary action. The mutation is what changes something
              here, so it takes the outer right edge — the same slot the primary
              action takes in the page header above.

              Condition 2's only per-row part: `?service_account=<id>`
              preselects THIS account, so the trial answers the Ranger question
              for it rather than for whichever account the picker happened to
              hold. `TryIt.tsx` reads that exact key and says so on its side too.

              **Still a `<Link>`, and that is load-bearing.** It navigates, and
              it must stay reachable on a TRASHED endpoint — the shell's
              `<fieldset disabled={trashed}>` reaches form controls only, so a
              real `<button>` here would be greyed exactly when a trial is the
              only thing that can answer "could this account read the data" for
              an endpoint an admin is deciding whether to restore. `asChild`
              keeps the anchor and takes only the paint; no `disabled` is passed,
              so `Button`'s `inertSlot` path (which would add `aria-disabled`
              and `tabIndex={-1}`) never engages.

              **`ghost`, not a second `outline`.** The section's own prose ends
              "Run **Try it** on a row to find out for real", so this cannot go
              on being the smallest and faintest thing in the row — at `text-xs`
              underlined muted it was 12px under a 13px button. Routed through
              the shared `Button` it is a peer in size, hit area and text; as
              `ghost` it carries no border and no fill, so it is quieter than
              the mutation without being smaller than it. (`--accent` appears
              only as that variant's hover/focus paint, which is what it is
              reserved for.)

              Not rendered for an orphaned grant: there is no account left to
              run a trial as, so the link would send the reader to a screen that
              cannot answer. That row's Revoke is the useful action. */}
          {!orphaned && (
            <Button asChild size="sm" variant="ghost">
              <Link to={tryItHref}>Try it</Link>
            </Button>
          )}
          {/* `min-w-28` on both mutations, for two reasons that happen to want
              the same number.

              It keeps `Try it` at the same offset on every row: measured,
              `Revoke` is 78px and `Grant invoke` 104px, so without a floor the
              link to their left jitters 26px down the page — on the screen
              whose entire job is reading down a column and comparing accounts
              (§4.3). And it stops the button resizing under the cursor when its
              own label changes mid-mutation (`Revoke` → `Revoking…`), which
              would move `Try it` at exactly the moment the reader is watching
              that row.

              112px, because it has to clear the widest of the four labels
              (`Grant invoke`, 104px). Not applied to a placeholder on the
              inherited row, which has no mutation at all: §4.3 says do not hold
              a slot empty for symmetry, so that row's single action sits at the
              outer edge. */}
          {status.kind === "granted" && (
            <Button
              type="button"
              size="sm"
              variant="outline"
              className="min-w-28"
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
              className="min-w-28"
              disabled={busy || trashed}
              onClick={() => onGrant(account.id)}
            >
              {busy ? "Granting…" : "Grant invoke"}
            </Button>
          )}
        </div>
      </TableCell>
    </TableRow>
  );
}
