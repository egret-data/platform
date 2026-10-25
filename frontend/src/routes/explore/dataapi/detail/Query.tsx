import { useEffect, useMemo, useRef, useState } from "react";
import { ChevronDown, ChevronRight, ChevronUp, Plus, Trash2 } from "lucide-react";
import type { Monaco } from "@monaco-editor/react";
import type * as MonacoNS from "monaco-editor";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Input } from "@/components/ui/input";
// `Separator` is aliased: `@/components/ui/separator` exports that name for the
// visual divider, and react-resizable-panels v4 renamed `PanelResizeHandle`
// into a collision with it. Same alias `components/sqllab/SqlTab.tsx` uses.
import { Group, Panel, Separator as PanelSeparator } from "react-resizable-panels";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { ConfigTable, ConfigTableRow } from "@/components/config/ConfigTable";
import { Caveat } from "./Caveat";
import { SqlEditor } from "@/components/sqllab/SqlEditor";
import MonacoEditor from "@/components/monaco/MonacoEditor";
import { useIsDark } from "@/hooks/useIsDark";
import { isFullConnection, useDatabases } from "@/api/databases";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { GOVERNED_BLOCKED_MESSAGE, governedWithoutPerPrincipal } from "../governance";
import { useEndpointDetail } from "../EndpointDetail";
import { ParamEditor } from "./ParamEditor";
import { PreviewBlock, type PreviewHandle } from "./PreviewBlock";
import type { StatementSource, StatementSpec } from "@/api/generated/model";
import { tabsListClass, tabsTriggerClass } from "@/components/ui/tabs";

/**
 * The SQL text of a statement, or `""` when a script builds it.
 *
 * Only `siblings` reads this now (see the note there for why a scripted
 * sibling contributes `""` rather than being dropped). The statement on
 * SCREEN is handed around as a whole [`StatementSource`], so that the two
 * cases stay one key each and neither can be flattened into the other.
 */
const sqlOf = (s: StatementSpec): string => ("sql" in s ? s.sql : "");

/**
 * A statement's SOURCE — everything about it except its name.
 *
 * Rebuilt as a fresh one-key object rather than passed through as `s`: a
 * `StatementSpec` is `{name, sql}` or `{name, script}`, and the two places
 * this feeds (the preview body and `setActiveSource`) must carry exactly one
 * source key and no `name`. The backend's hand-written reader refuses both
 * keys, neither key and an unknown key, so "spread whatever is there" is a
 * 422 waiting for the first edit that adds a field.
 */
const sourceOf = (s: StatementSpec): StatementSource =>
  "sql" in s ? { sql: s.sql } : { script: s.script };

/**
 * A fresh, syntactically valid `build_sql` an author can edit from — the shape
 * `crates/egret-dataapi/src/transform.rs` documents as the entry point, so what
 * the box opens with is what the evaluator is looking for.
 *
 * Not blank, for the reason `nextStatementName` is not blank: a script that
 * does not define `build_sql` is refused by name (`MissingEntryPoint`), and an
 * author who ticks the box, types their SQL where the `return` goes and saves
 * would meet that refusal for a function they were never shown.
 */
const DEFAULT_BUILD_SQL_SCRIPT = 'def build_sql(params):\n    return "SELECT 1"\n';

