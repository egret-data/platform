import { useEffect, useRef, useState } from "react";
import { Link, useSearchParams } from "react-router-dom";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { CHIP_TONE } from "../chipTone";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { ConfigTable, ConfigTableRow } from "@/components/config/ConfigTable";
import { Caveat } from "./Caveat";
import { useDataApiListServiceAccounts, useDataApiTryEndpoint } from "@/api/generated/data-api/data-api";
import type { DataApiServiceAccountDto, TryRequestParams, TryResponse } from "@/api/generated/model";
import { LeaveGuardLink, useEndpointDetail } from "../EndpointDetail";

/**
 * Best-effort extraction of the refusal's machine-readable code/message from
 * `TryResponseBody`. Typed as an open map (`{[key: string]: unknown}`)
 * because the route hands the serving path's body back VERBATIM — so this file
 * cannot lean on a generated model for it. Handles both the documented
 * top-level `{code, message, request_id}` shape and an `{error: {code, message}}`
 * wrapper (the shape `ErrorBody` uses elsewhere in this API), rather than
 * assuming one and silently showing nothing useful if the body arrives
 * shaped the other way.
 *
 * A SUCCESS body is now `{status, message, data, meta}` (spec §4.4) or, for an
 * endpoint published with `output.envelope: "bare"`, `data`'s value on its own.
 * The wrapper carries a top-level `message`, which is a key this function reads
 * — and it is safe for two independent reasons, both of which have to stay
 * true: on success that field is `null`, not a string, so the `typeof` guard
 * below returns `undefined` for it; and the only caller renders this inside an
 * `!outcomeOk` branch gated on the HTTP STATUS, never on whether a message was
 * found. Do not turn that gate into a truthiness check on `refusal`.
 */
function extractRefusal(body: Record<string, unknown>): { code?: string; message?: string } {
  const err = body.error;
  if (err && typeof err === "object") {
    const e = err as Record<string, unknown>;
    return {
      code: typeof e.code === "string" ? e.code : undefined,
      message: typeof e.message === "string" ? e.message : undefined,
    };
  }
  return {
    code: typeof body.code === "string" ? body.code : undefined,
    message: typeof body.message === "string" ? body.message : typeof err === "string" ? err : undefined,
  };
}

/** Copies a value to the clipboard — the shared "Copy"/"Copied" idiom. */
function CopyButton({ value }: { value: string }) {
  const [copied, setCopied] = useState(false);
  return (
    <Button
      type="button"
      variant="outline"
      size="sm"
      onClick={() => {
        void navigator.clipboard?.writeText(value);
        setCopied(true);
        window.setTimeout(() => setCopied(false), 1500);
      }}
    >
      {copied ? "Copied" : "Copy"}
    </Button>
  );
}

