import type { ReactNode } from "react";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { Button } from "@/components/ui/button";
import type { ResultsPage } from "@/api/sqllab";

export function ResultsGrid({ page, pageIndex, onPage, csvHref, truncated, error, statusLine }: {
  page: ResultsPage | null; pageIndex: number; onPage: (n: number) => void; csvHref: string | null; truncated: boolean;
  error?: string | null; statusLine?: ReactNode;
}) {
  if (error) {
    return (
      <div className="flex h-full flex-col">
        <div className="flex-1 overflow-auto whitespace-pre-wrap p-4 font-mono text-sm text-red-600">{error}</div>
        {statusLine && <div className="border-t p-2 text-sm text-muted-foreground">{statusLine}</div>}
      </div>
    );
  }
  if (!page) return <div className="p-4 text-sm text-muted-foreground">No results yet. Run a query.</div>;
  const totalPages = Math.max(1, Math.ceil(page.row_count / page.page_size));
  return (
    <div className="flex h-full flex-col">
      {truncated && <div className="bg-yellow-100 px-3 py-1 text-xs text-yellow-900">Results were limited to {page.row_count} rows.</div>}
      <div className="flex-1 overflow-auto">
        <Table>
          <TableHeader>
            <TableRow>{page.columns.map((c) => <TableHead key={c.name}>{c.name}</TableHead>)}</TableRow>
          </TableHeader>
          <TableBody>
            {page.rows.map((row, i) => (
              <TableRow key={i}>{row.map((cell, j) => <TableCell key={j}>{cell === null ? "" : String(cell)}</TableCell>)}</TableRow>
            ))}
          </TableBody>
        </Table>
      </div>
      <div className="flex items-center gap-2 border-t p-2 text-sm">
        <Button variant="outline" size="sm" disabled={pageIndex <= 0} onClick={() => onPage(pageIndex - 1)}>Prev</Button>
        <span>Page {pageIndex + 1} / {totalPages}</span>
        <Button variant="outline" size="sm" disabled={pageIndex >= totalPages - 1} onClick={() => onPage(pageIndex + 1)}>Next</Button>
        {statusLine && <span className="text-muted-foreground">{statusLine}</span>}
        <span className="ml-auto">{page.row_count} rows</span>
        {csvHref && <a href={csvHref} download><Button variant="outline" size="sm">Download CSV</Button></a>}
      </div>
    </div>
  );
}
