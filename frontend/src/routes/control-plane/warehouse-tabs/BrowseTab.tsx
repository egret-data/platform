import { useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  Check,
  ChevronDown,
  ChevronRight,
  Columns3,
  Database,
  FileText,
  History,
  RotateCcw,
  Search,
  Shield,
  ShieldCheck,
  Table2,
  Tag as TagIcon,
  Trash2,
} from "lucide-react";

import {
  type Column,
  type DeletedTabular,
  type Namespace,
  type SearchResult,
  type TableDetail,
  type Tabular,
  listDeletedTabulars,
  listNamespaces,
  listTabulars,
  loadTableDetail,
  loadViewColumns,
  searchTabulars,
  setProtection,
  undropTabular,
} from "@/api/controlPlane";
import { ApiError } from "@/api/client";
import { CatalogTagDialog } from "@/components/access/CatalogTagEditor";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { CopyButton } from "./shared";

// ── Browse (namespaces / tables / schema) ────────────────────────────────────

export function BrowseTab({ warehouseId }: { warehouseId: string }) {
  const [selectedNs, setSelectedNs] = useState<string | null>(null);
  const [selectedTabular, setSelectedTabular] = useState<Tabular | null>(null);
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const [deletedOpen, setDeletedOpen] = useState(false);
  const [protectError, setProtectError] = useState<string | null>(null);

  const namespaces = useQuery<Namespace[], ApiError>({
    queryKey: ["cp", "browse", warehouseId, "ns"],
    queryFn: () => listNamespaces(warehouseId),
    retry: false,
  });

  function toggle(label: string) {
    setExpanded((prev) => {
      const next = new Set(prev);
      if (next.has(label)) next.delete(label);
      else next.add(label);
      return next;
    });
  }

  const list = namespaces.data ?? [];
  const [search, setSearch] = useState("");
  const q = search.trim();
  const results = useQuery<SearchResult[], ApiError>({
    queryKey: ["cp", "browse", warehouseId, "search", q],
    queryFn: () => searchTabulars(warehouseId, q),
    enabled: q.length > 0,
    retry: false,
  });

  return (
    <div className="space-y-4">
      {protectError && (
        <Alert variant="destructive">
          <AlertDescription>{protectError}</AlertDescription>
        </Alert>
      )}
      <div className="grid gap-4 lg:grid-cols-[300px_minmax(0,1fr)]">
      <Card className="overflow-hidden">
        <div className="flex items-center gap-2 border-b bg-muted/30 px-4 py-2.5">
          <Database className="h-4 w-4 text-muted-foreground" />
          <span className="text-sm font-medium">Namespaces</span>
          <span className="ml-auto text-xs text-muted-foreground">{list.length}</span>
          <Button
            variant="ghost"
            size="sm"
            className="h-6 gap-1 px-1.5 text-xs text-muted-foreground"
            onClick={() => setDeletedOpen(true)}
            title="Recover recently-dropped tables"
          >
            <Trash2 className="h-3.5 w-3.5" />
            Recycle bin
          </Button>
        </div>
        <div className="border-b p-2">
          <div className="relative">
            <Search className="absolute left-2 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-muted-foreground" />
            <Input
              className="h-8 pl-7 text-sm"
              placeholder="Search tables & views…"
              value={search}
              onChange={(e) => setSearch(e.target.value)}
            />
          </div>
        </div>
        <div className="max-h-[520px] overflow-y-auto py-1">
          {q ? (
            results.isLoading ? (
              <div className="p-2">
                <ListSkeleton rows={5} cols={1} />
              </div>
            ) : (results.data ?? []).length === 0 ? (
              <BrowseEmpty icon={Search}>No matches for “{q}”.</BrowseEmpty>
            ) : (
              (results.data ?? []).map((r, i) => {
                const nsLabel = r.namespace.join(".");
                return (
                  <button
                    key={`${nsLabel}.${r.name}.${i}`}
                    type="button"
                    onClick={() => {
                      setSelectedNs(nsLabel);
                      setSelectedTabular({ name: r.name, kind: r.kind, id: null, protected: false });
                    }}
                    className="flex w-full items-center gap-2 px-3 py-1.5 text-left text-sm hover:bg-muted"
                  >
                    <Table2 className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
                    <span className="flex-1 truncate">
                      <span className="text-muted-foreground">{nsLabel}.</span>
                      {r.name}
                    </span>
                    {r.kind === "view" && (
                      <Badge variant="outline" className="text-[10px] uppercase">view</Badge>
                    )}
                  </button>
                );
              })
            )
          ) : namespaces.isError ? (
            <BrowseEmpty icon={Database}>
              <span className="text-destructive">
                {/* `ApiError.message` for a 403 is `Error::Forbidden`'s Display, the
                    bare word "forbidden". This route is gated `catalog_registry:read`
                    (SP3c-2, Task 7 retired `warehouse:read`), named here instead. */}
                {namespaces.error?.status === 403
                  ? "Couldn't load namespaces: this needs the catalog_registry:read permission. A platform administrator can grant it."
                  : (namespaces.error?.message ?? "Couldn't load namespaces.")}
              </span>
            </BrowseEmpty>
          ) : namespaces.isLoading ? (
            <div className="p-2">
              <ListSkeleton rows={6} cols={1} />
            </div>
          ) : list.length === 0 ? (
            <BrowseEmpty icon={Database}>This warehouse has no namespaces yet.</BrowseEmpty>
          ) : (
            list.map((ns) => {
              const label = ns.parts.join(".");
              return (
                <NsNode
                  key={label}
                  warehouseId={warehouseId}
                  ns={ns}
                  label={label}
                  open={expanded.has(label)}
                  onToggle={() => toggle(label)}
                  selectedTable={selectedNs === label ? (selectedTabular?.name ?? null) : null}
                  onSelectTable={(t) => {
                    setSelectedNs(label);
                    setSelectedTabular(t);
                  }}
                  onProtectError={setProtectError}
                />
              );
            })
          )}
        </div>
      </Card>

      <Card className="overflow-hidden">
        {selectedNs && selectedTabular ? (
          <TableDetailView
            warehouseId={warehouseId}
            namespace={selectedNs}
            tabular={selectedTabular}
            onProtectError={setProtectError}
          />
        ) : (
          <div className="flex min-h-[320px] flex-col items-center justify-center gap-2 p-6 text-sm text-muted-foreground">
            <Table2 className="h-8 w-8 opacity-30" />
            <span>Select a table or view to see its details.</span>
          </div>
        )}
      </Card>

      <DeletedTablesDialog warehouseId={warehouseId} open={deletedOpen} onOpenChange={setDeletedOpen} />
      </div>
    </div>
  );
}

