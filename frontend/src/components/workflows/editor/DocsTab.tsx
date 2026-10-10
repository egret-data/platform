import { useMemo } from "react";
import { Skeleton } from "@/components/ui/skeleton";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import type { EditorContext } from "@/components/workflows/editor/EditorContext";
import { flowDoc, typeDoc, type TypeDoc } from "@/components/workflows/editor/flowSchema";
import { Markdown } from "@/components/workflows/editor/Markdown";
import { SchemaUnavailable } from "@/components/workflows/editor/SchemaUnavailable";
import { typeAt } from "@/components/workflows/editor/yamlPath";

/**
 * The Docs tab (spec §5.2): the documentation of the task or trigger under the
 * cursor, from the same schema `monaco-yaml` completes from, or the flow's own
 * outside any. The cursor reaches it 150 ms behind the editor
 * (`EditorContext.cursorOffset`).
 */
export function DocsTab({ ctx }: { ctx: EditorContext }) {
  const { schema, source, cursorOffset } = ctx;
  const spelled = useMemo(
    () => (cursorOffset === null ? null : typeAt(source, cursorOffset)),
    [source, cursorOffset],
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
          No documentation for <code className="rounded bg-muted px-1 font-mono">{spelled}</code>. This is the flow&apos;s.
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
        <Table aria-label="Properties">
          <TableHeader>
            <TableRow>
              <TableHead>Name</TableHead>
              <TableHead>Type</TableHead>
              <TableHead>Required</TableHead>
              <TableHead>Default</TableHead>
              <TableHead>Description</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {doc.properties.map((p) => (
              <TableRow key={p.name}>
                <TableCell className="align-top font-mono">{p.name}</TableCell>
                <TableCell className="align-top font-mono text-muted-foreground">{p.type}</TableCell>
                <TableCell className="align-top">{p.required ? "Yes" : "No"}</TableCell>
                <TableCell className="align-top font-mono">{p.default}</TableCell>
                <TableCell className="align-top whitespace-normal">
                  <Markdown inline>{p.description}</Markdown>
                </TableCell>
              </TableRow>
            ))}
          </TableBody>
        </Table>
      </section>

      {doc.outputs && (
        <section className="flex flex-col gap-1">
          <h3 className="font-semibold text-foreground">Outputs</h3>
          {doc.outputs.length === 0 ? (
            <p className="text-muted-foreground">This type has no documented outputs.</p>
          ) : (
            <Table aria-label="Outputs">
              <TableHeader>
                <TableRow>
                  <TableHead>Name</TableHead>
                  <TableHead>Type</TableHead>
                  <TableHead>Description</TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {doc.outputs.map((o) => (
                  <TableRow key={o.name}>
                    <TableCell className="align-top font-mono">{o.name}</TableCell>
                    <TableCell className="align-top font-mono text-muted-foreground">{o.type}</TableCell>
                    <TableCell className="align-top whitespace-normal">
                      <Markdown inline>{o.description}</Markdown>
                    </TableCell>
                  </TableRow>
                ))}
              </TableBody>
            </Table>
          )}
        </section>
      )}
    </>
  );
}