/**
 * Everything that decides what the query RETURNS: the database it runs against,
 * the SQL, the parameters it declares, and a preview of the rows.
 *
 * **The shape is SQL Lab's, deliberately** (`components/sqllab/SqlTab.tsx`):
 * a strip of tables on top, then a vertical `Group` whose two panels hold the
 * editor and the preview with a drag handle between them. This is the third
 * place in the product where somebody writes SQL and looks at rows; inventing a
 * third idiom for it would mean the editor a user learned in SQL Lab behaves
 * differently here. Everything else on this page is a table, per
 * `docs/en/ui-conventions.md` §4 — including the database picker, which is one
 * `ConfigTable` row rather than a labelled control floating above the editor.
 *
 * **Why the split rather than a stack.** The preview used to sit BELOW
 * `ParamEditor`, which put it off-screen for anyone editing SQL with more than
 * a couple of parameters declared: run it, scroll down, read, scroll back. The
 * two halves are now on screen at once and ⌘↵ runs the lower one from the
 * upper one.
 *
 * **Schema-aware completion is deliberately absent.** `SqlEditor` takes
 * `schemaWords`/`schema`, and SQL Lab fills them from its schema explorer's
 * local state (a `SchemaSink` the `DataPanel` writes into). There is no
 * reusable hook to call and no explorer on this page, so `[]` is passed and
 * Monaco offers only its built-in SQL keyword list. Wiring a catalog fetch here
 * is its own task, not a side effect of this one.
 *
 * Parameters appear twice here on purpose. `ParamEditor` holds the DECLARATION
 * (name, type, required, default, allowed) which is saved with the endpoint;
 * `PreviewBlock` holds a TEST VALUE per declared parameter, which is not. Putting
 * a test value in the declaration's row would invite the reader to think it was
 * stored.
 *
 * # One statement is the common case, and it pays nothing for the others
 *
 * An endpoint's definition is a LIST of named statements (M2), but almost every
 * endpoint has exactly one — that is the shape migration 0129 gave every
 * endpoint that existed before the feature, and the shape `EndpointCreate` still
 * makes. So the list UI is **rendered by the second statement, not by the
 * first**: at one statement this section renders what it rendered before the
 * feature existed — an editor under an `SQL` heading — plus the one control that
 * has to be discoverable for the feature to exist at all, `Add statement`.
 *
 * The name is hidden in that case for a reason beyond tidiness. A one-statement
 * endpoint answers with an ARRAY; only a several-statement endpoint keys `data`
 * by statement name. Showing `main` beside a lone editor would put a word on
 * screen that looks like part of the contract and is not one.
 *
 * # Why the switcher is not a `<button>`, and not Radix `Tabs`
 *
 * This section renders inside the shell's `<fieldset disabled={trashed}>`, which
 * disables every form control under it — and a child's own `disabled={false}`
 * cannot override that. Switching statements is READING, exactly like collapsing
 * the strip above, so a switcher made of buttons (Radix `TabsTrigger` is one)
 * would leave a trashed endpoint's second statement permanently unreachable.
 * `role="tab"` on a plain element is not a form control, so it survives — the
 * same reason the strip above is a native `<details>`/`<summary>`.
 */

/** The rule `egret_dataapi_api::validate_statement_name` enforces with a 400. */
const STATEMENT_NAME = /^[a-z][a-z0-9_]*$/;

/**
 * Why a name is refusable, or `null`.
 *
 * Said beside the field rather than left to the Save: a name is the KEY a
 * caller reads this statement's rows under, so it is a contract, and the server
 * answers a bad one with a 400 that arrives as a red toast after the work.
 */
function statementNameError(name: string, all: string[], index: number): string | null {
  if (!STATEMENT_NAME.test(name)) {
    return "A statement name must start with a lower-case letter and use only lower-case letters, digits and underscores — it is the key callers read this statement's rows under.";
  }
  if (all.some((other, i) => i !== index && other === name)) {
    return `The name “${name}” is already used by another statement. Each one is a distinct key in the response, so two cannot share it.`;
  }
  return null;
}

/**
 * How many statements one endpoint may declare — `MAX_STATEMENTS` in
 * `crates/egret-dataapi-api/src/lib.rs`, which is where the number is decided
 * and where the measurements behind it are written down.
 *
 * Still a copy: there is no mechanism in this repo that shares a constant
 * between Rust and TypeScript, and the number is a rule `validate_statements`
 * applies rather than a field of any model. The server remains the authority —
 * create, PATCH and publish all refuse a longer list with a 400 — and this
 * decides only when the control stops offering to make one, so a stale copy
 * costs a toast, not a bad save.
 *
 * What is no longer missing is the thing that NOTICES the copy going stale.
 * `statements` now carries `maxItems` in `openapi.json` (a `#[schema(max_items)]`
 * the Rust side pins to `MAX_STATEMENTS` with a `const` assertion), and
 * `__tests__/Statements.test.tsx` reads that committed document and compares it
 * with this constant. Exported for that test alone — nothing else imports it.
 */
export const MAX_STATEMENTS = 16;

/**
 * A valid, unused name for a statement being added.
 *
 * Generated rather than left blank because a blank name is not a valid one, and
 * an author who adds a statement and saves without noticing would get a 400 for
 * a field they never knowingly filled in.
 */
function nextStatementName(existing: string[]): string {
  for (let n = existing.length + 1; ; n += 1) {
    const candidate = `statement_${n}`;
    if (!existing.includes(candidate)) return candidate;
  }
}

/** The id of the script editor's ⌘↵ action (run the preview). */
const PREVIEW_RUN_ACTION = "egret.dataapi.preview";