/** Shield toggle that protects/unprotects a namespace/table/view from drops. */
function ProtectToggle({
  warehouseId,
  target,
  id,
  isProtected,
  className,
  onError,
}: {
  warehouseId: string;
  target: "table" | "view" | "namespace";
  id: string | null;
  isProtected: boolean;
  className?: string;
  onError?: (m: string | null) => void;
}) {
  const queryClient = useQueryClient();
  const mut = useMutation<{ protected: boolean }, ApiError, boolean>({
    mutationFn: (next) => setProtection(warehouseId, target, id!, next),
    onSuccess: () => {
      onError?.(null);
      queryClient.invalidateQueries({ queryKey: ["cp", "browse", warehouseId] });
    },
    onError: (e) => onError?.(e.message ?? "Failed to change protection."),
  });
  if (!id) return null;
  return (
    <button
      type="button"
      disabled={mut.isPending}
      onClick={(e) => {
        e.stopPropagation();
        mut.mutate(!isProtected);
      }}
      title={isProtected ? "Protected from drops — click to unprotect" : "Protect from drops"}
      aria-label={isProtected ? "Unprotect" : "Protect"}
      className={`shrink-0 rounded p-1 transition-colors ${
        isProtected
          ? "text-highlight hover:text-highlight/80"
          : "text-muted-foreground/40 opacity-0 hover:text-foreground group-hover:opacity-100"
      } ${className ?? ""}`}
    >
      {isProtected ? <ShieldCheck className="h-4 w-4" /> : <Shield className="h-4 w-4" />}
    </button>
  );
}

