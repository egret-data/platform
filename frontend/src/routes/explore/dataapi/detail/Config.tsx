import { useState } from "react";
import { Input } from "@/components/ui/input";
import { Textarea } from "@/components/ui/textarea";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import { Button } from "@/components/ui/button";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { ConfigTable, ConfigTableGroup, ConfigTableRow } from "@/components/config/ConfigTable";
import { useDataApiValidateEndpoint } from "@/api/generated/data-api/data-api";
import { CacheScope, type StatementSnapshot } from "@/api/generated/model";
import { useEndpointDetail } from "../EndpointDetail";
import { pendingRoute } from "../endpointState";

/**
 * What `POST /validate` came back with.
 *
 * `statements`, not `columns`: the response carries one entry per statement,
 * each with the columns `describe` resolved for it (M2). Folding them into one
 * list would report a contract no version will ever hold — two statements each
 * returning a `total` are two result sets, not one column declared twice.
 */
type Validation =
  | { ok: true; statements: StatementSnapshot[] }
  | { ok: false; message: string };

/**
 * "Resolves to 2 columns: a (int), b (int)", or the same without the lead-in
 * for a line that is already labelled with its statement's name.
 *
 * One function rather than two strings so the singular/plural and the
 * "no columns at all" case cannot diverge between the one-statement sentence
 * and the several-statement list.
 */
function describeColumns(columns: { name: string; ty: string }[], bare = false): string {
  const count = `${columns.length} column${columns.length === 1 ? "" : "s"}`;
  const named = columns.map((c) => `${c.name} (${c.ty})`).join(", ");
  if (columns.length === 0) return bare ? count : `Resolves to ${count}`;
  return bare ? `${count} — ${named}` : `Resolves to ${count}: ${named}`;
}

/**
 * What a served request is given before egret answers it `504`, in seconds.
 *
 * `egret_dataapi::serving::SERVING_REQUEST_DEADLINE`. Duplicated here because
 * this is the one number the warning below is about and there is no endpoint
 * that reports it — and duplicated SAFELY: the Rust test
 * `serving::guard::tests::the_config_surface_warns_against_this_modules_deadline`
 * reads this file and fails if the two stop agreeing, the same device the
 * ingress annotation is held to.
 */
const SERVING_REQUEST_DEADLINE_SECONDS = 50;

/** `LimitsSpec`'s default for `timeout_seconds` (`egret_dataapi_api`). */
const DEFAULT_TIMEOUT_SECONDS = 30;

/** `TIMEOUT_CEILING_SECS` — serving clamps a larger value down to this. */
const TIMEOUT_CEILING_SECONDS = 300;

/**
 * The worst wall clock this configuration can spend, in seconds.
 *
 * `statements x clamp(timeout_seconds, 1, 300)`, because the statements run
 * SEQUENTIALLY (`serving::router`'s execution loop) and each gets the whole
 * `timeout_seconds` — the value is per statement, not per call, which is the
 * thing an author reading one number in one box has no way to know.
 *
 * The clamp is applied here for the same reason the Details cell names the
 * ceiling: serving reads `timeout_seconds.clamp(1, TIMEOUT_CEILING_SECS)`, so a
 * warning computed from the literal typed value would over-report for anyone who
 * typed 10,000 and under-report for anyone who typed 0.
 */
export function worstCaseSeconds(statementCount: number, timeoutSeconds?: number): number {
  const per = Math.min(Math.max(timeoutSeconds ?? DEFAULT_TIMEOUT_SECONDS, 1), TIMEOUT_CEILING_SECONDS);
  return statementCount * per;
}

/**
 * The sentence shown under the Timeout box when this endpoint's worst case
 * exceeds what a served call is given, or `null` when it does not.
 *
 * # Why here and not at the moment a call dies
 *
 * A two-statement endpoint at the defaults is 2 x 30 = 60s, past the 50s
 * serving deadline — and that is LEGAL. Publishing it is not refused and must
 * not be: a long endpoint is a real thing to want, the author simply has to know
 * that consumers can be cut off before it finishes. Told at serve time it is a
 * mystery `504` in someone else's logs; told at Try-it time it is a mystery
 * `504` in the author's. Told here it is a fact about the two boxes the author
 * is looking at, at the moment they set them.
 *
 * Deliberately NOT a publish refusal, and deliberately not phrased as an error.
 */
