import { useMemo, useState, type ReactNode } from "react";
import { useQuery } from "@tanstack/react-query";
import { useSearchParams } from "react-router-dom";
import { ChevronsUpDown, ShieldAlert } from "lucide-react";

import { getEffectiveAccess, type EffectiveAccess } from "@/api/access";
import { getWhoCanAccess, type WhoCanAccess as WhoCanAccessData } from "@/api/whoCanAccess";
import { listPrincipals } from "@/api/policies";
import { useWorkspaceShares } from "@/api/workspaceAccess";
import { adminWorkspacesListWs } from "@/api/generated/admin-workspaces/admin-workspaces";
import type { WorkspaceDto } from "@/api/generated/model";
import { EffectBadge, denyStyle } from "@/components/access/EffectBadge";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Tabs, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { Column, FilterableTable } from "@/components/explore/FilterableTable";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { errorStatus } from "@/lib/httpStatus";
import { PageContainer } from "@/components/layout/PageContainer";
import { PageTitle } from "@/components/layout/PageTitle";

/** Which way the explorer reads: principal → their access, or object → who can access it. */
type Direction = "principal" | "object";
type PrincipalType = "user" | "group";

/**
 * Control Plane → Access → Access Explorer.
 *
 * A single, bidirectional read-only view of the access graph:
 *  - **principal** direction: pick a user or group and see their *effective*
 *    access (platform roles, resolved permissions, workspaces, shared objects,
 *    and explicit denies), each annotated with provenance.
 *  - **object** direction: pick an object and see every principal who can
 *    access it — direct grants, workspace roles, and platform admins.
 *
 * The direction is derived from the URL so old deep-links keep working:
 * `?user=`/`?group=` → principal, `?kind=&id=` → object, otherwise `?dir=`
 * (default principal). Flipping the toggle clears the other direction's params.
 */
export default function AccessExplorer() {
  const [sp, setSp] = useSearchParams();
  const kind = sp.get("kind");
  const id = sp.get("id");
  const userId = sp.get("user");
  const groupId = sp.get("group");
  const dirParam = sp.get("dir");

  const direction: Direction =
    kind && id
      ? "object"
      : userId || groupId
        ? "principal"
        : dirParam === "object"
          ? "object"
          : "principal";

  // Switching direction resets the selection: keep only `?dir=` and drop the
  // other direction's params (user/group vs. ws/kind/id).
  const setDirection = (next: Direction) => {
    if (next === direction) return;
    setSp({ dir: next });
  };

  return (
    <PageContainer className="space-y-5">
      <div>
        <PageTitle>Access Explorer</PageTitle>
        <p className="text-sm text-muted-foreground">
          Explore access both ways — a user or group and everything they can reach, or an
          object and everyone who can reach it — with full provenance.
        </p>
      </div>

      <Tabs value={direction} onValueChange={(v) => setDirection(v as Direction)}>
        <TabsList>
          <TabsTrigger value="principal">User or group → can access</TabsTrigger>
          <TabsTrigger value="object">Object → who can access</TabsTrigger>
        </TabsList>
      </Tabs>

      {direction === "principal" ? <PrincipalDirection /> : <ObjectDirection />}
    </PageContainer>
  );
}

/* ------------------------------------------------------------------ *
 * Principal direction: user/group → their effective access
 * ------------------------------------------------------------------ */

/**
 * Turn a grant `source` ("direct" | "group:{name}") into a human suffix.
 * "direct" → "direct"; "group:analysts" → "via analysts".
 */
function sourceLabel(source: string): string {
  if (source.startsWith("group:")) return `via ${source.slice("group:".length)}`;
  return source;
}

/** Muted "· direct" / "· via {group}" suffix for a source. */
function SourceSuffix({ source }: { source: string }) {
  return <span className="text-xs text-muted-foreground">· {sourceLabel(source)}</span>;
}

/** Titled block wrapper for a report section. */
function Section({ title, children }: { title: string; children: ReactNode }) {
  return (
    <section className="space-y-2">
      <h2 className="text-base font-semibold">{title}</h2>
      {children}
    </section>
  );
}

