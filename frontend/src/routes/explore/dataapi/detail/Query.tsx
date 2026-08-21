import { useRef } from "react";
import { ChevronRight } from "lucide-react";
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
 */
export default function Query() {
  const { ws, form, setForm, trashed } = useEndpointDetail();
  const { data: databases } = useDatabases(ws);
  // The editor's ⌘↵ has to reach the preview's button without the preview's
  // state (test values, rows, error) being lifted up here to be passed back
  // down — see `PreviewHandle`.
  const preview = useRef<PreviewHandle>(null);

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
              <h3 className="text-sm font-medium">SQL</h3>
            </div>
            <div className="min-h-0 flex-1 pt-2">
              <SqlEditor
                value={form.sql}
                onChange={(sql) => setForm((f) => ({ ...f, sql }))}
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
                // on-screen draft, which is what the note beside the button
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
          <PreviewBlock ref={preview} />
        </Panel>
      </Group>
    </div>
  );
}