export function worstCaseWarning(statementCount: number, timeoutSeconds?: number): string | null {
  const worst = worstCaseSeconds(statementCount, timeoutSeconds);
  if (worst <= SERVING_REQUEST_DEADLINE_SECONDS) return null;
  const plural = statementCount === 1 ? "statement" : "statements";
  return (
    `This endpoint's worst case is ${worst}s — ${statementCount} ${plural} that may each take ` +
    `the full timeout, run one after another. A served call is cut off at ` +
    `${SERVING_REQUEST_DEADLINE_SECONDS}s, so a consumer may get a 504 before it finishes, and so ` +
    `may Try it. This is allowed — lower the timeout, or split the endpoint, if callers need it to complete.`
  );
}

/**
 * Everything about the endpoint as an HTTP object: the URL contract callers use,
 * and the cache / limits / rate-limit policy applied to their calls. Nothing here
 * decides what the query returns — that is `Query`.
 *
 * ONE `Setting | Details | Value` table, its four groups being `ConfigTableGroup`
 * rows. This replaced four `ConfigSection` cards — `Endpoint` framing a
 * four-field form, `Cache` three rows, `Limits` two, `Rate limit` three. Read
 * down the page, that was a column of mostly-empty boxes. The identity fields
 * (name, description, method, path) lived inside the first of those cards as a
 * hand-written form and are now ordinary rows, so every setting on the tab
 * reads the same way and the Details column can explain them.
 *
 * One column, deliberately: a two-column grid pads the short groups to the height
 * of the tall one beside them, and CSS `columns` fights the flex container this
 * page sits in and clips the form at the fold. Both were tried on this editor and
 * both were wrong.
 */