/**
 * `TryIt` — a real trial run against the real serving router (spec §3.2), not
 * a simulation. The `/try` route mints a genuine 60-second service-account
 * token server-side and drives it through the actual serving path, so a
 * green result here is evidence a real consumer's call would work, not a
 * guess. The route itself always answers `200` once the trial ran — the
 * endpoint's OWN status (`200`, a `403 not_granted` refusal, …) lives inside
 * the response body, and is rendered here as a RESULT, never as a page-level
 * failure.
 *
 * **The distinction this section exists to make, and it is NOT simply "whose
 * identity".** A trial authenticates a real 60-second token for the chosen
 * SERVICE ACCOUNT, is authorized by that account's invoke grant, spends the
 * rate limit, reads and writes the cache, and runs the PUBLISHED version.
 * `Query`'s preview does none of those: it authenticates nothing, checks no
 * grant, and runs the SQL on screen. Those four are the difference that always
 * holds, and they are what the copy has to carry.
 *
 * The ENGINE identity is a weaker distinction than this module used to claim.
 * Preview runs under the endpoint's own connection identity, resolved by
 * `egret-query/src/execution/mod.rs`'s `delegation_for`: the author only when
 * that connection is `delegated` or `options.impersonate`, and the same shared
 * service account otherwise — which is every ordinary connection. The live
 * measurement on kind (`who=admin` from a preview, `who=svc-kind-smoke-bot` from
 * a trial, same endpoint and same SQL) was taken on a DELEGATED connection,
 * where the two do differ; on an ordinary one they are the same identity.
 * `PreviewBlock`'s note says this from the other side.
 *
 * **The shape is `Query`'s in everything but the split** (`docs/en/ui-conventions.md`
 * §11): settings in tables, a run shortcut named in the result's header, and
 * the setup and the result on screen together. It replaced eight stacked
 * `Card`s — the most of any section — read down a page that could only scroll,
 * so a re-run meant scroll down to read, scroll back up to change a value,
 * scroll down again.
 *
 * **The split panel is the part that did NOT survive, and it is worth saying
 * why, because it looked like the obvious thing to copy.** This section had a
 * vertical `Group` at a fixed `40%` / `60%`. It has three parts — *Run as*,
 * *Parameter values*, *Result* — and only the last grows without bound; the
 * first two are sized by their CONTENT, and a fraction cannot size those.
 * Measured in Chromium at 1600x1000, the setup pane's `clientHeight` was 230
 * whatever it held, against a `scrollHeight` of 277 / 297 / 342 / 432 / 612 for
 * 0 / 1 / 2 / 4 / 8 declared parameters — so it clipped 47px even on an
 * endpoint declaring NO parameters, and 202px on four, slicing the
 * `Parameter | Type | Value` header row through the middle. The result pane
 * below it was 389px and, before a run, held a header and one collapsed
 * caveat.
 *
 * `Query` keeps its split for a reason that does not transfer: its top half is
 * a Monaco editor, which must be HANDED a height because an auto-height editor
 * is 0px tall. `Query.tsx`'s own comment states the general rule — "the strip
 * can be expanded at any window height, so any fixed split has a window that
 * defeats it" — and that rule is what condemns the split HERE, where the top
 * half is a table with an intrinsic height. Nor could it be repaired in place:
 * `react-resizable-panels` v4 has no content-sizing mode (`defaultSize` takes
 * %, px, em, rem, vh or vw), so "size the top pane to its content" means
 * measuring the content and calling `panelRef.resize()` on mount and on every
 * parameter change — machinery whose only job is to imitate normal flow.
 *
 * So this is one column: the setup at its own height, never scrolling; the
 * result taking what is left, with a floor, and holding the section's only
 * scrolling box. When an endpoint declares more parameters than the section is
 * tall, the whole section overflows the shell's scroll box — which is
 * reachable — rather than a pane clipping inside a page that does not move.
 *
 * **No `<details>` strip, unlike `Query`** — the SETUP is not collapsed. Query
 * collapses its strip because it holds `ParamEditor`, which measured 370px of a
 * 615px section; the setup here is one picker and one short row per parameter,
 * and there is nothing to win.
 *
 * **The PROSE is, and that reverses half of what this comment used to say.**
 * It said the trial warning could not be collapsed at all, on `PreviewBlock`'s
 * rule: a reader who has to go and find "this is a real call" has already
 * pressed the button. That rule is still right and the conclusion did not
 * follow — it argues against hiding the FACT, not against hiding the four
 * sentences of mechanism under it. The audit measured what the old reading
 * cost: ~950 characters of standing prose over one dropdown and one button.
 *
 * So both blocks are `Caveat`s (§4.4): the consequence is the summary and is on
 * screen with nothing clicked — "A trial is a real call, not a dry run", beside
 * Run — and the rate-limit, access-log and cache mechanics are behind it.
 * Nothing was deleted; every sentence is one gesture away, and the gesture
 * works on a trashed endpoint because a `<summary>` is not a form control.
 *
 * The published-version note stays PLAIN TEXT, and that is the same rule
 * pointing the other way: two lines with no explanation to hold would cost a
 * click to reveal nothing. Collapse a caveat because it is big.
 */