/** Picker + effective-access report for a chosen principal. */
function PrincipalDirection() {
  const [sp, setSp] = useSearchParams();
  const userId = sp.get("user");
  const groupId = sp.get("group");
  const principalType: PrincipalType | null = userId ? "user" : groupId ? "group" : null;
  const principalId = userId ?? groupId ?? null;

  const q = useQuery({
    queryKey: ["cp", "effective", principalType, principalId],
    queryFn: () => getEffectiveAccess(principalType!, principalId!),
    enabled: !!principalId,
    retry: false,
  });

  const forbidden = errorStatus(q.error) === 403;
  const data = q.data;

  return (
    <>
      <Section title="Principal">
        <PrincipalPicker
          type={principalType ?? "user"}
          selected={principalId}
          onPick={(t, id) => setSp(t === "user" ? { user: id } : { group: id })}
        />
      </Section>

      {!principalId ? (
        <p className="py-10 text-center text-sm text-muted-foreground">
          Pick a user or group to see their effective access.
        </p>
      ) : q.isLoading ? (
        <ListSkeleton rows={4} cols={2} />
      ) : forbidden ? (
        <Alert variant="destructive">
          <AlertDescription>
            You need the <code>role:read</code> permission to view effective access.
          </AlertDescription>
        </Alert>
      ) : q.isError ? (
        <Alert variant="destructive">
          <AlertDescription>
            {(q.error as Error)?.message ?? "Failed to load effective access."}
          </AlertDescription>
        </Alert>
      ) : data ? (
        <PrincipalReport data={data} />
      ) : null}
    </>
  );
}

/** The full effective-access report for a resolved principal. */
function PrincipalReport({ data }: { data: EffectiveAccess }) {
  const { principal } = data;
  const isSuper = principal.is_platform_admin;

  return (
    <div className="space-y-6">
        {/* Header */}
        <div className="space-y-2">
          <div className="flex flex-wrap items-center gap-2">
            <span className="text-lg font-semibold">{principal.name ?? principal.id}</span>
            <Badge variant="outline">{principal.type === "group" ? "Group" : "User"}</Badge>
            {isSuper && (
              <Badge
                className="border-transparent"
                style={{
                  backgroundColor: "var(--highlight)",
                  color: "var(--highlight-foreground)",
                }}
              >
                <ShieldAlert className="mr-1 h-3 w-3" />
                Superuser
              </Badge>
            )}
          </div>
          {isSuper && (
            <p className="text-sm text-muted-foreground">
              Bypasses all permission checks — effectively has every permission.
            </p>
          )}
        </div>

        {/* Platform roles */}
        {data.platform_roles.length > 0 && (
          <Section title="Platform roles">
            <div className="flex flex-wrap gap-2">
              {data.platform_roles.map((r) => (
                <span
                  key={`${r.name}-${r.source}`}
                  className="inline-flex items-center gap-1.5 rounded-md border px-2 py-1"
                >
                  <span className="text-sm font-medium">{r.name}</span>
                  <SourceSuffix source={r.source} />
                </span>
              ))}
            </div>
          </Section>
        )}

        {/* Effective permissions (superuser skips — backend returns it empty) */}
        {!isSuper && data.permissions.length > 0 && (
          <Section title="Effective permissions">
            <PermissionsTable permissions={data.permissions} />
          </Section>
        )}

        {/* Workspaces */}
        {data.workspaces.length > 0 && (
          <Section title="Workspaces">
            <ul className="divide-y rounded-md border">
              {data.workspaces.map((w) => (
                <li key={w.workspace_id} className="flex items-center gap-2 px-3 py-2">
                  <span className="truncate text-sm">{w.workspace_name}</span>
                  <div className="ml-auto flex items-center gap-2">
                    <Badge variant="outline">{w.role}</Badge>
                    <SourceSuffix source={w.source} />
                  </div>
                </li>
              ))}
            </ul>
          </Section>
        )}

        {/* Shared objects */}
        {data.objects.length > 0 && (
          <Section title="Shared objects">
            <ul className="divide-y rounded-md border">
              {data.objects.map((o) => (
                <li key={`${o.kind}-${o.id}`} className="flex items-center gap-2 px-3 py-2">
                  <Badge variant="secondary" className="shrink-0 font-normal">
                    {o.kind}
                  </Badge>
                  <span className="truncate text-sm" title={o.id}>
                    {o.name ?? o.id}
                  </span>
                  {o.workspace_slug && (
                    <span className="text-xs text-muted-foreground">/{o.workspace_slug}</span>
                  )}
                  <div className="ml-auto flex items-center gap-2">
                    <Badge variant="outline">{o.role}</Badge>
                    <EffectBadge effect={o.effect} />
                    <SourceSuffix source={o.source} />
                  </div>
                </li>
              ))}
            </ul>
          </Section>
        )}

        {/* Denies / Blocked */}
        {data.denies.length > 0 && (
          <Section title="Blocked">
            <ul className="divide-y rounded-md border" style={denyStyle}>
              {data.denies.map((d, i) => (
                <li
                  key={`${d.scope}-${d.target}-${i}`}
                  className="flex items-center gap-2 px-3 py-2"
                >
                  <Badge variant="secondary" className="shrink-0 font-normal">
                    {d.scope}
                  </Badge>
                  <span className="truncate text-sm">{d.target}</span>
                  <div className="ml-auto flex items-center gap-2">
                    <Badge variant="outline">{d.role}</Badge>
                    <SourceSuffix source={d.source} />
                  </div>
                </li>
              ))}
            </ul>
          </Section>
        )}
    </div>
  );
}