export default function Config() {
  const { ws, id, endpoint, form, setForm, dirty } = useEndpointDetail();
  // A new method or path is saved on the draft but served only from the next
  // publish (KI-023); the rows say so, with the URL callers still use.
  const pending = pendingRoute(endpoint, form.method, form.path);
  const validateMutation = useDataApiValidateEndpoint();
  const [validation, setValidation] = useState<Validation | null>(null);

  async function runValidate() {
    setValidation(null);
    try {
      const res = await validateMutation.mutateAsync({ workspace: ws, id });
      if (res.status === 200) setValidation({ ok: true, statements: res.data.statements });
    } catch (e) {
      setValidation({ ok: false, message: e instanceof Error ? e.message : String(e) });
    }
  }

  return (
    <div className="space-y-6">
      <div>
        <h2 className="text-lg font-semibold">Config</h2>
        <p className="max-w-prose text-sm text-muted-foreground">
          The endpoint as an HTTP object: the URL contract callers use, and the cache, limits, and
          rate-limit policy applied to their calls.
        </p>
      </div>

      {/* Removing the page-level cap did NOT help the controls on its own:
          `ConfigTable` is `table-fixed`, so all surplus width goes to the one
          unsized column — Details — and Value stays pinned wherever its `<th>`
          puts it. This tab owns the two widest controls of any detail section
          (see Description and Path below), so it opts into a wider one.

          `lg:` and not plain `w-96`, which is a measurement, not a hunch. Below
          `lg` the Details column is hidden but the two sized columns do NOT
          shrink to fit — `table-fixed` honours their pixel widths and the
          shadcn `Table` wrapper (`overflow-x-auto`) scrolls instead. At a 500px
          viewport the content box is 356px, so the table already scrolled 140px
          at `w-52`; a flat `w-96` would have pushed that to 316px. Gating the
          widening at `lg` leaves the narrow regime exactly as it was and takes
          the space only where there is space to take.

          Between those two regimes there is a third, and it has a defect THIS
          PHASE INTRODUCED, worth knowing about before anyone "fixes" the widths
          here. `ConfigTableGroup` spans `colSpan={3}`, which keeps the table
          three columns wide even where Details is `display:none` — so fixed
          layout hands the leftover to a phantom third column with no first-row
          cell, and it lands as dead space to the RIGHT of Value. Measured at a
          1010px viewport (content box 706px): columns 288/0/208 with 210px
          dead. Deleting the four group rows from that same live table gives
          410/0/296 and no dead space.

          Do not read that as "pre-existing". `ConfigTableGroup` is new in this
          phase: at the merge base, no file importing `ConfigTable` contained a
          `colSpan` at all, so relative to `main` there was no phantom column on
          any page. It was already there when the WIDTH pass measured — the
          group rows had landed in the earlier tasks — and that is the only
          sense in which it pre-dates anything.

          This comment used to add "and is the only source of `colSpan={3}`
          inside a `ConfigTable` in the tree". THAT IS NO LONGER TRUE, and the
          detail-tabs pass is what falsified it: `ResponseSection.tsx` now
          carries two hand-written `<TableCell colSpan={3}>` rows for the
          transform's editors (:907, :967) beside its TWO group rows (:869,
          :955), `AccessSection.tsx` three (:427, :437, :449) and `TryIt.tsx`
          one (:477). The consequence for anyone re-tuning these widths is the
          part that matters: the phantom column is produced by ANY
          `colSpan={3}` row, so counting `ConfigTableGroup` rows undercounts
          it. `ResponseSection` has the same phantom column from two group rows
          plus two editor rows — not from "its one group row".

          The width pass did not create it, then, but it does make it bigger in
          one sub-regime, and that depends on something easy to miss: the
          sidebar is a user-toggled rail, `w-56` expanded and `w-16` collapsed.
          Measured at that same 1010px viewport with the rail COLLAPSED (content
          box 866px): 370px dead, where the old 768px cap would have given
          272px. Expanded, the content box is 706px and the cap never bound, so
          nothing changed.
          Fixing it means making Value the unsized column below `lg`. That does
          not require touching the shared default — `valueClassName` replaces it
          outright, so this section could pass `"lg:w-96"` with no base width
          and affect nobody else. It is deferred because it is UNVERIFIED that
          an unsized Value actually absorbs the phantom column rather than
          splitting with it, not because it is blocked. */}
      <ConfigTable valueClassName="w-52 lg:w-96" detailsClassName="max-w-prose">
        <ConfigTableGroup label="Endpoint" />
        <ConfigTableRow
          label="Name"
          htmlFor="cfg-name"
          details="Shown wherever this endpoint is listed, and used as the operation summary in the generated OpenAPI document."
        >
          <Input
            id="cfg-name"
            className="w-full"
            value={form.name}
            onChange={(e) => setForm((f) => ({ ...f, name: e.target.value }))}
          />
        </ConfigTableRow>
        {/* Description and Path are the two widest controls on the tab. At the
            shared `w-52` (208px) a 2-row Textarea showed about 50 characters
            and the Path input about 25 before it scrolled — the reason this
            table passes a wider `valueClassName` above. Measured after: both
            controls render 368px at a 1728px viewport. */}
        <ConfigTableRow
          label="Description"
          htmlFor="cfg-description"
          details="Free text explaining what the endpoint returns. It becomes the operation description in the generated OpenAPI document."
        >
          <Textarea
            id="cfg-description"
            className="w-full"
            rows={2}
            value={form.description}
            onChange={(e) => setForm((f) => ({ ...f, description: e.target.value }))}
          />
        </ConfigTableRow>
        <ConfigTableRow label="Method" htmlFor="cfg-method" details="The HTTP verb callers use.">
          <Select value={form.method} onValueChange={(v) => setForm((f) => ({ ...f, method: v }))}>
            <SelectTrigger
              id="cfg-method"
              className="w-full"
              aria-label="Method"
              aria-describedby={pending?.method ? "cfg-method-pending" : undefined}
            >
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="GET">GET</SelectItem>
              <SelectItem value="POST">POST</SelectItem>
            </SelectContent>
          </Select>
          {pending?.method && (
            <p id="cfg-method-pending" className="mt-1.5 text-xs text-muted-foreground">
              The new method goes live when you publish. Callers still use{" "}
              <code className="font-mono">{pending.liveUrl}</code>.
            </p>
          )}
        </ConfigTableRow>
        <ConfigTableRow
          label="Path"
          htmlFor="cfg-path"
          details="Appended to the workspace's Data API base URL to form the URL callers call."
        >
          <Input
            id="cfg-path"
            className="w-full"
            value={form.path}
            aria-describedby={pending?.path ? "cfg-path-pending" : undefined}
            onChange={(e) => setForm((f) => ({ ...f, path: e.target.value }))}
          />
          {pending?.path && (
            <p id="cfg-path-pending" className="mt-1.5 text-xs text-muted-foreground">
              The new path goes live when you publish. Callers still use{" "}
              <code className="font-mono">{pending.liveUrl}</code>.
            </p>
          )}
        </ConfigTableRow>

        <ConfigTableGroup label="Cache" />
        <ConfigTableRow
          label="Enabled"
          htmlFor="cfg-cache-enabled"
          details="Serve repeat calls from a cached response instead of re-running the query."
        >
          <Switch
            id="cfg-cache-enabled"
            checked={!!form.cache.enabled}
            onCheckedChange={(v) => setForm((f) => ({ ...f, cache: { ...f.cache, enabled: v } }))}
          />
        </ConfigTableRow>
        {form.cache.enabled && (
          <>
            <ConfigTableRow
              label="TTL (seconds)"
              htmlFor="cfg-cache-ttl"
              details="How long a cached response stays valid. A call after this re-runs the query."
            >
              <Input
                id="cfg-cache-ttl"
                className="w-full"
                type="number"
                min={0}
                value={form.cache.ttl_seconds ?? 0}
                onChange={(e) =>
                  setForm((f) => ({ ...f, cache: { ...f.cache, ttl_seconds: Number(e.target.value) } }))
                }
              />
            </ConfigTableRow>
            <ConfigTableRow
              label="Scope"
              htmlFor="cfg-cache-scope"
              details="Per principal caches separately for each service account, so Ranger-filtered rows never cross principals. Shared caches one copy for everyone — only safe when the connection does not run queries as the caller."
            >
              <Select
                value={form.cache.scope ?? CacheScope.per_principal}
                onValueChange={(v) => setForm((f) => ({ ...f, cache: { ...f.cache, scope: v as CacheScope } }))}
              >
                <SelectTrigger id="cfg-cache-scope" className="w-full" aria-label="Cache scope">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value={CacheScope.per_principal}>Per caller</SelectItem>
                  {/* Refused at validate time on a delegated connection — Ranger filters
                      per principal, so a shared cache would leak across identities. The
                      picker still offers it (unlike `location`): this refusal depends on
                      the DATABASE, which can change out from under a saved choice, so
                      there is no static set of values to omit here. */}
                  <SelectItem value={CacheScope.shared}>Shared</SelectItem>
                </SelectContent>
              </Select>
            </ConfigTableRow>
          </>
        )}

        {/* # `Limits` is a THIRD sentinel, and it is the opposite of the rate
            limits' one — do not unify the three groups' copy.

            `LimitsSpec` does not default to `0`
            (`crates/egret-dataapi-api/src/lib.rs:172-183`): `max_rows` defaults
            to 10,000 and `timeout_seconds` to 30, so a blank box is the
            DEFAULT, not "no limit". And serving clamps both rather than reading
            them literally — `router.rs:2054-2057` is
            `max_rows.clamp(1, MAX_ROWS_CEILING)` and
            `timeout_seconds.clamp(1, TIMEOUT_CEILING_SECS)` — so `0` means ONE
            ROW and ONE SECOND. `router.rs:2718`'s test says it in as many
            words: "a 0 cap must not mean 'return nothing'".

            **There is no value here that means unlimited**, which is what the
            `placeholder="unlimited"` these two rows shipped with claimed. The
            ceilings (1,000,000 rows, 300 seconds) are named in the Details
            cells because an author who types more gets silently less — the
            same "a limit that silently is not one" the ceiling constants exist
            to prevent, pointed the other way. */}
        <ConfigTableGroup label="Limits" />
        <ConfigTableRow
          label="Max rows"
          htmlFor="cfg-max-rows"
          details="Hard cap on rows returned. A result larger than this fails the call rather than being silently truncated. Blank means the default, 10,000; 0 serves one row rather than everything; above 1,000,000 is clamped to it."
        >
          <Input
            id="cfg-max-rows"
            className="w-full"
            type="number"
            min={0}
            placeholder="10,000 — the default"
            value={form.limits.max_rows ?? ""}
            onChange={(e) =>
              setForm((f) => ({
                ...f,
                limits: { ...f.limits, max_rows: e.target.value === "" ? undefined : Number(e.target.value) },
              }))
            }
          />
        </ConfigTableRow>
        <ConfigTableRow
          label="Timeout (seconds)"
          htmlFor="cfg-timeout"
          details="How long the engine may take before the call is abandoned. Blank means the default, 30; 0 becomes one second rather than no timeout; above 300 is clamped to it."
        >
          <Input
            id="cfg-timeout"
            className="w-full"
            type="number"
            min={0}
            placeholder="30 — the default"
            value={form.limits.timeout_seconds ?? ""}
            onChange={(e) =>
              setForm((f) => ({
                ...f,
                limits: {
                  ...f.limits,
                  timeout_seconds: e.target.value === "" ? undefined : Number(e.target.value),
                },
              }))
            }
          />
          {/* Under the box that causes it, not in a banner at the top of the
              page: the number the author would change is six inches away. It
              appears and disappears as they type, because it is a property of
              what is in the two inputs and not of anything the server was
              asked. */}
          {worstCaseWarning(form.statements.length, form.limits.timeout_seconds) && (
            <p className="mt-2 text-sm text-muted-foreground" data-testid="worst-case-warning">
              {worstCaseWarning(form.statements.length, form.limits.timeout_seconds)}
            </p>
          )}
        </ConfigTableRow>

        {/* # What a rate limit of `0` means, and why the three rows do not agree
            about it

            Every endpoint carries `0` on all three fields until somebody sets
            one — `RateLimitSpec` derives `#[serde(default)]` for each
            (`crates/egret-dataapi-api/src/lib.rs:187-194`) — and on a
            security-shaped setting "unlimited" and "blocked" are opposites. All
            three rows shipped the same `placeholder="unlimited"` and said
            nothing else, which was right for the wrong reason on two of them and
            simply false on the third.

            `crates/egret-dataapi/src/serving/ratelimit.rs` settles it:

              - `Rate::from_spec` (`:86-89`) returns `None` when `qps == 0`, and
                `check_request` (`:202`, `:214`) installs no bucket for a `None`.
                So `0` on either qps field is NO LIMIT. The function's own doc
                (`:76-80`) says why it has to be: "Treating that as 'zero
                requests per second' would make every default endpoint
                unreachable; it means unlimited."
              - `burst == 0` is a DIFFERENT sentinel (`:91-95`): the burst
                becomes the qps — one second of allowance, the smallest burst
                that does not refuse a client sending exactly at its configured
                rate. Not unlimited, and not zero.
              - That burst belongs to the per-KEY bucket alone. `:214` hard-codes
                `0` for the endpoint-wide one, with the reason in the comment
                there, so a burst set on an endpoint with no `per_key_qps` has no
                effect whatsoever.

            A stored `0` renders as `0` (`?? ""` is nullish-coalescing, and `0`
            is not nullish), so the placeholder is only ever seen on an EMPTY
            box: it teaches what leaving the field alone does, and the Details
            cell is what explains the `0` an author is looking at. Both, because
            neither reaches the other's case. */}
        <ConfigTableGroup label="Rate limit" />
        <ConfigTableRow
          label="Per caller (qps)"
          htmlFor="cfg-rl-per-key"
          details="Sustained requests per second allowed for each client credential. 0, the default, means no per-caller limit."
        >
          <Input
            id="cfg-rl-per-key"
            className="w-full"
            type="number"
            min={0}
            placeholder="0 — unlimited"
            value={form.rate_limit.per_key_qps ?? ""}
            onChange={(e) =>
              setForm((f) => ({
                ...f,
                rate_limit: {
                  ...f.rate_limit,
                  per_key_qps: e.target.value === "" ? undefined : Number(e.target.value),
                },
              }))
            }
          />
        </ConfigTableRow>
        <ConfigTableRow
          label="Burst"
          htmlFor="cfg-rl-burst"
          details="How many requests may arrive at once above the sustained rate before throttling starts. 0 means one second of the per-caller rate, and it does nothing at all unless Per caller (qps) is set."
        >
          <Input
            id="cfg-rl-burst"
            className="w-full"
            type="number"
            min={0}
            placeholder="0 — one second of the per-caller rate"
            value={form.rate_limit.burst ?? ""}
            onChange={(e) =>
              setForm((f) => ({
                ...f,
                rate_limit: { ...f.rate_limit, burst: e.target.value === "" ? undefined : Number(e.target.value) },
              }))
            }
          />
        </ConfigTableRow>
        <ConfigTableRow
          label="Endpoint-wide (qps)"
          htmlFor="cfg-rl-per-endpoint"
          details="Ceiling across all callers of this endpoint combined, whatever their individual limits. 0, the default, means no endpoint-wide limit."
        >
          <Input
            id="cfg-rl-per-endpoint"
            className="w-full"
            type="number"
            min={0}
            placeholder="0 — unlimited"
            value={form.rate_limit.per_endpoint_qps ?? ""}
            onChange={(e) =>
              setForm((f) => ({
                ...f,
                rate_limit: {
                  ...f.rate_limit,
                  per_endpoint_qps: e.target.value === "" ? undefined : Number(e.target.value),
                },
              }))
            }
          />
        </ConfigTableRow>
      </ConfigTable>

      <div className="space-y-2 border-t border-border pt-4">
        <div className="flex items-center gap-2">
          <h3 className="text-sm font-medium">Validate</h3>
          <Button
            type="button"
            variant="outline"
            size="sm"
            onClick={runValidate}
            disabled={validateMutation.isPending || dirty}
            title={dirty ? "Save your changes before validating — Validate checks the saved draft." : undefined}
          >
            {validateMutation.isPending ? "Validating…" : "Validate saved draft"}
          </Button>
        </div>
        {/* Inline, not a toast: a toast vanishes before an author who is mid-edit
            reads it, and a validation failure is exactly the moment they need
            the message to still be on screen. */}
        {validation &&
          (validation.ok ? (
            <Alert>
              <AlertDescription>
                {/* One statement keeps the sentence it always had, name and
                    all left out: its response is an ARRAY, so `main` is not a
                    key any caller reads, and printing it here would be the
                    first thing on this page to suggest otherwise. Several get
                    a list, because each one is a separately-keyed result set
                    with its own contract. */}
                {validation.statements.length <= 1 ? (
                  // `<= 1`, not `=== 1`: a validate that resolves nothing must
                  // still say "Resolves to 0 columns" rather than render an
                  // empty list with no sentence at all.
                  describeColumns(validation.statements[0]?.columns ?? [])
                ) : (
                  <ul className="space-y-0.5">
                    {validation.statements.map((s) => (
                      <li key={s.name}>
                        <span className="font-mono">{s.name}</span>: {describeColumns(s.columns, true)}
                      </li>
                    ))}
                  </ul>
                )}
              </AlertDescription>
            </Alert>
          ) : (
            <Alert variant="destructive">
              <AlertDescription>{validation.message}</AlertDescription>
            </Alert>
          ))}
      </div>
    </div>
  );
}