function NsNode({
  warehouseId,
  ns,
  label,
  open,
  onToggle,
  selectedTable,
  onSelectTable,
  onProtectError,
}: {
  warehouseId: string;
  ns: Namespace;
  label: string;
  open: boolean;
  onToggle: () => void;
  selectedTable: string | null;
  onSelectTable: (t: Tabular) => void;
  onProtectError?: (m: string | null) => void;
}) {
  const tabulars = useQuery<Tabular[], ApiError>({
    queryKey: ["cp", "browse", warehouseId, "tables", label],
    queryFn: () => listTabulars(warehouseId, label),
    enabled: open,
    retry: false,
  });
  return (
    <div>
      <div className="group flex w-full items-center pr-2 transition-colors hover:bg-muted">
        <button
          type="button"
          onClick={onToggle}
          className="flex min-w-0 flex-1 items-center gap-1.5 px-3 py-2 text-left text-sm"
        >
          {open ? (
            <ChevronDown className="h-4 w-4 shrink-0 text-muted-foreground" />
          ) : (
            <ChevronRight className="h-4 w-4 shrink-0 text-muted-foreground" />
          )}
          <Database className="h-4 w-4 shrink-0 text-muted-foreground" />
          <span className="truncate">{label}</span>
        </button>
        <ProtectToggle warehouseId={warehouseId} target="namespace" id={ns.id} isProtected={ns.protected} onError={onProtectError} />
      </div>
      {open && (
        <div className="pb-1">
          {tabulars.isLoading ? (
            <div className="px-9 py-1.5">
              <ListSkeleton rows={2} cols={1} />
            </div>
          ) : (tabulars.data ?? []).length === 0 ? (
            <p className="px-9 py-1.5 text-xs text-muted-foreground">No tables.</p>
          ) : (
            (tabulars.data ?? []).map((t) => {
              const active = selectedTable === t.name;
              return (
                <button
                  key={t.name}
                  type="button"
                  onClick={() => onSelectTable(t)}
                  className={`flex w-full items-center gap-2 border-l-2 py-1.5 pl-9 pr-3 text-left text-sm transition-colors ${
                    active
                      ? "border-primary bg-primary/10 font-medium text-primary"
                      : "border-transparent hover:bg-muted"
                  }`}
                >
                  <Table2 className={`h-3.5 w-3.5 shrink-0 ${active ? "text-primary" : "text-muted-foreground"}`} />
                  <span className="flex-1 truncate">{t.name}</span>
                  {t.protected && (
                    <ShieldCheck className="h-3.5 w-3.5 shrink-0 text-highlight" aria-label="protected" />
                  )}
                  {t.kind === "view" && (
                    <Badge variant="outline" className="text-[10px] uppercase">view</Badge>
                  )}
                </button>
              );
            })
          )}
        </div>
      )}
    </div>
  );
}