/** Sortable/searchable table of resolved permissions and how each is granted. */
function PermissionsTable({ permissions }: { permissions: EffectiveAccess["permissions"] }) {
  const columns: Column<EffectiveAccess["permissions"][number]>[] = [
    {
      key: "key",
      label: "Permission",
      sortable: true,
      value: (r) => r.key,
      cell: (r) => <code className="font-mono text-xs">{r.key}</code>,
    },
    {
      key: "via",
      label: "Granted via",
      cell: (r) => (
        <span className="text-sm text-muted-foreground">
          {r.via
            .map((v) => (v.inherited_from ? `${v.role} (inherited from ${v.inherited_from})` : v.role))
            .join(", ")}
        </span>
      ),
    },
  ];

  return (
    <FilterableTable
      rows={permissions}
      columns={columns}
      getId={(r) => r.key}
      searchPlaceholder="Search permissions…"
      initialSort={{ key: "key", dir: "asc" }}
      pageSize={12}
      bordered
      emptyText="No permissions."
    />
  );
}

/**
 * Principal chooser: a User/Group toggle plus a combobox (Popover + search)
 * driven by the policy principal picker's server-side search — the list can be
 * thousands of users, so we query on demand rather than loading them all.
 * Picking an entry lifts the choice to the caller (which writes it to the URL).
 * The selected value is the principal id (a user's email or a group's name).
 */
function PrincipalPicker({
  type: initialType,
  selected,
  onPick,
}: {
  type: PrincipalType;
  selected: string | null;
  onPick: (type: PrincipalType, id: string) => void;
}) {
  const [type, setType] = useState<PrincipalType>(initialType);
  const [open, setOpen] = useState(false);
  const [q, setQ] = useState("");

  const suggestQ = useQuery({
    queryKey: ["explorer-principals", type, q],
    queryFn: () => listPrincipals(q, type),
    enabled: open,
  });
  const suggestions = useMemo(() => suggestQ.data ?? [], [suggestQ.data]);

  return (
    <div className="flex items-center gap-2">
      <Select
        value={type}
        onValueChange={(v) => {
          setType(v as PrincipalType);
          setQ("");
        }}
      >
        <SelectTrigger className="h-9 w-28" aria-label="Principal type">
          <SelectValue />
        </SelectTrigger>
        <SelectContent>
          <SelectItem value="user">User</SelectItem>
          <SelectItem value="group">Group</SelectItem>
        </SelectContent>
      </Select>

      <Popover open={open} onOpenChange={setOpen}>
        <PopoverTrigger asChild>
          <Button
            variant="outline"
            role="combobox"
            aria-label={type === "group" ? "Group name" : "User email"}
            className="h-9 w-72 justify-between font-normal"
          >
            <span className="truncate">
              {selected ?? (type === "group" ? "Select a group…" : "Select a user…")}
            </span>
            <ChevronsUpDown className="ml-2 h-4 w-4 shrink-0 opacity-50" />
          </Button>
        </PopoverTrigger>
        <PopoverContent className="w-72 p-0" align="start">
          <div className="border-b p-2">
            <Input
              autoFocus
              placeholder={type === "group" ? "Search groups…" : "Search users…"}
              value={q}
              onChange={(e) => setQ(e.target.value)}
            />
          </div>
          <div className="max-h-64 overflow-y-auto py-1">
            {suggestQ.isLoading ? (
              <p className="px-3 py-2 text-sm text-muted-foreground">Searching…</p>
            ) : suggestions.length === 0 ? (
              <p className="px-3 py-2 text-sm text-muted-foreground">
                No {type === "group" ? "groups" : "users"} found.
              </p>
            ) : (
              suggestions.map((p) => (
                <button
                  key={p.name}
                  type="button"
                  className="block w-full px-3 py-1.5 text-left text-sm hover:bg-accent"
                  onClick={() => {
                    setOpen(false);
                    setQ("");
                    onPick(type, p.name);
                  }}
                >
                  <span className="font-medium">{p.label}</span>
                  {p.label !== p.name && (
                    <span className="text-muted-foreground"> · {p.name}</span>
                  )}
                </button>
              ))
            )}
          </div>
        </PopoverContent>
      </Popover>
    </div>
  );
}

