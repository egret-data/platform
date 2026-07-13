import { useMemo } from "react";
import { useQuery } from "@tanstack/react-query";
import { useSearchParams } from "react-router-dom";

import { getWhoCanAccess, type WhoCanAccess as WhoCanAccessData } from "@/api/whoCanAccess";
import { useWorkspaceShares } from "@/api/workspaceAccess";
import { adminWorkspacesListWs } from "@/api/generated/admin-workspaces/admin-workspaces";
import type { WorkspaceDto } from "@/api/generated/model";
import { EffectBadge } from "@/components/access/EffectBadge";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Column, FilterableTable } from "@/components/explore/FilterableTable";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { errorStatus } from "@/lib/httpStatus";

/**
 * Turn an access-entry `source` ("direct" | "workspace:{slug}" | "platform")
 * into a human suffix.
 */
function sourceLabel(source: string): string {
  if (source === "direct") return "direct grant";
  if (source === "platform") return "platform admin";
  if (source.startsWith("workspace:")) return `via workspace ${source.slice("workspace:".length)}`;
  return source;
}

/** Muted source suffix, e.g. "direct grant" / "via workspace demo" / "platform admin". */
function SourceSuffix({ source }: { source: string }) {
  return <span className="text-xs text-muted-foreground">{sourceLabel(source)}</span>;
}

/**
 * Control Plane → Access control → Who can access.
 *
 * Read-only reverse view of Access explorer: given an *object* (chart,
 * dashboard, dataset, notebook, saved query, folder…), show every principal
 * who can access it — direct grants, workspace roles, and platform admins —
 * each annotated with where it comes from. Deep-linked via
 * `?ws=<slug>&kind=<kind>&id=<uuid>`.
 */
export default function WhoCanAccess() {
  const [sp, setSp] = useSearchParams();
  const ws = sp.get("ws");
  const kind = sp.get("kind");
  const id = sp.get("id");

  const q = useQuery({
    queryKey: ["cp", "who-can-access", kind, id],
    queryFn: () => getWhoCanAccess(kind!, id!),
    enabled: !!kind && !!id,
    retry: false,
  });

  const forbidden = errorStatus(q.error) === 403;
  const notFound = errorStatus(q.error) === 404;
  const data = q.data;

  return (
    <div className="space-y-5">
      <div>
        <h1 className="text-2xl font-semibold leading-tight">Who can access</h1>
        <p className="text-sm text-muted-foreground">
          See every principal who can access an object — direct grants, workspace roles, and
          platform admins — with provenance.
        </p>
      </div>

      <Card>
        <CardHeader>
          <CardTitle className="text-lg">Object</CardTitle>
        </CardHeader>
        <CardContent>
          <ObjectPicker
            ws={ws}
            kind={kind}
            id={id}
            onPickWorkspace={(newWs) => setSp({ ws: newWs })}
            onPickObject={(newKind, newId) => setSp({ ws: ws ?? "", kind: newKind, id: newId })}
          />
        </CardContent>
      </Card>

      {!kind || !id ? (
        <p className="py-10 text-center text-sm text-muted-foreground">
          Pick an object to see who can access it.
        </p>
      ) : q.isLoading ? (
        <ListSkeleton rows={4} cols={4} />
      ) : forbidden ? (
        <Alert variant="destructive">
          <AlertDescription>
            You need the <code>role:read</code> permission.
          </AlertDescription>
        </Alert>
      ) : notFound ? (
        <p className="py-10 text-center text-sm text-muted-foreground">Object not found.</p>
      ) : q.isError ? (
        <Alert variant="destructive">
          <AlertDescription>
            {(q.error as Error)?.message ?? "Failed to load access."}
          </AlertDescription>
        </Alert>
      ) : data ? (
        <Report data={data} />
      ) : null}
    </div>
  );
}