function TableDetailView({
  warehouseId,
  namespace,
  tabular,
  onProtectError,
}: {
  warehouseId: string;
  namespace: string;
  tabular: Tabular;
  onProtectError?: (m: string | null) => void;
}) {
  const isView = tabular.kind === "view";
  const table = tabular.name;
  const detail = useQuery<TableDetail, ApiError>({
    queryKey: ["cp", "browse", warehouseId, "detail", namespace, table],
    queryFn: () => loadTableDetail(warehouseId, namespace, table),
    enabled: !isView,
    retry: false,
  });
  const viewCols = useQuery<Column[], ApiError>({
    queryKey: ["cp", "browse", warehouseId, "view", namespace, table],
    queryFn: () => loadViewColumns(warehouseId, namespace, table),
    enabled: isView,
    retry: false,
  });
  const d = detail.data;
  const transform = (t: string, col: string) => (t === "identity" ? col : `${t}(${col})`);
  const [tagsDialog, setTagsDialog] = useState<{ open: boolean; column?: string }>({ open: false });

  return (
    <div>
      <div className="border-b p-5">
        <div className="flex items-center gap-1 text-xs text-muted-foreground">
          <span>{namespace}</span>
          <ChevronRight className="h-3 w-3" />
          <span className="font-medium text-foreground">{table}</span>
        </div>
        <div className="mt-1 flex items-center gap-2">
          <h2 className="flex items-center gap-2 text-lg font-semibold">
            <Table2 className="h-5 w-5 text-primary" />
            {table}
          </h2>
          {isView && (
            <Badge variant="outline" className="uppercase">view</Badge>
          )}
          <div className="group">
            <ProtectToggle
              warehouseId={warehouseId}
              target={tabular.kind}
              id={tabular.id}
              isProtected={tabular.protected}
              className="opacity-100"
              onError={onProtectError}
            />
          </div>
          {tabular.protected && <span className="text-xs text-highlight">Protected from drops</span>}
          <Button
            variant="ghost"
            size="sm"
            className="h-7 gap-1 px-2 text-muted-foreground"
            onClick={() => setTagsDialog({ open: true, column: undefined })}
          >
            <TagIcon className="h-3.5 w-3.5" />
            Tags
          </Button>
        </div>
        {!isView && d && (
          <div className="mt-3 flex flex-wrap items-center gap-x-6 gap-y-2 text-sm">
            <Fact2 label="Format" value={`v${d.formatVersion}`} />
            <Fact2 label="Columns" value={String(d.columns.length)} />
            <Fact2 label="Snapshots" value={String(d.snapshotCount)} />
            <Fact2 label="Updated" value={new Date(d.lastUpdatedMs).toLocaleString()} />
            <div className="flex min-w-0 items-center gap-1.5 text-muted-foreground">
              <span className="text-xs uppercase tracking-wide">Location</span>
              <code className="max-w-[260px] truncate font-mono text-xs text-foreground">{d.location}</code>
              <CopyButton value={d.location} />
            </div>
          </div>
        )}
      </div>

      {isView ? (
        <div className="p-5">
          {viewCols.isLoading ? (
            <ListSkeleton rows={5} cols={3} />
          ) : viewCols.data ? (
            <Table>
              <TableHeader>
                <TableRow>
                  <TableHead>Name</TableHead>
                  <TableHead>Type</TableHead>
                  <TableHead className="text-right">Required</TableHead>
                  <TableHead className="text-right">
                    <span className="sr-only">Tags</span>
                  </TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {viewCols.data.map((col) => (
                  <TableRow key={col.name}>
                    <TableCell className="font-mono">{col.name}</TableCell>
                    <TableCell>
                      <Badge variant="secondary" className="font-mono font-normal">{col.type}</Badge>
                    </TableCell>
                    <TableCell className="text-right">
                      {col.required ? (
                        <Check className="ml-auto h-4 w-4 text-primary" />
                      ) : (
                        <span className="text-muted-foreground">—</span>
                      )}
                    </TableCell>
                    <TableCell className="text-right">
                      <Button
                        variant="ghost"
                        size="sm"
                        className="h-7 gap-1 px-2 text-muted-foreground"
                        onClick={() => setTagsDialog({ open: true, column: col.name })}
                        aria-label={`Tags for ${col.name}`}
                      >
                        <TagIcon className="h-3.5 w-3.5" />
                      </Button>
                    </TableCell>
                  </TableRow>
                ))}
              </TableBody>
            </Table>
          ) : (
            <p className="text-center text-sm text-destructive">Failed to load view.</p>
          )}
        </div>
      ) : detail.isLoading ? (
        <div className="p-5">
          <ListSkeleton rows={6} cols={3} />
        </div>
      ) : d ? (
        <Tabs defaultValue="schema" className="p-5">
          <TabsList variant="underline">
            <TabsTrigger value="schema">
              <Columns3 className="h-4 w-4" />
              Schema
            </TabsTrigger>
            <TabsTrigger value="properties">
              <FileText className="h-4 w-4" />
              Properties
            </TabsTrigger>
            <TabsTrigger value="snapshots">
              <History className="h-4 w-4" />
              Snapshots
            </TabsTrigger>
          </TabsList>

          <TabsContent value="schema" className="mt-4 space-y-4">
            <Table>
              <TableHeader>
                <TableRow>
                  <TableHead>Name</TableHead>
                  <TableHead>Type</TableHead>
                  <TableHead className="text-right">Required</TableHead>
                  <TableHead className="text-right">
                    <span className="sr-only">Tags</span>
                  </TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {d.columns.map((col) => (
                  <TableRow key={col.name}>
                    <TableCell className="font-mono">{col.name}</TableCell>
                    <TableCell>
                      <Badge variant="secondary" className="font-mono font-normal">{col.type}</Badge>
                    </TableCell>
                    <TableCell className="text-right">
                      {col.required ? (
                        <Check className="ml-auto h-4 w-4 text-primary" />
                      ) : (
                        <span className="text-muted-foreground">—</span>
                      )}
                    </TableCell>
                    <TableCell className="text-right">
                      <Button
                        variant="ghost"
                        size="sm"
                        className="h-7 gap-1 px-2 text-muted-foreground"
                        onClick={() => setTagsDialog({ open: true, column: col.name })}
                        aria-label={`Tags for ${col.name}`}
                      >
                        <TagIcon className="h-3.5 w-3.5" />
                      </Button>
                    </TableCell>
                  </TableRow>
                ))}
              </TableBody>
            </Table>
            {d.partitionFields.length > 0 && (
              <div className="flex flex-wrap items-center gap-2 text-sm">
                <span className="text-muted-foreground">Partitioned by:</span>
                {d.partitionFields.map((pf) => (
                  <Badge key={pf.name} variant="outline">{transform(pf.transform, pf.sourceColumn)}</Badge>
                ))}
              </div>
            )}
            {d.sortFields.length > 0 && (
              <div className="flex flex-wrap items-center gap-2 text-sm">
                <span className="text-muted-foreground">Sorted by:</span>
                {d.sortFields.map((sf, i) => (
                  <Badge key={i} variant="outline">
                    {transform(sf.transform, sf.column)} {sf.direction}
                  </Badge>
                ))}
              </div>
            )}
          </TabsContent>

          <TabsContent value="properties" className="mt-4">
            {Object.keys(d.properties).length === 0 ? (
              <p className="py-6 text-center text-sm text-muted-foreground">No table properties.</p>
            ) : (
              <Table>
                <TableHeader>
                  <TableRow>
                    <TableHead>Key</TableHead>
                    <TableHead>Value</TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {Object.entries(d.properties).map(([k, v]) => (
                    <TableRow key={k}>
                      <TableCell className="font-mono text-muted-foreground">{k}</TableCell>
                      <TableCell className="break-all font-mono">{v}</TableCell>
                    </TableRow>
                  ))}
                </TableBody>
              </Table>
            )}
          </TabsContent>

          <TabsContent value="snapshots" className="mt-4">
            {d.snapshots.length === 0 ? (
              <p className="py-6 text-center text-sm text-muted-foreground">No snapshots yet.</p>
            ) : (
              <Table>
                <TableHeader>
                  <TableRow>
                    <TableHead>Snapshot</TableHead>
                    <TableHead>Operation</TableHead>
                    <TableHead>When</TableHead>
                    <TableHead className="text-right">Records</TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {d.snapshots.map((snap) => (
                    <TableRow key={snap.snapshotId}>
                      <TableCell className="font-mono text-xs">
                        {snap.snapshotId}
                        {snap.snapshotId === d.currentSnapshotId && (
                          <Badge variant="secondary" className="ml-2 text-[10px]">current</Badge>
                        )}
                      </TableCell>
                      <TableCell>
                        <Badge variant="outline">{snap.operation}</Badge>
                      </TableCell>
                      <TableCell className="text-sm text-muted-foreground">
                        {new Date(snap.timestampMs).toLocaleString()}
                      </TableCell>
                      <TableCell className="text-right font-mono text-xs tabular-nums">
                        {snap.summary["added-records"] ?? snap.summary["total-records"] ?? "—"}
                      </TableCell>
                    </TableRow>
                  ))}
                </TableBody>
              </Table>
            )}
          </TabsContent>
        </Tabs>
      ) : (
        <p className="p-6 text-center text-sm text-destructive">Failed to load table detail.</p>
      )}

      <CatalogTagDialog
        warehouseId={warehouseId}
        schema={namespace}
        table={table}
        column={tagsDialog.column}
        open={tagsDialog.open}
        onOpenChange={(open) => setTagsDialog((prev) => ({ ...prev, open }))}
      />
    </div>
  );
}

