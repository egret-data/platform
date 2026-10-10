import { useMemo } from "react";
import { Skeleton } from "@/components/ui/skeleton";
import type { EditorContext } from "@/components/workflows/editor/EditorContext";
import { flowDoc, typeDoc, type TypeDoc } from "@/components/workflows/editor/flowSchema";
import { Markdown } from "@/components/workflows/editor/Markdown";
import { SchemaUnavailable } from "@/components/workflows/editor/SchemaUnavailable";

/**
 * The Docs tab (spec §5.2): the documentation of the task or trigger under the
 * cursor, from the same schema `monaco-yaml` completes from, or the flow's own
 * outside any. The cursor reaches it 150 ms behind the editor
 * (`EditorContext.cursorOffset`).
 */
export function DocsTab({ ctx }: { ctx: EditorContext }) {
  const { schema, yaml, cursorOffset } = ctx;
  const spelled = useMemo(
    () => (cursorOffset === null ? null : yaml.typeAt(cursorOffset)),
    [yaml, cursorOffset],
  );

  if (schema.status === "error") return <SchemaUnavailable error={schema.error} retry={schema.retry} />;
  if (schema.status === "loading" || !schema.schema) {
    return (
      <div role="status" className="flex flex-col gap-2 p-3">
        <span className="sr-only">Loading the documentation…</span>
        <Skeleton className="h-5 w-2/3" />
        <Skeleton className="h-4 w-full" />
        <Skeleton className="h-4 w-5/6" />
      </div>
    );
  }

  const doc = spelled ? typeDoc(schema.schema, spelled) : null;
  return (
    <div className="flex flex-col gap-4 p-3 text-sm">
      {spelled && !doc && (
        <p className="text-muted-foreground">
          No documentation for <code className="rounded bg-muted px-1 font-mono">{spelled}</code>. Showing the flow&apos;s documentation.
        </p>
      )}
      <TypeDocView doc={doc ?? flowDoc(schema.schema)} />
    </div>
  );
}

function TypeDocView({ doc }: { doc: TypeDoc }) {
  return (
    <>
      <div className="flex flex-col gap-1">
        <h2 className="font-mono text-base font-semibold break-all text-foreground">{doc.name}</h2>
        {doc.spelledAs && (
          <p className="text-muted-foreground">
            Alias: written here as <code className="rounded bg-muted px-1 font-mono">{doc.spelledAs}</code>.
          </p>
        )}
        {doc.title && <p className="text-foreground">{doc.title}</p>}
      </div>
      {doc.description && <Markdown>{doc.description}</Markdown>}

      <section className="flex flex-col gap-1">
        <h3 className="font-semibold text-foreground">Properties</h3>
        <ul role="list" aria-label="Properties" className="flex flex-col divide-y divide-border">
          {doc.properties.map((p) => (
            <DocItem
              key={p.name}
              name={p.name}
              type={p.type}
              required={p.required}
              defaultValue={p.default}
              description={p.description}
            />
          ))}
        </ul>
      </section>

      {doc.outputs && (
        <section className="flex flex-col gap-1">
          <h3 className="font-semibold text-foreground">Outputs</h3>
          {doc.outputs.length === 0 ? (
            <p className="text-muted-foreground">This type has no documented outputs.</p>
          ) : (
            <ul role="list" aria-label="Outputs" className="flex flex-col divide-y divide-border">
              {doc.outputs.map((o) => (
                <DocItem key={o.name} name={o.name} type={o.type} description={o.description} />
              ))}
            </ul>
          )}
        </section>
      )}
    </>
  );
}

/**
 * One property or output, stacked to fit a narrow panel (W7a Task 8 fix round 2):
 * the name, its type and whether it is required on the first line, the default
 * under it, and the description wrapping below. A five-column table did not fit
 * the default 40% panel at 1440px: the Description column was off-screen, as in
 * Kestra's docs panel, which lists properties the same way.
 *
 * Each part carries `data-part`, which the tests read.
 */
function DocItem({
  name,
  type,
  required,
  defaultValue,
  description,
}: {
  name: string;
  type: string;
  /** Undefined for an output, which is neither required nor optional. */
  required?: boolean;
  defaultValue?: string;
  description: string;
}) {
  return (
    <li className="flex min-w-0 flex-col gap-0.5 py-2">
      <div className="flex min-w-0 flex-wrap items-baseline gap-x-2 gap-y-0.5">
        <code data-part="name" className="font-mono font-medium break-all text-foreground">{name}</code>
        <span data-part="type" className="font-mono text-xs break-all text-muted-foreground">{type}</span>
        {required !== undefined && (
          <span
            data-part="required"
            className={required ? "text-xs font-medium text-foreground" : "text-xs text-muted-foreground"}
          >
            {required ? "Required" : "Optional"}
          </span>
        )}
      </div>
      {defaultValue ? (
        <p className="text-xs text-muted-foreground">
          Default: <code data-part="default" className="rounded bg-muted px-1 font-mono break-all text-foreground">{defaultValue}</code>
        </p>
      ) : null}
      {description ? (
        <div data-part="description" className="min-w-0 text-foreground [overflow-wrap:anywhere] [&_code]:rounded [&_code]:bg-muted [&_code]:px-1">
          <Markdown inline>{description}</Markdown>
        </div>
      ) : null}
    </li>
  );
}