/** The full who-can-access report for a resolved object. */
function Report({ data }: { data: WhoCanAccessData }) {
  const { object, entries } = data;

  const columns: Column<WhoCanAccessData["entries"][number]>[] = [
    {
      key: "principal",
      label: "Principal",
      sortable: true,
      value: (e) => e.principal_name ?? e.principal_id,
      cell: (e) => (
        <span className="flex min-w-0 items-center gap-2">
          <Badge variant="outline" className="shrink-0">
            {e.principal_type === "group" ? "Group" : "User"}
          </Badge>
          <span className="truncate text-sm" title={e.principal_id}>
            {e.principal_name ?? e.principal_id}
          </span>
        </span>
      ),
    },
    {
      key: "access",
      label: "Access",
      sortable: true,
      value: (e) => e.access,
      cell: (e) => <Badge variant="outline">{e.access}</Badge>,
    },
    {
      key: "effect",
      label: "Effect",
      sortable: true,
      value: (e) => e.effect,
      cell: (e) => <EffectBadge effect={e.effect} />,
    },
    {
      key: "source",
      label: "Source",
      sortable: true,
      value: (e) => sourceLabel(e.source),
      cell: (e) => <SourceSuffix source={e.source} />,
    },
  ];

  return (
    <Card>
      <CardContent className="space-y-6 pt-6">
        <div className="flex flex-wrap items-center gap-2">
          <Badge variant="secondary" className="shrink-0 font-normal">
            {object.kind}
          </Badge>
          <span className="text-lg font-semibold" title={object.id}>
            {object.name ?? object.id}
          </span>
          {object.workspace_slug && (
            <span className="text-sm text-muted-foreground">/{object.workspace_slug}</span>
          )}
        </div>

        <FilterableTable
          rows={entries}
          columns={columns}
          getId={(e) => `${e.principal_type}:${e.principal_id}:${e.source}:${e.access}`}
          searchPlaceholder="Search principals…"
          initialSort={{ key: "principal", dir: "asc" }}
          emptyText="No one has access."
        />
      </CardContent>
    </Card>
  );
}

/**
 * Object chooser: a workspace `<Select>` followed by an object `<Select>`
 * populated from that workspace's shared-object rollup (`useWorkspaceShares`).
 * Picking an arbitrary object outside the shared-object rollup isn't
 * supported yet — a known follow-up.
 */
function ObjectPicker({
  ws,
  kind,
  id,
  onPickWorkspace,
  onPickObject,
}: {
  ws: string | null;
  kind: string | null;
  id: string | null;
  onPickWorkspace: (ws: string) => void;
  onPickObject: (kind: string, id: string) => void;
}) {
  const wsListQ = useQuery({
    queryKey: ["cp", "admin-workspaces", "list"],
    queryFn: async () => (await adminWorkspacesListWs({ per_page: 200 })).data,
    retry: false,
  });
  const workspaces = (wsListQ.data ?? []) as WorkspaceDto[];

  const sharesQ = useWorkspaceShares(ws ?? "");
  const shares = ws ? (sharesQ.data ?? []) : [];

  const objects = useMemo(() => {
    const seen = new Set<string>();
    const out: { kind: string; object_id: string; object_name: string | null }[] = [];
    for (const s of shares) {
      if (seen.has(s.object_id)) continue;
      seen.add(s.object_id);
      out.push({ kind: s.kind, object_id: s.object_id, object_name: s.object_name });
    }
    return out;
  }, [shares]);

  const objectValue = kind && id ? `${kind}:${id}` : undefined;

  return (
    <div className="flex flex-wrap items-start gap-2">
      <Select value={ws ?? undefined} onValueChange={onPickWorkspace}>
        <SelectTrigger className="h-9 w-56" aria-label="Workspace">
          <SelectValue placeholder="Select a workspace…" />
        </SelectTrigger>
        <SelectContent>
          {workspaces.map((w) => (
            <SelectItem key={w.id} value={w.slug}>
              {w.name}
            </SelectItem>
          ))}
        </SelectContent>
      </Select>

      <div className="flex flex-col gap-1">
        <Select
          value={objectValue}
          disabled={!ws}
          onValueChange={(v) => {
            const sep = v.indexOf(":");
            onPickObject(v.slice(0, sep), v.slice(sep + 1));
          }}
        >
          <SelectTrigger className="h-9 w-72" aria-label="Object">
            <SelectValue placeholder="Select an object…" />
          </SelectTrigger>
          <SelectContent>
            {objects.map((o) => (
              <SelectItem key={o.object_id} value={`${o.kind}:${o.object_id}`}>
                {o.kind} · {o.object_name ?? o.object_id}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
        {ws && !sharesQ.isLoading && objects.length === 0 && (
          <p className="text-xs text-muted-foreground">No shared objects in this workspace.</p>
        )}
      </div>
    </div>
  );
}