export default function TryIt() {
  const { ws, id, endpoint, trashed } = useEndpointDetail();
  const [searchParams] = useSearchParams();
  // Task 5's Access section links here with `?service_account=<id>` to
  // preselect the account a row could not answer for — read that EXACT key.
  // Nothing fails if this key were different; it would just silently not
  // preselect, which is why the contract is written down on both sides.
  const preselected = searchParams.get("service_account") ?? "";

  const accountsQuery = useDataApiListServiceAccounts(ws);
  const accountsEnvelope = accountsQuery.data;
  const accounts: DataApiServiceAccountDto[] =
    accountsEnvelope?.status === 200 ? accountsEnvelope.data : [];

  const [accountId, setAccountId] = useState(preselected);
  const [paramValues, setParamValues] = useState<Record<string, string>>({});

  // The result half, so a run started by ⌘↵ can bring its own outcome on
  // screen. See `onRun`.
  const resultRef = useRef<HTMLElement | null>(null);

  const tryMutation = useDataApiTryEndpoint();
  const [result, setResult] = useState<TryResponse | null>(null);
  const [tooLarge, setTooLarge] = useState(false);
  const [runError, setRunError] = useState<string | null>(null);

  const accessHref = `/explore/${ws}/data-api/endpoints/${id}/access`;
  // Where a service account is CREATED — not Access, which only grants one
  // that already exists. See the empty-picker note below.
  const newAccountHref = `/explore/${ws}/data-api/clients/new`;
  const activityHref = `/explore/${ws}/data-api/activity`;

  const canRun = !!accountId && !trashed && !tryMutation.isPending;

  async function onRun() {
    // ⌘↵ reaches this without passing the button, so the button's own
    // `disabled` is not the guard. Three things ride on this line:
    // a held shortcut cannot fire a second run over the first; an empty
    // `service_account_id` cannot be posted; and a TRASHED endpoint cannot run
    // a trial — the shell's `fieldset disabled` greys the button but a window
    // key handler is not a form control and walks straight past it.
    if (!canRun) return;
    // BEHIND the guard, and that placement is the whole of it: in front, a
    // trashed endpoint or a held shortcut during a pending run yanks the page
    // toward a result that is not going to change.
    //
    // **Why a scroll at all.** This listener is on `window` (§11.1), so ⌘↵
    // fires wherever the page is scrolled — and the section may be taller than
    // the shell's scroll box, which is the price of letting the setup keep its
    // own height. Measured: at 8 declared parameters the result's header starts
    // 5px below the fold and at 16 it is ~355px below it. Without this line a
    // reader at the top of a long parameter list presses ⌘↵, a REAL trial runs
    // — a real serving-path call, a rate-limit token, an `access_log` row — and
    // nothing they can see changes, so they press it again. `sticky` on the
    // header below cannot help here: at scroll-top the result has not been
    // reached at all, so there is nothing to pin.
    //
    // `block: "nearest"` so it is a no-op when the result is already on screen,
    // which is every run started by clicking the button.
    resultRef.current?.scrollIntoView({ block: "nearest" });
    // Clear every prior outcome up front, not just the one this branch is
    // about to set — otherwise a second run that hits `413` (or errors)
    // would leave a PREVIOUS run's success `result` rendered alongside
    // the new outcome, showing two different answers to "what happened" at
    // once.
    setResult(null);
    setTooLarge(false);
    setRunError(null);
    try {
      // A `Vec` per key because a repeated parameter is a LIST (`TryRequestParams`'s
      // own doc) — a blank field is simply omitted rather than sent as `[""]`, so an
      // unfilled optional parameter behaves like it was never supplied.
      const params: TryRequestParams = {};
      for (const p of endpoint.params) {
        const v = paramValues[p.name];
        if (v !== undefined && v !== "") params[p.name] = [v];
      }
      const res = await tryMutation.mutateAsync({
        workspace: ws,
        id,
        data: { service_account_id: accountId, params },
      });
      // The mutator throws on any non-2xx (see `client.ts`/`mutator.ts`) — the
      // `/try` route itself only ever resolves 200 (the trial ran); the
      // endpoint's OWN status lives in `res.data.status`, handled below. Same
      // 2xx-narrowing trick every other section in this feature uses.
      if (res.status !== 200) {
        setRunError("Could not run the trial.");
        return;
      }
      setResult(res.data);
    } catch (e) {
      const err = e as Error & { status?: number };
      if (err.status === 413) {
        // The serving path's own body was too large for THIS route to carry
        // back — not a failure of the trial, which still ran for real.
        setTooLarge(true);
      } else {
        setRunError(err.message || "Could not run the trial.");
      }
    }
  }

  // The shortcut is the whole reason the setup and the result are on screen
  // together, so it must reach the CURRENT account id and the CURRENT parameter
  // values. A `[]`-dep listener closes over the first render's `onRun` and would
  // post yesterday's form; re-subscribing on every keystroke instead would churn
  // a window listener per character. A ref refreshed after every render is what
  // `PreviewBlock` does for the same reason (its `useImperativeHandle` carries
  // no dependency array).
  const runRef = useRef(onRun);
  useEffect(() => {
    runRef.current = onRun;
  });
  useEffect(() => {
    // Window-scoped, like SQL Lab's ⌘K (`components/sqllab/SqlTab.tsx`), and
    // safe to be: this listener exists only while the section is mounted, and
    // no other ⌘↵ handler exists outside a Monaco instance (which stops the
    // event itself). Scoping it to the section's own root instead would make
    // the shortcut dead until something inside had focus — an affordance the
    // header advertises and that does nothing is worse than none.
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key === "Enter") {
        e.preventDefault();
        void runRef.current();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  const outcomeOk = result ? result.status >= 200 && result.status < 300 : false;
  const refusal = result ? extractRefusal(result.body) : undefined;
  const notGranted = !!result && result.status === 403 && refusal?.code === "not_granted";

  return (
    // Uncapped, like every other tab on this route (§5.1, as reversed). This
    // used to carry `max-w-5xl` on §5.4a's complaint — a
    // `Setting | Details | Value` table stretched across a wide monitor puts
    // the picker a screen away from the label that names it. That complaint is
    // real, and it is not what the cap was costing. At 1600×1000 in a 1296px
    // content box, Try it stopped at 1024 while Config, Query and Response took
    // all 1296, so the content edge jumped 272px on a tab change. One measure
    // across the six beats a per-section optimum, and the picker-to-label
    // distance is a COLUMN width, answerable by `valueClassName` (§5.2) without
    // starving the section — which is exactly the resolution §5.4b reached for
    // Response, where the same complaint had earned the same cap.
    //
    // `h-full` is what gives the two halves a height to divide at all: the
    // route wraps its `Outlet` in a `min-h-0 flex-1` fieldset inside an
    // `h-screen` column, so this resolves against a definite box rather than
    // collapsing to zero. It is what lets the RESULT fill the page while the
    // setup keeps its own height; it is not a cap on the section, because the
    // children may exceed it and the fieldset is the scroll box that reveals
    // them.
    <div className="flex h-full flex-col">
      {/* On one line, as `Query`'s is: every section opens with its own `h2`,
          and it is the only thing that names the section for a reader who
          arrived by URL rather than by clicking the nav. The sentence is also
          the first of the three places that say a trial runs as a SERVICE
          ACCOUNT rather than as you — see the module doc. */}
      <div className="flex shrink-0 flex-wrap items-baseline gap-x-3 gap-y-1 pb-2">
        <h2 className="text-lg font-semibold">Try it</h2>
        <p className="max-w-prose text-sm text-muted-foreground">
          Runs a real call through the actual serving router, as a chosen service account — this answers
          what a real consumer would receive, not a simulation.
        </p>
      </div>

      {/* The SETUP, at its own height — never a fraction of the section, and
          never a scroll box of its own. `shrink-0` is the load-bearing half
          and the one that regresses silently: a flex item shrinks by default,
          so without it a long result squeezes this back into the clipped state
          the split produced. A `<section>` with a name because it IS one — the
          two halves are the section's own children, and naming them is what
          lets a reader (and a test) say which half a thing is in without
          counting wrappers. */}
      <section aria-labelledby="try-setup-heading" className="shrink-0">
        <div className="flex items-center gap-2 border-b border-border px-1 pb-2">
          <h3 id="try-setup-heading" className="text-sm font-medium">
            Run as
          </h3>
        </div>
        <div className="space-y-4 px-1 pt-3">
          {/* A `Setting | Details | Value` table rather than a labelled
              control floating in a card — `docs/en/ui-conventions.md` §4.
              The Value column is widened because it carries the picker AND,
              when the fetch fails, the message explaining why it is empty;
              208px is not enough for a sentence.

              `lg:w-96`, and this is the OTHER HALF of the width change §5.1
              records as owed by this section. Removing the section's
              `max-w-5xl` freed 272px, and under `table-fixed` every one of them
              went to `Details` — the only unsized column — while the picker did
              not move. Measured in Chromium at 1600x1000 with the cap gone and
              this table still at `lg:w-80`: Setting 288, Details **680**, Value
              320, with the sentence inside Details laying out at 532px (its own
              `max-w-prose` measure), so 148px of that column could never be
              used. At `lg:w-96` the same measurement gives 288 / 616 / **384**,
              the trigger 368px wide and 84px dead.

              384 rather than the locally optimal number, on §5.1's own finding.
              At 448px the caveat's summary drops from two lines to one (the row
              85 -> 69px) and the dead space falls to 20px — but `Config` and
              `Response` render this identical three-column grid on the two
              adjacent tabs at `w-52 lg:w-96`, and a reader sees the tabs in
              sequence. A fourth width for one grid is the defect §5.1 was
              reversed to remove, in miniature. It is also worse where the
              breakpoint engages: at a 1024px viewport `Details` is already down
              to 40px at `w-96`. */}
          <ConfigTable valueClassName="w-52 lg:w-96" detailsClassName="max-w-prose">
            <ConfigTableRow
              label="Service account"
              htmlFor="try-account"
              // ONE sentence, which is the §4.4 cap — this cell held 314
              // characters, and the Details column is the one column
              // `table-fixed` hands every surplus pixel to (§5.2), so a
              // paragraph here sets very wide and pushes the picker away
              // from the label naming it. The two sentences that were here
              // are in the `Caveat` under the picker, not gone.
              //
              // A plain string: `details` renders no markup and backticks
              // arrive as literal characters (§6). That is also why the
              // disclosure cannot live in this cell — `details` is
              // deliberately not a `ReactNode` — so it goes in the Value
              // cell, which is beside the control it is about anyway.
              details="The identity the trial authenticates as."
            >
              <div className="space-y-2">
                <Select value={accountId} onValueChange={setAccountId}>
                  <SelectTrigger id="try-account" className="w-full" aria-label="Service account">
                    <SelectValue placeholder="Choose a service account" />
                  </SelectTrigger>
                  <SelectContent>
                    {accounts.map((a) => (
                      <SelectItem key={a.id} value={a.id}>
                        {a.name}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
                {/* A FAILED fetch is not an empty workspace. Collapsed into
                    one branch, the "no service accounts yet" copy explains a
                    500 as "you have not created any" while Run sits disabled
                    — a dead end with a false explanation, and the author goes
                    off to create an account that already exists. Kept in the
                    Value cell, beside the control it is about, rather than
                    moved below the table where it would read as a statement
                    about the whole section. */}
                {accountsQuery.isError ? (
                  <Alert variant="destructive">
                    <AlertDescription>
                      Could not load this workspace&apos;s service accounts.{" "}
                      {(accountsQuery.error as unknown as Error)?.message ?? "Try again in a moment."}
                    </AlertDescription>
                  </Alert>
                ) : (
                  accounts.length === 0 &&
                  !accountsQuery.isLoading && (
                    // Two verbs, two places. This used to bind both to one
                    // link — "Create one, then grant it invoke, in Access" —
                    // and only the second half is true of Access: creating an
                    // account is `clients/new`, under Clients, and
                    // `AccessSection` has no create affordance at all. In THIS
                    // state (zero accounts) its table renders the same "no
                    // service accounts yet" sentence with nothing to act on,
                    // so the one link on screen led to the problem restated.
                    //
                    // One link, on the only step available now. Access is
                    // still named because the second step is real and this is
                    // the only place in this state that says it — but a
                    // destination that can do nothing yet does not earn a
                    // click.
                    <p className="text-xs text-muted-foreground">
                      This workspace has no service accounts yet.{" "}
                      <LeaveGuardLink className="underline" to={newAccountHref}>
                        Create one in Clients
                      </LeaveGuardLink>
                      , then grant it invoke on this endpoint in Access.
                    </p>
                  )
                )}

                {/* The distinction this whole section exists to make, split
                    at its own hinge: the CONSEQUENCE is the summary and is
                    on screen unclicked, the mechanism is behind it. It is
                    the second of the three places that say a trial runs as
                    a SERVICE ACCOUNT rather than as you (the section
                    description and the note beside Run are the others), and
                    `PreviewBlock` says the same thing from the other side.

                    What it must NOT say is that preview runs as the author:
                    on an ordinary connection preview reaches the engine as
                    the same service account a served call does. The trial's
                    claim here is authentication and authorization, which is
                    true on every connection — see the module doc. */}
                <Caveat
                  summary="A green preview does not prove a consumer can call this endpoint"
                  className="text-xs"
                >
                  A real 60-second token is minted for this account server-side and driven
                  through the actual serving path, so the answer is the one that account would
                  get. Preview in Query authenticates no token and checks no invoke grant, so it
                  cannot answer that question however green it is.
                </Caveat>
              </div>
            </ConfigTableRow>
          </ConfigTable>

          {/* Not a `ConfigTableRow`, because it has no Value to edit — a row
              for it would fabricate an empty third cell, which §4.1 records
              as exactly the confusion the cards were replaced to remove. It
              stays a sentence under the table it qualifies.

              Deliberately a NOTE, not a `disabled={dirty}` guard like
              Publish's. A trial traverses the real serving router, which
              joins `endpoint_versions` on `live_version_id`
              (`serving/router.rs`) — it runs the PUBLISHED contract, so it
              neither reads nor can be corrupted by the draft. Blocking it
              while the form is dirty would take away the one tool that
              answers "does what consumers get still work?" at exactly the
              moment an author is changing things. The rows below are built
              from `endpoint.params`, the draft's parameter list, so they can
              even offer a parameter the live version does not accept — which
              is why this says so out loud rather than letting a 400 explain
              it. */}
          <p className="text-xs text-muted-foreground">
            A trial runs the published version. Draft edits — unsaved ones, and saved ones you have
            not published — are not part of what runs here.
          </p>

          {/* A plain `Table`, not `ConfigTable`: these are LIST-shaped — one
              row per declared parameter — not a fixed set of settings with
              help text. Same rule that keeps `ParamEditor` a plain table. */}
          <div className="space-y-2">
            <h4 className="text-sm font-medium">Parameter values</h4>
            <Table className="table-fixed">
              <TableHeader>
                <TableRow>
                  <TableHead className="w-1/3">Parameter</TableHead>
                  <TableHead className="w-28">Type</TableHead>
                  <TableHead>Value</TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {endpoint.params.length === 0 && (
                  // Spanning every column, so the sentence reads as the state
                  // of the whole table rather than as a value in the
                  // Parameter column — `ParamEditor`'s idiom.
                  <TableRow className="hover:bg-transparent">
                    <TableCell colSpan={3} className="text-sm text-muted-foreground">
                      This endpoint declares no parameters. A trial sends only the ones declared in
                      Query.
                    </TableCell>
                  </TableRow>
                )}
                {endpoint.params.map((p, i) => (
                  // The DOM id is the row's POSITION, not its name: a name
                  // is not guaranteed unique on screen (validation refuses a
                  // duplicate DECLARATION, but this list is the draft's), and
                  // two rows sharing an id give one label driving both
                  // inputs — the defect `PreviewBlock` records. The VALUE
                  // state stays keyed by name because that is the shape the
                  // request body carries.
                  <TableRow key={i}>
                    <TableCell className="align-top font-medium whitespace-normal break-words">
                      <label htmlFor={`try-param-${i}`}>
                        {p.name}
                        {p.required && <span className="text-destructive"> *</span>}
                      </label>
                    </TableCell>
                    <TableCell className="align-top text-sm text-muted-foreground">{p.type}</TableCell>
                    <TableCell className="align-top">
                      <Input
                        id={`try-param-${i}`}
                        className="w-full"
                        value={paramValues[p.name] ?? ""}
                        placeholder={p.default ?? undefined}
                        onChange={(e) =>
                          setParamValues((prev) => ({ ...prev, [p.name]: e.target.value }))
                        }
                      />
                    </TableCell>
                  </TableRow>
                ))}
              </TableBody>
            </Table>
          </div>
        </div>
      </section>

      {/* The RESULT, taking whatever the setup leaves. Its header — the status,
          the button, and the shortcut that reaches it — stays put while the
          note, the alerts and the JSON scroll underneath, which is the one
          thing the split was actually buying and the one thing kept.

          `min-h-64` is a FLOOR, not a fraction, and it is what makes the
          degenerate case degrade the right way: an endpoint declaring more
          parameters than the section is tall pushes the total past the
          section's height, so the shell's `overflow-auto` fieldset scrolls and
          everything stays reachable — rather than this half collapsing to
          nothing inside a page that does not move. 256px holds the header, the
          caveat, a refusal alert and the first lines of a body. */}
      <section
        ref={resultRef}
        aria-labelledby="try-result-heading"
        className="mt-4 flex min-h-64 flex-1 flex-col"
      >
        {/* **Not `sticky top-0`, and that is a measurement rather than an
            omission.** Review asked for it, as the cheap way to keep `Run` on
            screen once the section can outgrow the shell's scroll box. I built
            it, measured it in Chromium, and it never fired ONCE: at 8 and 16
            declared parameters, at four scroll offsets each, this header's top
            was equal to the result section's top every time.

            The reason is structural. Sticky engages only when its containing
            block's top passes above the scroll box's top, and this section is
            `flex-1` inside an `h-full` column, so its height is
            `max(256, available − setup)` — never MORE than the box. A box that
            fits can never be scrolled past its own top, so the class is inert.
            Shipping it would have been a class assertion passing over a no-op,
            which is the failure this branch keeps finding.

            What Run's reachability actually rests on is two facts that were
            measured instead: this section is never taller than the scroll box,
            so scrolling to the bottom always reveals Run; and `onRun` scrolls
            the result into view, so a ⌘↵ fired from the top of a long
            parameter list cannot run invisibly. */}
        <div className="flex shrink-0 flex-wrap items-center gap-2 border-b border-border px-1 pb-2">
          <h3 id="try-result-heading" className="text-sm font-medium">
            Result
          </h3>
          {/* In the HEADER, not in the scrolling body: the status is the one
              fact a reader comes back for, and a long JSON body would carry
              it off the top of the panel. */}
          {result && (
            <dl className="flex items-center gap-1.5 text-sm">
              <dt className="text-muted-foreground">Status</dt>
              <dd>
                {/* Both arms are now tinted chips with `--foreground` ink
                    (`chipTone.ts`): `text-success` on its own wash measures
                    3.72:1, under WCAG 1.4.3's 4.5:1 at this size. The refusal
                    arm used to fall through to `Badge`'s DEFAULT variant —
                    solid `bg-primary`, the page's action colour — so a 500 was
                    painted in the same teal as the Run button while the alert
                    directly below it called the same call a refusal. The
                    STATUS NUMBER is what carries the outcome either way. */}
                <Badge className={outcomeOk ? CHIP_TONE.success : CHIP_TONE.destructive}>
                  {result.status}
                </Badge>
              </dd>
            </dl>
          )}
          <Button
            type="button"
            size="sm"
            onClick={() => void onRun()}
            disabled={!canRun}
            title="Run the trial (⌘/Ctrl + ↵)"
          >
            {tryMutation.isPending ? "Running…" : "Run"}
          </Button>
          {/* Named beside the button it fires, not in a help dialog: an
              affordance nobody can see is not an affordance. */}
          <span className="flex items-center gap-1.5 text-xs text-muted-foreground">
            <kbd className="rounded border border-border bg-muted px-1.5 py-0.5 font-mono">⌘↵</kbd>
            runs it
          </span>
        </div>

        {/* The section's ONLY scrolling box. A JSON body has no bound, so this
            is where a bound belongs — and putting it here rather than around
            the setup is the whole change. */}
        <div className="min-h-0 flex-1 space-y-4 overflow-auto px-1 pt-3">
          {/* The note travels WITH the button, `PreviewBlock`'s rule — and
              the headline is what that rule is actually about. "A trial is
              a real call, not a dry run" is on screen with nothing clicked,
              so a reader cannot press Run without having been told; the
              four sentences of mechanism under it are what a reader goes
              looking for once the headline has stopped them, and as
              standing prose they were ~500 characters over one button.

              Not a Radix disclosure: the shell disables this section for a
              trashed endpoint and a Radix trigger is a `<button>`. Reading
              is not editing. */}
          <Caveat summary="A trial is a real call, not a dry run" className="max-w-prose text-xs">
            It runs as the service account above rather than as you. It spends the endpoint-wide
            rate-limit bucket for real — the per-client bucket it counts against is only this
            trial&apos;s own, so a click here cannot throttle a live consumer. It also writes to
            the access log and appears in Activity&apos;s call list under &quot;Include trial
            runs&quot;. And on a cached endpoint it can populate the cache real consumers read,
            including a per-account (
            <code className="rounded bg-muted px-1 py-0.5 font-mono text-xs">per_principal</code>)
            entry for this same account, not only{" "}
            <code className="rounded bg-muted px-1 py-0.5 font-mono text-xs">shared</code> ones.
          </Caveat>

          {runError && (
            <Alert variant="destructive">
              <AlertDescription>{runError}</AlertDescription>
            </Alert>
          )}

          {tooLarge && (
            <Alert>
              <AlertDescription>
                This result is too large to display here. The call still ran for real — its
                rate-limit, access-log and cache effects all happened — check{" "}
                <LeaveGuardLink className="underline" to={activityHref}>
                  Activity
                </LeaveGuardLink>{" "}
                to see it.
              </AlertDescription>
            </Alert>
          )}

          {result && (
            <>
              {notGranted ? (
                <Alert variant="destructive">
                  <AlertDescription>
                    This service account is not granted invoke on this endpoint — the most common
                    first failure.{" "}
                    <Link className="underline" to={accessHref}>
                      Grant it in Access
                    </Link>
                    .
                  </AlertDescription>
                </Alert>
              ) : (
                !outcomeOk && (
                  <Alert variant="destructive">
                    <AlertDescription>
                      The endpoint refused this call{refusal?.code ? ` (${refusal.code})` : ""}.{" "}
                      {refusal?.message ?? "See the body below for details."}
                    </AlertDescription>
                  </Alert>
                )
              )}

              <div className="space-y-1.5">
                <Label>Body</Label>
                {/* No inner height cap: the result half's scrolling body is
                    the cap, and capping again would leave a scrollbar inside a
                    scrollbar with dead space under it. */}
                <pre className="overflow-auto rounded-md border border-border bg-muted/40 p-3 font-mono text-xs">
                  {JSON.stringify(result.body, null, 2)}
                </pre>
              </div>

              <div className="space-y-1.5">
                <Label>Request</Label>
                <div className="space-y-2 rounded-md border border-border bg-muted/40 p-3 font-mono text-xs">
                  <div>
                    {result.request.method} {result.request.url}
                  </div>
                  <div>Authorization: {result.request.authorization}</div>
                  <div className="flex items-start gap-2">
                    <pre className="flex-1 overflow-x-auto whitespace-pre-wrap break-all">
                      {result.request.curl}
                    </pre>
                    <CopyButton value={result.request.curl} />
                  </div>
                </div>
              </div>
            </>
          )}
        </div>
      </section>
    </div>
  );
}