function DeletedTablesDialog({
  warehouseId,
  open,
  onOpenChange,
}: {
  warehouseId: string;
  open: boolean;
  onOpenChange: (o: boolean) => void;
}) {
  const queryClient = useQueryClient();
  const deleted = useQuery<DeletedTabular[], ApiError>({
    queryKey: ["cp", "browse", warehouseId, "deleted"],
    queryFn: () => listDeletedTabulars(warehouseId),
    enabled: open,
    retry: false,
  });
  const undrop = useMutation<void, ApiError, DeletedTabular>({
    mutationFn: (d) => undropTabular(warehouseId, d.kind, d.id),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ["cp", "browse", warehouseId] }),
  });
  const rows = deleted.data ?? [];

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-2xl">
        <DialogHeader>
          <DialogTitle>Recycle bin</DialogTitle>
          <DialogDescription>
            Dropped tables and views are soft-deleted and purged after a retention window. Recover
            one before it expires.
          </DialogDescription>
        </DialogHeader>
        <div className="max-h-[420px] overflow-y-auto">
          {deleted.isLoading ? (
            <ListSkeleton rows={4} cols={3} />
          ) : rows.length === 0 ? (
            <p className="py-8 text-center text-sm text-muted-foreground">Nothing to recover.</p>
          ) : (
            <Table>
              <TableHeader>
                <TableRow>
                  <TableHead>Name</TableHead>
                  <TableHead>Dropped</TableHead>
                  <TableHead>Purges</TableHead>
                  <TableHead className="text-right">
                    <span className="sr-only">Recover</span>
                  </TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {rows.map((d) => (
                  <TableRow key={d.id}>
                    <TableCell>
                      <div className="flex items-center gap-2">
                        <Table2 className="h-4 w-4 text-muted-foreground" />
                        <span className="font-mono text-sm">
                          {d.namespace.join(".")}.{d.name}
                        </span>
                        {d.kind === "view" && (
                          <Badge variant="outline" className="text-[10px] uppercase">view</Badge>
                        )}
                      </div>
                    </TableCell>
                    <TableCell className="text-sm text-muted-foreground">
                      {new Date(d.deletedAt).toLocaleString()}
                    </TableCell>
                    <TableCell className="text-sm text-muted-foreground">
                      {new Date(d.expiration).toLocaleDateString()}
                    </TableCell>
                    <TableCell className="text-right">
                      <Button
                        variant="outline"
                        size="sm"
                        disabled={undrop.isPending}
                        onClick={() => undrop.mutate(d)}
                      >
                        <RotateCcw className="h-4 w-4" />
                        Undrop
                      </Button>
                    </TableCell>
                  </TableRow>
                ))}
              </TableBody>
            </Table>
          )}
        </div>
      </DialogContent>
    </Dialog>
  );
}

function Fact2({ label, value }: { label: string; value: string }) {
  return (
    <div className="flex items-center gap-1.5 text-muted-foreground">
      <span className="text-xs uppercase tracking-wide">{label}</span>
      <span className="text-foreground">{value}</span>
    </div>
  );
}

function BrowseEmpty({ icon: Icon, children }: { icon?: typeof Database; children: React.ReactNode }) {
  return (
    <div className="flex flex-col items-center gap-2 px-4 py-10 text-center text-sm text-muted-foreground">
      {Icon && <Icon className="h-6 w-6 opacity-40" />}
      <span>{children}</span>
    </div>
  );
}