export default function Query() {
  const { ws, form, setForm, trashed } = useEndpointDetail();
  const { data: databases } = useDatabases(ws);
  // The editor's ⌘↵ has to reach the preview's button without the preview's
  // state (test values, rows, error) being lifted up here to be passed back
  // down — see `PreviewHandle`.
  const preview = useRef<PreviewHandle>(null);

  const statements = form.statements;
  const several = statements.length > 1;
  // Which statement is being edited. Clamped on read rather than corrected in
  // an effect: removing the last statement in the list drops the index out of
  // range for exactly one render, and a `useEffect` that calls `setState` to
  // fix it is the shape this feature avoids everywhere else.
  const [selected, setSelected] = useState(0);
  const active = Math.min(selected, Math.max(statements.length - 1, 0));
  const current = statements[active] ?? { name: "", sql: "" };

  /**
   * Every statement's SQL except the one on screen.
   *
   * By INDEX, not by name: two statements may share a name while the author is
   * mid-edit (`statementNameError` is what says so), and filtering by name would
   * then drop the wrong one — or drop `current` twice and leave a sibling in.
   *
   * A scripted sibling contributes `""`, and it is KEPT in the list rather than
   * filtered out. The wire type is `string[]` with no way to say "unknown", and
   * the two candidate lies differ: `validate_preview` refuses
   * `1 + siblings.length > MAX_STATEMENTS`, so dropping a sibling understates the
   * endpoint and lets a preview through that publish would refuse, while `""`
   * keeps the COUNT exact and contributes nothing to the iterated-parameter
   * union.
   *
   * **That second part is a lie, and as of Task 8 it is a reachable one.** A
   * sibling script's emitted template may well carry a `{% for %}` header, and
   * `""` never joins the iterated-parameter union — so a preview can resolve a
   * scalar where serving binds a list, and can count a several-template script
   * as one against the endpoint's template budget. It is the divergence
   * `preview.rs`'s module comment records in full, and it is a property of the
   * WIRE: `siblings` is `string[]`, so no client can say "this one is a
   * script". Closing it means sending sources rather than strings and
   * generating every one of them per preview — N author scripts per press of a
   * button — which is a decision about the request type, not something this
   * component can fix by choosing a better lie. Publish and serving both see
   * the real thing, so the failure is "preview green, publish refuses", never
   * the reverse.
   */
  const siblings = useMemo(
    () => statements.filter((_, i) => i !== active).map(sqlOf),
    [statements, active],
  );

  /**
   * Whether the statement on screen has its SQL built by a script.
   *
   * Derived from the statement, never held in state of its own. A `useState`
   * here would make the mode a property of the SECTION — one flag for N
   * statements — so switching tabs would show the wrong editor and, worse, the
   * next keystroke would write a script into a statement the author never
   * switched. M2 gave every statement its own contract; this is that.
   */
  const scripted = !("sql" in current);

  /** The statement's body, whichever of the two it has — what a switch would
   *  discard, and `""` when there is nothing to discard. */
  const body = scripted ? current.script : current.sql;

  const nameError = useMemo(
    () => (several ? statementNameError(current.name, statements.map((s) => s.name), active) : null),
    [several, current.name, statements, active],
  );

  /** Replace the whole list — `statements` is patched whole, never per entry. */
  const setStatements = (next: StatementSpec[]) => setForm((f) => ({ ...f, statements: next }));

  /**
   * Rename the statement on screen, **without touching its source.**
   *
   * `{ ...s, name }` and nothing else, which is what makes a rename a rename.
   * The first version of package S wrote `{ name, sql: sqlOf(s) }` here, and
   * for a scripted statement `sqlOf` is `""` — so a pure rename silently threw
   * the author's script away and sent a text statement in its place.
   * Identity and content are separate concerns, and this function only knows
   * about the first.
   */
  const renameActive = (name: string) =>
    setStatements(statements.map((s, i) => (i === active ? { ...s, name } : s)));

  /**
   * Replace the SOURCE of the statement on screen, keeping its name.
   *
   * Rebuilt as `{ name, ...source }` rather than patched onto `s`: a patch
   * leaves the OTHER case's key in place, and a statement carrying both `sql`
   * and `script` has two definitions with no rule for which one runs — which
   * the backend's reader refuses with a 422. Building the object from the name
   * plus exactly one source is what makes "never both, never neither" a
   * property of the code rather than of every caller remembering.
   */
  const setActiveSource = (source: StatementSource) =>
    setStatements(statements.map((s, i) => (i === active ? { name: s.name, ...source } : s)));

  /**
   * What switching a statement's mode threw away, by POSITION in the list.
   *
   * A `ref`, not state, and read on the way back — `ResponseSection`'s
   * `clearedByTransform` exactly: an author who ticks the box to see what a
   * script would look like and unticks it again must find their SQL where they
   * left it. Without this, the control whose entire purpose is to be tried out
   * is also silent data loss, and the thing lost is the whole statement.
   *
   * # A position is the only key available, so `move` and `remove` must forget
   *
   * A statement has no id, and its name is not unique while it is being
   * renamed (`statementNameError` is what says so), so the entry is keyed by
   * POSITION — and `move` and `removeActive` are precisely the two operations
   * that change what a position means. Leaving the memory alone across them
   * hands one statement's body to another: with `[rows(sql), total(script)]`,
   * switching `rows` to a script and then removing it leaves `total` at
   * position 0, and unticking `total` rewrites it with the DELETED statement's
   * SQL — its own script gone. Both call sites clear the memory, and
   * `__tests__/Query.test.tsx` has one test per operation.
   *
   * **`addStatement` deliberately does not clear it.** Adding only ever
   * appends, so every existing position still means what it meant, and
   * forgetting there would throw away a live undo for nothing. "Clear it
   * everywhere, defensively" is the wrong shape: it costs a real affordance
   * and, being untestable, is exactly the kind of line that survives a
   * mutation because no assertion can tell it from its absence.
   *
   * # It does not survive this section unmounting, and the UI says so
   *
   * `Query`, `Config`, `Response` and the rest are SIBLING routes (`App.tsx`),
   * so opening another section unmounts this one and takes the memory with it;
   * so does a reload. The alternative — hoisting the ref into
   * `EndpointDetail`'s context so it outlives the section — would widen the
   * undo without ever making it a guarantee (a reload, another endpoint, a new
   * tab all still lose it), which leaves an author unable to tell the cases
   * apart. That is the actual defect, and it is not fixed by covering one more
   * case: it is fixed by SAYING what the undo is worth, beside the control,
   * BEFORE the click. The sentence in the render below is that, and it is what
   * makes the memory an in-page convenience rather than a promise this section
   * quietly breaks on navigation.
   */
  const discarded = useRef<Record<number, StatementSource>>({});

  /**
   * Switch the statement on screen between written SQL and a script.
   *
   * The mode is not a stored field — it IS which key the source carries — so
   * this both removes the control that goes inert and clears the value it held,
   * in one patch, because they are the same write. That is C2's rule
   * (`ResponseSection`) made unavoidable: there is no representable state where
   * the hidden `sql` survives underneath a script.
   *
   * The ref is read AND written outside `setForm`'s updater. React double-
   * invokes a `useState` updater under `StrictMode` and keeps only the second
   * call's result, so a ref mutated inside would be seen already-consumed on
   * the second pass and the restore would silently undo itself — measured in
   * C2, in `disableTransform`.
   */
  function setActiveMode(toScript: boolean) {
    if (toScript === scripted) return;
    const remembered = discarded.current[active];
    discarded.current = { ...discarded.current, [active]: sourceOf(current) };
    // Only if it is the mode being switched TO: the memory holds one entry per
    // position, and a third switch would otherwise restore the wrong half.
    const restore = remembered && "script" in remembered === toScript ? remembered : undefined;
    setActiveSource(restore ?? (toScript ? { script: DEFAULT_BUILD_SQL_SCRIPT } : { sql: "" }));
  }

  const atLimit = statements.length >= MAX_STATEMENTS;

  const addStatement = () => {
    // Guarded as well as disabled: the button is the only caller today, and a
    // guard here is what makes that stay true.
    if (atLimit) return;
    // No `discarded.current = {}` here, unlike `move` and `removeActive`:
    // appending renumbers nothing, so every remembered position still points
    // at the statement it was taken from. See the ref's own comment.
    setStatements([...statements, { name: nextStatementName(statements.map((s) => s.name)), sql: "" }]);
    // Select the one just added: an author who asks for a statement means to
    // write it, and landing them anywhere else would hide what they created.
    setSelected(statements.length);
  };

  const move = (delta: -1 | 1) => {
    const to = active + delta;
    if (to < 0 || to >= statements.length) return;
    const next = [...statements];
    [next[active], next[to]] = [next[to], next[active]];
    discarded.current = {};
    setStatements(next);
    // The moved statement stays selected — it is the one being worked on, and
    // following the INDEX instead would silently switch the editor's contents.
    setSelected(to);
  };

  const removeActive = () => {
    // Structurally unreachable at one statement (the control is not rendered),
    // and guarded anyway: the schema's own CHECK makes an empty list
    // unrepresentable, so this must never be the request that tries.
    if (statements.length < 2) return;
    discarded.current = {};
    setStatements(statements.filter((_, i) => i !== active));
    setSelected(Math.max(active - 1, 0));
  };

  /**
   * `trashed`, readable from a Monaco command registered once at mount.
   *
   * The script editor's ⌘↵ is registered in `onMount` and never re-registered,
   * so a plain closure over `trashed` would keep whatever the value was when
   * the editor appeared — and an endpoint can be trashed from the header while
   * this section is on screen. `SqlEditor` holds its `onRun` the same way.
   *
   * Written from an EFFECT rather than during render: `react-hooks/refs`
   * refuses a ref assignment in a render body, and this repo's lint gate is
   * warning-count-based. The command cannot fire before the commit that runs
   * this effect — it needs a mounted editor and a keystroke — so the guard is
   * never read stale.
   */
  const trashedRef = useRef(trashed);
  useEffect(() => {
    trashedRef.current = trashed;
  }, [trashed]);
  // The script editor's ⌘↵ action, disposed when the script editor leaves the
  // page (the statement switched to SQL, or this section unmounted): disposing
  // a Monaco editor does not remove an action's keybinding.
  const scriptRunAction = useRef<MonacoNS.IDisposable | null>(null);
  useEffect(() => {
    if (!scripted) return;
    return () => {
      scriptRunAction.current?.dispose();
      scriptRunAction.current = null;
    };
  }, [scripted]);
  const isDark = useIsDark();

  return (
    <div className="flex h-full flex-col">
      {/* Kept, and kept on one line: every one of the six sections opens with
          its own `h2`, and this is the only thing that names the section for a
          reader who arrived by URL rather than by clicking the nav. */}
      <div className="flex shrink-0 flex-wrap items-baseline gap-x-3 gap-y-1 pb-2">
        <h2 className="text-lg font-semibold">Query</h2>
        <p className="max-w-prose text-sm text-muted-foreground">
          The query this endpoint runs, and the parameters callers may pass it.
        </p>
      </div>

      {/* A native `<details>`, not `ui/collapsible`: this section renders inside
          the shell's `<fieldset disabled={trashed}>`, which disables every
          BUTTON under it — a Radix trigger included. Collapsing the strip is
          reading, not editing, so a trashed endpoint must keep it. `<summary>`
          is not a form control and is untouched by the fieldset. */}
      {/* Collapsed by default, and that is a measured choice rather than a
          preference. On a 900px-tall viewport this section gets 615px: open,
          the strip took 370 of them and left the SQL editor 107px — about four
          lines, i.e. the exact problem this redesign exists to fix. Collapsed,
          the editor gets 295px and the preview 241px. The summary still names
          what is inside, including how many parameters, so nothing is hidden —
          it is one click, and the tab's purpose is writing SQL.
          The summary names the database and the parameters themselves, not
          only how many there are (KI-031): closed, the strip still answers
          "what does this run against, and what can a caller pass". */}
      <details className="group shrink-0">
        <summary className="flex cursor-pointer list-none items-center gap-2 py-1 text-sm text-muted-foreground hover:text-foreground">
          <ChevronRight className="h-4 w-4 shrink-0 transition-transform group-open:rotate-90" aria-hidden="true" />
          <span className="shrink-0">Database and parameters ({form.params.length})</span>
          <span className="min-w-0 truncate text-foreground" data-testid="query-strip-contents">
            {[
              // Nothing while the list loads: "no database" would be a false claim.
              form.database_id
                ? (databases ?? []).find((d) => d.id === form.database_id)?.name
                : "No database chosen",
              form.params.length === 0 ? "no parameters" : form.params.map((p) => p.name).join(", "),
            ]
              .filter(Boolean)
              .join(" · ")}
          </span>
        </summary>
        {/* Capped and scrollable: an endpoint with a dozen declarations would
            otherwise push the editor and the preview out of the viewport,
            which is the exact failure this layout exists to fix. A VIEWPORT
            cap, not a percentage one — `<details>` has an auto height, and a
            percentage `max-height` against an auto-height parent computes to
            `none`, i.e. no cap at all. */}
        <div className="max-h-[40vh] space-y-4 overflow-auto pb-3 pt-2">
          <ConfigTable valueClassName="w-64">
            <ConfigTableRow
              label="Database"
              htmlFor="q-db"
              details="The connection the SQL runs against — for the preview here, and for every request the published endpoint serves."
            >
              <Select
                value={form.database_id}
                onValueChange={(v) => setForm((f) => ({ ...f, database_id: v }))}
              >
                <SelectTrigger id="q-db" className="w-full" aria-label="Database">
                  <SelectValue placeholder="Choose a database" />
                </SelectTrigger>
                <SelectContent>
                  {/* Governed connections are still offered — see
                      `EndpointCreate` for why hiding them is the wrong shape —
                      and the ones that cannot carry a publish are named by the
                      warning below rather than by an absence. */}
                  {(databases ?? []).map((d) => (
                    <SelectItem key={d.id} value={d.id}>
                      {d.name}
                      {isFullConnection(d) && d.governed ? (
                        <span className="ml-1.5 text-xs text-muted-foreground">Governed</span>
                      ) : null}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </ConfigTableRow>
          </ConfigTable>

          {/* The refusal an author would otherwise meet only at Publish, said at
              the moment the database is chosen.

              It names Run too, because Run is refused too. An earlier version of
              this comment claimed the opposite — "a preview runs as the author,
              under the author's own engine policy" — and that was simply untrue:
              `preview::run` goes through the same `execute_as` path as a served
              request, so on a connection that is neither delegated nor
              impersonating the engine sees the SERVICE ACCOUNT and the preview
              returns unmasked rows. The server gates it now
              (`preview::run` → `lifecycle::check_governed_connection`), and this
              sentence has to match or the author finds out at the first press. */}
          {governedWithoutPerPrincipal(
            (databases ?? []).find((d) => d.id === form.database_id),
          ) ? (
            <Alert>
              <AlertDescription>{GOVERNED_BLOCKED_MESSAGE}</AlertDescription>
            </Alert>
          ) : null}

          <ParamEditor params={form.params} onChange={(params) => setForm((f) => ({ ...f, params }))} />
        </div>
      </details>

      {/* `min-h-0` is what lets this hand height back: a flex item's default
          `min-height: auto` refuses to shrink below its content, and the
          panels' content is a Monaco instance that will happily claim it. */}
      <Group orientation="vertical" className="min-h-0 flex-1">
        {/* STRINGS. A bare `55` means 55 PIXELS in react-resizable-panels v4 —
            the silent-semantics change this repo already hit once during that
            upgrade. `SqlTab.tsx` uses the same two literals. */}
        <Panel defaultSize="55%" minSize="20%">
          <div className="flex h-full flex-col">
            <div className="flex shrink-0 items-center gap-2 border-b border-border px-1 pb-2">
              {/* One statement renders the heading it always rendered; the
                  switcher is what the SECOND statement brings. */}
              {several ? (
                // `overflow-x-auto` lives HERE, on the strip itself, not on this
                // row. It used to sit on the row (the parent of this and the
                // button below), which let the strip's own children paint past
                // ITS box — nothing clipped them — and bleed across the button
                // beside it; scrolling the row did not help because the button
                // scrolled along with the strip, so the overlap was identical at
                // every scroll position. Measured in a real browser at 1600px
                // with 16 statements: `elementFromPoint` at the button's centre
                // returned the 13th tab, not the button, at every `scrollLeft`
                // tried (0, 142, max). `flex-1 min-w-0` lets the strip shrink to
                // make room for the button (`shrink-0` below) instead of pushing
                // it; the strip scrolls, the button never moves and is never
                // covered.
                <div
                  role="tablist"
                  aria-label="Statements"
                  className={`min-w-0 flex-1 ${tabsListClass("underline")}`}
                >
                  {statements.map((s, i) => (
                    <span
                      // Index keys: this list IS reorderable, but every child
                      // here is fully controlled from `form.statements` and
                      // holds no local or uncontrolled state of its own, so a
                      // reorder has nothing to carry to the wrong row.
                      key={i}
                      role="tab"
                      tabIndex={0}
                      aria-selected={i === active}
                      aria-controls="stmt-panel"
                      onClick={() => setSelected(i)}
                      onKeyDown={(e) => {
                        if (e.key === "Enter" || e.key === " ") {
                          e.preventDefault();
                          setSelected(i);
                        }
                      }}
                      // The panel tab style (FE-PAGE-9), keyed on `aria-selected`.
                      className={`cursor-pointer font-mono ${tabsTriggerClass("underline")}`}
                    >
                      {/* A cleared name would render an empty, unhittable tab
                          — and clearing it is the first thing an author does
                          to rename one. The placeholder cannot collide with a
                          real name: the rule forbids parentheses. */}
                      {s.name || "(unnamed)"}
                    </span>
                  ))}
                </div>
              ) : (
                <h3 className="text-sm font-medium">SQL</h3>
              )}
              {/* A real `<button>`, so the shell's disabled fieldset reaches it:
                  adding a statement is editing, unlike switching between them. */}
              <Button
                type="button"
                variant="ghost"
                size="sm"
                // No `disabled:pointer-events-auto` here any more. This button
                // used to carry one, to override `Button`'s own
                // `disabled:pointer-events-none` for this instance alone, so
                // that the `title` below could be reached by a pointer at all.
                // `ui/button.tsx` no longer sets that class on ANY button — the
                // same defect was hiding the `title` on fifteen others — so the
                // local override has nothing left to override.
                className="ml-auto shrink-0"
                disabled={atLimit}
                // Says WHY it is unavailable, and says what to do instead.
                // Statements run one after another, so the count multiplies the
                // endpoint's worst-case response time — a fact nothing else on
                // this screen shows.
                title={
                  atLimit
                    ? `An endpoint may declare at most ${MAX_STATEMENTS} statements — they run one after another, so the count multiplies how long a call can take. Fold statements a caller always reads together into one query, or split this into two endpoints.`
                    : undefined
                }
                onClick={addStatement}
              >
                <Plus className="h-4 w-4" aria-hidden="true" />
                Add statement
              </Button>
            </div>

            {several && (
              <div className="flex shrink-0 flex-wrap items-center gap-2 px-1 pt-2">
                <Input
                  aria-label="Statement name"
                  className="h-8 w-48 font-mono text-xs"
                  value={current.name}
                  onChange={(e) => renameActive(e.target.value)}
                />
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  aria-label="Move up"
                  disabled={active === 0}
                  onClick={() => move(-1)}
                >
                  <ChevronUp className="h-4 w-4" aria-hidden="true" />
                </Button>
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  aria-label="Move down"
                  disabled={active === statements.length - 1}
                  onClick={() => move(1)}
                >
                  <ChevronDown className="h-4 w-4" aria-hidden="true" />
                </Button>
                <Button type="button" variant="outline" size="sm" onClick={removeActive}>
                  <Trash2 className="h-4 w-4" aria-hidden="true" />
                  Remove statement
                </Button>
              </div>
            )}
            {nameError && (
              <p role="alert" className="shrink-0 px-1 pt-2 text-xs text-destructive">
                {nameError}
              </p>
            )}

            {/* `overflow-y-auto` is load-bearing, and its partner is the
                `min-h-40` on the editor's own box below. This column holds two
                things: prose that has a fixed natural height, and an editor
                that takes what is left. The panel above it is a fraction of the
                page, so expanding `Database and parameters` takes that height
                away — and every line of prose here is subtracted from the
                editor before anything else is.

                Measured in Chrome on this branch, same endpoint, same viewport
                (1600x1300): with the strip expanded the editor laid out at
                100px in SQL mode and 31px in script mode, against 180px at
                `5774bc77` — and at a 1000px-tall window the script editor was
                FIVE pixels tall and painted outside this panel, over the
                preview below it. The two help paragraphs this section gained
                are the height that went; they are correct and they are not
                what should pay.

                So the editor keeps a floor and the prose above it scrolls
                instead. Not a taller panel or a bigger `minSize`: the strip can
                be expanded at any window height, so any fixed split has a
                window that defeats it. */}
            <div
              id="stmt-panel"
              role={several ? "tabpanel" : undefined}
              className="flex min-h-0 flex-1 flex-col overflow-y-auto pt-2"
            >
              {/* Where this statement's SQL comes from. A real `<Checkbox>`, so
                  the shell's disabled fieldset reaches it: changing the source
                  of a statement is editing, not reading. */}
              <div className="shrink-0 pb-2">
                <label className="flex items-center gap-2 text-sm" htmlFor="stmt-scripted">
                  <Checkbox
                    id="stmt-scripted"
                    checked={scripted}
                    onCheckedChange={(v) => setActiveMode(v === true)}
                  />
                  Build this statement&rsquo;s SQL with a script
                </label>
                {/* Only when it is on, and it says the one thing an author has
                    to know to write a correct script: what comes back is a
                    TEMPLATE, so a parameter still goes through `{{ }}` and is
                    bound on every call. A script that pastes the value into the
                    text instead is refused at publish, by name. */}
                {scripted && (
                  // The RULE is the summary and is on screen unclicked; the
                  // mechanics are behind it. As a standing paragraph this was
                  // 330 characters directly above a Monaco editor, and every
                  // line of prose here is height subtracted from the editor
                  // before anything else is (see the `#stmt-panel` note).
                  <div className="max-w-prose pl-6 pt-1 text-xs">
                    <Caveat
                      summary={
                        <>
                          <span className="font-mono">build_sql(params)</span> returns this
                          statement&rsquo;s SQL as a template
                        </>
                      }
                    >
                      Leave a parameter as <span className="font-mono">{"{{ name }}"}</span> — it is
                      bound on every call, exactly as if you had typed the SQL yourself. A script
                      that puts a parameter&rsquo;s value into the text is refused at publish.
                    </Caveat>
                  </div>
                )}
                {/* What the NEXT click costs, said before it is clicked.
                    A statement has one body, so switching always discards the
                    other one, and the undo below it is an in-page convenience
                    — leaving this section unmounts `Query` and takes the
                    memory with it (see `discarded`). An author who cannot tell
                    "switching back restores this" from "switching back does
                    not" has been promised something this section only
                    sometimes keeps, so the limit travels with the promise
                    rather than being discovered by losing a statement.

                    Only when there is something to lose: a warning over an
                    empty editor is one an author learns to stop reading, and
                    it would be false — nothing is discarded. */}
                {body !== "" && (
                  // Split at the em dash the sentence already had: what the
                  // next click COSTS is the summary, the limit of the undo is
                  // behind it. Three lines above the editor was the audit's
                  // complaint, and here it is not only noise — it is editor
                  // height.
                  //
                  // A native `<details>`, never `ui/collapsible`: switching
                  // statements is already a plain-element `role="tab"` here for
                  // the same reason (the shell's `<fieldset disabled>`), and
                  // reading a caveat on a trashed endpoint is reading.
                  <div className="max-w-prose pl-6 pt-1 text-xs">
                    <Caveat
                      summary={`Switching discards this statement’s ${scripted ? "script" : "SQL"}`}
                    >
                      A statement has one body, not both. Switching back brings it back while you
                      stay in Query; leaving this section or reloading the page does not.
                    </Caveat>
                  </div>
                )}
              </div>

              {/* One editor or the other, never both, and the one that is not
                  in use is REMOVED rather than disabled — `ResponseSection`'s
                  rule, and here it is structural: the state it would be showing
                  does not exist any more, because switching the mode clears it
                  in the same patch. */}
              {/* `min-h-40` rather than `min-h-0`: a flex item with
                  `min-height: 0` may be shrunk to nothing, and this one was
                  (see the note on `#stmt-panel`). 10rem is about ten lines at
                  this font size — enough to read a `build_sql` and the SQL it
                  returns without the box becoming the thing you fight. */}
              <div className="min-h-40 flex-1">
                {scripted ? (
                  <MonacoEditor
                    height="100%"
                    // `python`, which `lib/monaco.ts` already registers for
                    // notebook cells — Starlark is close enough to read in it
                    // and registering an eightieth language would cost the
                    // whole Monaco bundle's diet. Same choice C2 made for the
                    // response transform.
                    defaultLanguage="python"
                    value={current.script}
                    onChange={(v) => setActiveSource({ script: v ?? "" })}
                    // Monaco applies a theme GLOBALLY, so this editor and the
                    // SQL one below must not disagree about light and dark. The
                    // two surface-matched themes are private to `SqlEditor`;
                    // these are the built-ins nearest them.
                    theme={isDark ? "vs-dark" : "vs"}
                    onMount={(editor: MonacoNS.editor.IStandaloneCodeEditor, monaco: Monaco) => {
                      // The same ⌘↵ the SQL editor registers, so the promise
                      // the preview panel prints ("⌘↵ in the editor") is true
                      // in both modes. Guarded through a ref rather than by
                      // not registering: the action is registered once at
                      // mount and an endpoint can be trashed after that.
                      //
                      // An editor ACTION, not `addCommand`, for `SqlEditor`'s
                      // reason: an `addCommand` shortcut answers in whichever
                      // editor has focus and is never removed, so it would
                      // outlive this editor and fire from the SQL one that
                      // replaces it. Disposed when this editor goes (below).
                      scriptRunAction.current?.dispose();
                      scriptRunAction.current = editor.addAction({
                        id: PREVIEW_RUN_ACTION,
                        label: "Run the preview",
                        keybindings: [monaco.KeyMod.CtrlCmd | monaco.KeyCode.Enter],
                        run: () => {
                          if (trashedRef.current) return;
                          preview.current?.run();
                        },
                      });
                    }}
                    options={{
                      minimap: { enabled: false },
                      fontSize: 13,
                      scrollBeyondLastLine: false,
                      padding: { top: 10, bottom: 10 },
                      renderLineHighlight: "line",
                      // Named, because Monaco's own label is a generic "Editor
                      // content" that cannot say WHICH field this is.
                      ariaLabel: "SQL script",
                      // A fieldset does not reach a Monaco instance — it is not
                      // a form control — so a trashed endpoint's script would
                      // be fully editable without these two. `domReadOnly`
                      // alongside `readOnly` for `SqlEditor`'s reason: without
                      // it the underlying textarea still reports itself
                      // editable to assistive tech.
                      readOnly: trashed,
                      domReadOnly: trashed,
                    }}
                  />
                ) : (
                  <SqlEditor
                    value={"sql" in current ? current.sql : ""}
                    onChange={(sql) => setActiveSource({ sql })}
                    ariaLabel="SQL"
                    // See `domReadOnly` above — the fieldset cannot do this.
                    readOnly={trashed}
                    // No schema source on this page — see the module doc.
                    schemaWords={[]}
                    // Ignores the SQL the editor offers (`pickRunSql` would hand
                    // back a SELECTION if there is one): a preview runs the whole
                    // on-screen statement, which is what the note beside the button
                    // promises and what the request body actually carries.
                    //
                    // `undefined` rather than a guarded callback when trashed:
                    // ⌘↵ bypasses both the read-only editor and the fieldset's
                    // disabled button, so this is the only thing stopping a
                    // trashed endpoint from running a preview.
                    onRun={trashed ? undefined : () => preview.current?.run()}
                  />
                )}
              </div>
            </div>
          </div>
        </Panel>
        <PanelSeparator className="my-1 h-1.5 rounded bg-border transition-colors hover:bg-primary/60 active:bg-primary" />
        <Panel defaultSize="45%" minSize="20%">
          {/* The preview route RUNS one `sql` — a statement, not an endpoint —
              so it is handed the statement on screen rather than reading `form`
              for it. It is also handed the other statements' text: a parameter
              is a list for the whole endpoint as soon as ONE statement loops
              over it, so a preview that saw only this statement could go green
              on a definition publish then refuses. They inform that decision and
              nothing else — the server never runs them. `statementName` is
              `undefined` in the common case, which is what keeps the note below
              byte-identical to what it said before this feature. */}
          <PreviewBlock
            ref={preview}
            source={sourceOf(current)}
            siblings={siblings}
            statementName={several ? current.name : undefined}
          />
        </Panel>
      </Group>
    </div>
  );
}
