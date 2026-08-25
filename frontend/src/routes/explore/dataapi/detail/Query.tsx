import { useMemo, useRef, useState } from "react";
import { ChevronDown, ChevronRight, ChevronUp, Plus, Trash2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
// `Separator` is aliased: `@/components/ui/separator` exports that name for the
// visual divider, and react-resizable-panels v4 renamed `PanelResizeHandle`
// into a collision with it. Same alias `components/sqllab/SqlTab.tsx` uses.
import { Group, Panel, Separator as PanelSeparator } from "react-resizable-panels";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { ConfigTable, ConfigTableRow } from "@/components/config/ConfigTable";
import { SqlEditor } from "@/components/sqllab/SqlEditor";
import { useDatabases } from "@/api/databases";
import { useEndpointDetail } from "../EndpointDetail";
import { ParamEditor } from "./ParamEditor";
import { PreviewBlock, type PreviewHandle } from "./PreviewBlock";

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
 * Repeated here rather than read from the API because it is not in the
 * generated model: it is a rule `validate_statements` applies, not a field. The
 * server is still the authority — create, PATCH and publish all refuse a longer
 * list with a 400 — and this constant only decides when the control stops
 * offering to make one. A stale copy costs a toast, not a bad save.
 */
const MAX_STATEMENTS = 16;

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

  const nameError = useMemo(
    () => (several ? statementNameError(current.name, statements.map((s) => s.name), active) : null),
    [several, current.name, statements, active],
  );

  /** Replace the whole list — `statements` is patched whole, never per entry. */
  const setStatements = (next: { name: string; sql: string }[]) =>
    setForm((f) => ({ ...f, statements: next }));

  const editActive = (patch: { name?: string; sql?: string }) =>
    setStatements(statements.map((s, i) => (i === active ? { ...s, ...patch } : s)));

  const atLimit = statements.length >= MAX_STATEMENTS;

  const addStatement = () => {
    // Guarded as well as disabled: the button is the only caller today, and a
    // guard here is what makes that stay true.
    if (atLimit) return;
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
    setStatements(statements.filter((_, i) => i !== active));
    setSelected(Math.max(active - 1, 0));
  };

  return (
    <div className="flex h-full flex-col">
      {/* Kept, and kept on one line: every one of the six sections opens with
          its own `h2`, and this is the only thing that names the section for a
          reader who arrived by URL rather than by clicking the nav. */}
      <div className="flex shrink-0 flex-wrap items-baseline gap-x-3 gap-y-1 pb-2">
        <h2 className="text-lg font-semibold">Query</h2>
        <p className="text-sm text-muted-foreground">
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
          it is one click, and the tab's purpose is writing SQL. */}
      <details className="group shrink-0">
        <summary className="flex cursor-pointer list-none items-center gap-2 py-1 text-sm text-muted-foreground hover:text-foreground">
          <ChevronRight className="h-4 w-4 transition-transform group-open:rotate-90" aria-hidden="true" />
          Database and parameters ({form.params.length})
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
                  {(databases ?? []).map((d) => (
                    <SelectItem key={d.id} value={d.id}>
                      {d.name}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </ConfigTableRow>
          </ConfigTable>

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
                  className="flex min-w-0 flex-1 items-center gap-1 overflow-x-auto"
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
                      className={`cursor-pointer whitespace-nowrap rounded border px-2 py-1 font-mono text-xs transition-colors ${
                        i === active
                          ? "border-primary bg-primary/10 text-foreground"
                          : "border-transparent text-muted-foreground hover:border-border hover:text-foreground"
                      }`}
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
                // `disabled:pointer-events-auto` overrides `Button`'s own
                // `disabled:pointer-events-none` (`ui/button.tsx`), for this
                // instance only — `cva`'s own className arg lands after the
                // base classes and `cn`'s `twMerge` keeps the later one, the
                // same mechanism `button.tsx`'s doc comment already relies on
                // for the focus-ring override.
                //
                // Required for the `title` below to mean anything: at the cap
                // this button IS the only thing on screen saying why, and
                // `pointer-events: none` makes a real browser skip it during
                // hit-testing — confirmed live, `elementFromPoint` at its own
                // centre landed on this row's container, not the button, and
                // Playwright's own hover action timed out with "intercepts
                // pointer events" naming that same container. `disabled` (the
                // HTML attribute, untouched below) already blocks the click and
                // is what the shell's `<fieldset disabled>` reaches — this only
                // restores hover.
                className="ml-auto shrink-0 disabled:pointer-events-auto"
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
                  onChange={(e) => editActive({ name: e.target.value })}
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

            <div id="stmt-panel" role={several ? "tabpanel" : undefined} className="min-h-0 flex-1 pt-2">
              <SqlEditor
                value={current.sql}
                onChange={(sql) => editActive({ sql })}
                // Named, because Monaco's own label is a generic "Editor
                // content" that cannot say WHICH field this is.
                ariaLabel="SQL"
                // The fieldset around this section does not reach a Monaco
                // instance — it is not a form control. Without this line a
                // trashed endpoint's SQL is fully editable.
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
            </div>
          </div>
        </Panel>
        <PanelSeparator className="my-1 h-1.5 rounded bg-border transition-colors hover:bg-primary/60 active:bg-primary" />
        <Panel defaultSize="45%" minSize="20%">
          {/* The preview route takes ONE `sql` — it runs a statement, not an
              endpoint — so it is handed the statement on screen rather than
              reading `form` for it. `statementName` is `undefined` in the
              common case, which is what keeps the note below byte-identical to
              what it said before this feature. */}
          <PreviewBlock ref={preview} sql={current.sql} statementName={several ? current.name : undefined} />
        </Panel>
      </Group>
    </div>
  );
}