/* ------------------------------------------------------------------ *
 * Object direction: object → who can access it
 * ------------------------------------------------------------------ */

/**
 * Turn an access-entry `source` ("direct" | "workspace:{slug}" | "platform")
 * into a human suffix.
 */
function objectSourceLabel(source: string): string {
  if (source === "direct") return "direct grant";
  if (source === "platform") return "platform admin";
  if (source.startsWith("workspace:")) return `via workspace ${source.slice("workspace:".length)}`;
  return source;
}

/** Muted source suffix, e.g. "direct grant" / "via workspace demo" / "platform admin". */
function ObjectSourceSuffix({ source }: { source: string }) {
  return <span className="text-xs text-muted-foreground">{objectSourceLabel(source)}</span>;
}

/** Picker + who-can-access report for a chosen object. */
function ObjectDirection() {
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
    <>
      <Section title="Object">
        <ObjectPicker
          ws={ws}
          kind={kind}
          id={id}
          onPickWorkspace={(newWs) => setSp({ dir: "object", ws: newWs })}
          onPickObject={(newKind, newId) =>
            setSp({ dir: "object", ws: ws ?? "", kind: newKind, id: newId })
          }
        />
      </Section>

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
        <ObjectReport data={data} />
      ) : null}
    </>
  );
}

/** Coarse category for a who-can-access entry's `source`, used by the filter. */
type SourceCategory = "direct" | "workspace" | "platform";
function sourceCategory(source: string): SourceCategory {
  if (source === "direct") return "direct";
  if (source === "platform") return "platform";
  return "workspace";
}

/** The full who-can-access report for a resolved object. */
function ObjectReport({ data }: { data: WhoCanAccessData }) {
  const { object, entries } = data;
  const [sourceFilter, setSourceFilter] = useState<"all" | SourceCategory>("all");

  // Platform admins typically dominate this list (every admin can reach every
  // object), which buries the direct/workspace grants that actually explain
  // *this* object's access. The filter lets you cut that noise.
  const counts = useMemo(() => {
    const c = { direct: 0, workspace: 0, platform: 0 };
    for (const e of entries) c[sourceCategory(e.source)] += 1;
    return c;
  }, [entries]);

  const filteredEntries = useMemo(
    () =>
      sourceFilter === "all"
        ? entries
        : entries.filter((e) => sourceCategory(e.source) === sourceFilter),
    [entries, sourceFilter],
  );

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
      value: (e) => objectSourceLabel(e.source),
      cell: (e) => <ObjectSourceSuffix source={e.source} />,
    },
  ];

  return (
    <div className="space-y-6">
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
          rows={filteredEntries}
          columns={columns}
          getId={(e) => `${e.principal_type}:${e.principal_id}:${e.source}:${e.access}`}
          searchPlaceholder="Search principals…"
          initialSort={{ key: "principal", dir: "asc" }}
          pageSize={12}
          bordered
          filters={
            <Select
              value={sourceFilter}
              onValueChange={(v) => setSourceFilter(v as "all" | SourceCategory)}
            >
              <SelectTrigger className="h-9 w-48" aria-label="Filter by source">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="all">All sources ({entries.length})</SelectItem>
                <SelectItem value="direct">Direct grant ({counts.direct})</SelectItem>
                <SelectItem value="workspace">Workspace role ({counts.workspace})</SelectItem>
                <SelectItem value="platform">Platform admin ({counts.platform})</SelectItem>
              </SelectContent>
            </Select>
          }
          emptyText="No one has access."
          noMatchText="No principals match."
        />
    </div>
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
