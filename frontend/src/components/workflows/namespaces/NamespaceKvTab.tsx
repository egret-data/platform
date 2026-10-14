import { useMemo, useState } from "react";
import { useSearchParams } from "react-router-dom";
import { useQueryClient } from "@tanstack/react-query";
import { KeyRound, Layers, Plus, RotateCw, Trash2 } from "lucide-react";
import { toast } from "sonner";
import { ApiError } from "@/api/client";
import type { KvEntryDto } from "@/api/generated/model";
import { useWorkflowListKv, workflowDeleteKvValue, workflowDeleteKvValues } from "@/api/generated/workflow/workflow";
import { Alert, AlertDescription } from "@/components/ui/alert";
import {
  AlertDialog,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { EmptyState } from "@/components/explore/EmptyState";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { DisabledReasons, ReasonButton } from "@/components/workflows/ReasonButton";
import type { Enablement } from "@/components/workflows/draftState";
import { InheritedKvDrawer } from "@/components/workflows/namespaces/kv/InheritedKvDrawer";
import { KvEditDrawer } from "@/components/workflows/namespaces/kv/KvEditDrawer";
import { KvTable, parseKvSort, sortKvRows, type KvSort } from "@/components/workflows/namespaces/kv/KvTable";
import { KvValueDrawer } from "@/components/workflows/namespaces/kv/KvValueDrawer";
import { kvErrorMessage } from "@/components/workflows/namespaces/kv/kvApi";
import { kvExpression } from "@/components/workflows/namespaces/kv/kvValue";
import { MANAGE_REASON } from "@/components/workflows/namespaces/kv/kvCopy";
import type { NamespaceTabProps } from "@/components/workflows/namespaces/namespaceTabs";
import { workflowsKeys } from "@/routes/workflows/queryKeys";

/** The bulk route takes at most 1,000 keys a request (W9c `MAX_BULK_DELETE_KEYS`). */
const BULK_LIMIT = 1000;

type Drawer =
  | { kind: "none" }
  | { kind: "view"; entry: KvEntryDto }
  | { kind: "edit"; entry: KvEntryDto }
  | { kind: "create" }
  | { kind: "inherited" };

/**
 * THE KV TAB (W7d spec §4.4), Kestra's KV page for one namespace over W9c's
 * routes: a table of the live keys (sortable, filtered by key; both in the
 * URL, FE-PAGE-2), each key's value in a drawer, a drawer to create and edit,
 * delete one or many, and the keys inherited from the parents.
 *
 * Reads need `workflow:read`, which the frame has already proven. Writes need
 * `workflow:manage`: without it (`canManage`, from the namespace detail) every
 * write is disabled with its reason (spec §4.5, FE-ACTION-4), and a 403 the
 * server still answers is shown as a refusal with its message.
 *
 * `NamespaceDetail.tsx` reaches this module through `lazy()` only
 * (`scripts/assert-workflows-lazy.mjs`).
 */
export default function NamespaceKvTab({ ws, namespace, canManage }: NamespaceTabProps) {
  const queryClient = useQueryClient();
  const query = useWorkflowListKv(ws, namespace, { query: { queryKey: workflowsKeys.kv(ws, namespace) } });
  const rows = query.data?.status === 200 ? query.data.data : undefined;

  const [params, setParams] = useSearchParams();
  const filter = params.get("key") ?? "";
  const sort = parseKvSort(params.get("sort"), params.get("dir"));
  const setParam = (changes: Record<string, string | null>) =>
    setParams(
      (p) => {
        for (const [k, v] of Object.entries(changes)) {
          if (v) p.set(k, v);
          else p.delete(k);
        }
        return p;
      },
      { replace: true },
    );
  const onSort = (column: KvSort["column"]) => {
    const dir = sort.column === column && sort.dir === "asc" ? "desc" : "asc";
    setParam(column === "key" && dir === "asc" ? { sort: null, dir: null } : { sort: column, dir: dir === "asc" ? null : "desc" });
  };

  const shown = useMemo(() => {
    const needle = filter.trim().toLowerCase();
    return sortKvRows(
      (rows ?? []).filter((r) => !needle || r.key.toLowerCase().includes(needle)),
      sort,
    );
  }, [rows, filter, sort]);

  const [selected, setSelected] = useState<Set<string>>(new Set());
  // Bulk actions reach only the selected keys still on screen: a filter hides
  // a selected key, and an action on what the user cannot see is a surprise.
  const targets = shown.filter((r) => selected.has(r.key)).map((r) => r.key);
  const toggle = (key: string) =>
    setSelected((s) => {
      const next = new Set(s);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
    });
  const toggleAll = () =>
    setSelected((s) => {
      const all = shown.length > 0 && shown.every((r) => s.has(r.key));
      const next = new Set(s);
      for (const r of shown) {
        if (all) next.delete(r.key);
        else next.add(r.key);
      }
      return next;
    });

  const [drawer, setDrawer] = useState<Drawer>({ kind: "none" });
  const [confirmDelete, setConfirmDelete] = useState<{ keys: string[]; bulk: boolean } | null>(null);
  const [deleting, setDeleting] = useState(false);
  const [deleteError, setDeleteError] = useState<string | null>(null);

  const refresh = () => queryClient.invalidateQueries({ queryKey: workflowsKeys.namespace(ws, namespace) }).then(() =>
    queryClient.invalidateQueries({ queryKey: workflowsKeys.namespaces(ws) }),
  );

  const manage: Enablement = canManage ? { ok: true } : { ok: false, reason: MANAGE_REASON };
  const bulk: Enablement = !canManage
    ? { ok: false, reason: MANAGE_REASON }
    : targets.length === 0
      ? { ok: false, reason: "Select the keys to delete first" }
      : { ok: true };

  const copy = async (key: string) => {
    const expression = kvExpression(key);
    try {
      if (!navigator.clipboard) throw new Error("no clipboard");
      await navigator.clipboard.writeText(expression);
      toast.success(`Copied ${expression}.`);
    } catch {
      toast.error(`The expression could not be copied: the browser did not allow it. Copy it by hand: ${expression}`);
    }
  };

  const runDelete = async ({ keys, bulk }: { keys: string[]; bulk: boolean }) => {
    setDeleting(true);
    setDeleteError(null);
    try {
      if (!bulk) {
        const res = await workflowDeleteKvValue(ws, namespace, keys[0]);
        if (res.status === 200 && res.data === false) toast.info(`“${keys[0]}” was already gone.`);
        else toast.success(`Deleted the key “${keys[0]}”.`);
      } else {
        let gone = 0;
        for (let i = 0; i < keys.length; i += BULK_LIMIT) {
          const res = await workflowDeleteKvValues(ws, namespace, { keys: keys.slice(i, i + BULK_LIMIT) });
          if (res.status === 200) gone += res.data.keys.length;
        }
        const missing = keys.length - gone;
        const deleted = gone === 1 ? "1 key" : `${gone} keys`;
        toast.success(missing ? `Deleted ${deleted}; ${missing} ${missing === 1 ? "was" : "were"} already gone.` : `Deleted ${deleted}.`);
      }
      setSelected((s) => {
        const next = new Set(s);
        for (const k of keys) next.delete(k);
        return next;
      });
      setConfirmDelete(null);
      await refresh();
    } catch (e) {
      setDeleteError(kvErrorMessage(e, bulk ? "The keys were not deleted" : "The key was not deleted"));
    } finally {
      setDeleting(false);
    }
  };

  const existingKeys = useMemo(() => new Set((rows ?? []).map((r) => r.key)), [rows]);
  const addKey = () => setDrawer({ kind: "create" });
  const toolbarActions = [
    { id: "kv-add-reason", label: "Add a key", enablement: manage },
    { id: "kv-bulk-reason", label: "Delete selected", enablement: bulk },
  ];

  const body = (() => {
    // Error first (FE-STATE-1), while nothing has loaded.
    if (query.isError && !rows) return <ListError error={query.error} namespace={namespace} onRetry={() => void query.refetch()} />;
    if (!rows) return <ListSkeleton cols={5} />;
    if (rows.length === 0) {
      return (
        <EmptyState
          variant="dashed"
          icon={KeyRound}
          title="No KV keys yet"
          description="A key holds a value that this namespace's flows read with {{ kv('key') }}, and that its child namespaces inherit."
          action={
            <span className="flex flex-col items-center gap-1">
              <ReasonButton enablement={manage} reasonId="kv-empty-add-reason" onClick={addKey}>
                <Plus aria-hidden />
                Add a key
              </ReasonButton>
              {canManage ? null : (
                <span id="kv-empty-add-reason" className="text-xs text-muted-foreground">
                  {MANAGE_REASON}
                </span>
              )}
            </span>
          }
        />
      );
    }
    return (
      <>
        <p className="text-sm text-muted-foreground tabular-nums">{rows.length === 1 ? "1 key" : `${rows.length} keys`}</p>
        {shown.length === 0 ? (
          <div className="flex flex-wrap items-center gap-2">
            <p className="text-sm text-muted-foreground">No keys match “{filter}”.</p>
            <Button variant="outline" size="sm" onClick={() => setParam({ key: null })}>
              Clear filter
            </Button>
          </div>
        ) : (
          <KvTable
            rows={shown}
            sort={sort}
            onSort={onSort}
            canManage={canManage}
            selected={selected}
            onToggle={toggle}
            onToggleAll={toggleAll}
            onView={(entry) => setDrawer({ kind: "view", entry })}
            onEdit={(entry) => setDrawer({ kind: "edit", entry })}
            onCopy={(entry) => void copy(entry.key)}
            onDelete={(entry) => {
              setDeleteError(null);
              setConfirmDelete({ keys: [entry.key], bulk: false });
            }}
          />
        )}
      </>
    );
  })();

  return (
    <section aria-labelledby="kv-heading" className="flex min-w-0 flex-col gap-3">
      <h2 id="kv-heading" className="text-lg font-semibold">
        KV store
      </h2>
      <div className="flex flex-wrap items-end gap-2">
        <div className="grid gap-1">
          <Label htmlFor="kv-filter">Filter keys</Label>
          <Input
            id="kv-filter"
            type="search"
            className="w-56"
            placeholder="Key contains…"
            value={filter}
            onChange={(e) => setParam({ key: e.target.value })}
          />
        </div>
        <div className="ml-auto flex flex-wrap items-center gap-2">
          <Button type="button" size="sm" variant="outline" onClick={() => setDrawer({ kind: "inherited" })}>
            <Layers aria-hidden />
            Inherited keys
          </Button>
          <ReasonButton
            enablement={bulk}
            reasonId="kv-bulk-reason"
            variant="outline"
            onClick={() => {
              setDeleteError(null);
              setConfirmDelete({ keys: targets, bulk: true });
            }}
          >
            <Trash2 aria-hidden />
            {targets.length ? `Delete selected (${targets.length})` : "Delete selected"}
          </ReasonButton>
          <ReasonButton enablement={manage} reasonId="kv-add-reason" onClick={addKey}>
            <Plus aria-hidden />
            Add a key
          </ReasonButton>
        </div>
      </div>
      <DisabledReasons actions={toolbarActions} />
      {body}

      {drawer.kind === "view" ? (
        <KvValueDrawer
          ws={ws}
          namespace={namespace}
          entry={drawer.entry}
          canManage={canManage}
          onEdit={() => setDrawer({ kind: "edit", entry: drawer.entry })}
          onCopy={() => void copy(drawer.entry.key)}
          onClose={() => setDrawer({ kind: "none" })}
        />
      ) : null}
      {drawer.kind === "edit" || drawer.kind === "create" ? (
        <KvEditDrawer
          ws={ws}
          namespace={namespace}
          entry={drawer.kind === "edit" ? drawer.entry : undefined}
          existingKeys={existingKeys}
          onSaved={refresh}
          onClose={() => setDrawer({ kind: "none" })}
        />
      ) : null}
      {drawer.kind === "inherited" ? (
        <InheritedKvDrawer ws={ws} namespace={namespace} ownKeys={existingKeys} onClose={() => setDrawer({ kind: "none" })} />
      ) : null}

      <AlertDialog open={confirmDelete !== null} onOpenChange={(open) => (open || deleting ? undefined : setConfirmDelete(null))}>
        {confirmDelete ? <DeleteConfirm keys={confirmDelete.keys} bulk={confirmDelete.bulk} deleting={deleting} error={deleteError} onConfirm={() => void runDelete(confirmDelete)} /> : null}
      </AlertDialog>
    </section>
  );
}

/** The question before a delete: one key, or the selected ones (listed). */
function DeleteConfirm({
  keys,
  bulk,
  deleting,
  error,
  onConfirm,
}: {
  keys: string[];
  bulk: boolean;
  deleting: boolean;
  error: string | null;
  onConfirm: () => void;
}) {
  const count = keys.length === 1 ? "1 key" : `${keys.length} keys`;
  return (
    <AlertDialogContent>
      <AlertDialogHeader>
        <AlertDialogTitle>{bulk ? `Delete ${count}?` : `Delete “${keys[0]}”?`}</AlertDialogTitle>
        <AlertDialogDescription asChild>
          <div className="space-y-2">
            <p>
              {bulk
                ? "A flow that reads one of them with {{ kv('…') }} fails until it is written again."
                : `A flow that reads it with ${kvExpression(keys[0])} fails until it is written again.`}{" "}
              The earlier revisions are kept.
            </p>
            {bulk ? (
              <ul className="max-h-40 list-disc overflow-auto pl-5 font-mono text-xs">
                {keys.map((k) => (
                  <li key={k}>{k}</li>
                ))}
              </ul>
            ) : null}
          </div>
        </AlertDialogDescription>
      </AlertDialogHeader>
      {error ? (
        <Alert variant="destructive">
          <AlertDescription>{error}</AlertDescription>
        </Alert>
      ) : null}
      <AlertDialogFooter>
        <AlertDialogCancel disabled={deleting}>Cancel</AlertDialogCancel>
        <Button variant="destructive" disabled={deleting} onClick={onConfirm}>
          {bulk ? `Delete ${count}` : "Delete key"}
        </Button>
      </AlertDialogFooter>
    </AlertDialogContent>
  );
}

/**
 * The list could not be read: what happened and what to do (FE-COPY-2). A
 * namespace W9c refuses (an empty segment, `a..b`; a `_kv` segment) is a 400
 * that a retry cannot fix.
 */
function ListError({ error, namespace, onRetry }: { error: unknown; namespace: string; onRetry: () => void }) {
  const status = error instanceof ApiError ? error.status : undefined;
  const server = error instanceof Error && error.message ? error.message.replace(/\.$/, "") : "";
  if (status === 400) {
    return (
      <Alert variant="destructive">
        <AlertDescription>
          The namespace {namespace} cannot hold KV keys: {server}. Keys live in a namespace whose name has no empty segment;
          open another namespace to manage its keys.
        </AlertDescription>
      </Alert>
    );
  }
  const message = kvErrorMessage(error, "The KV keys could not be loaded");
  return (
    <Alert variant="destructive">
      <AlertDescription className="flex flex-wrap items-center gap-3">
        <span>{message}</span>
        <Button variant="outline" size="sm" onClick={onRetry}>
          <RotateCw aria-hidden />
          Retry
        </Button>
      </AlertDescription>
    </Alert>
  );
}
