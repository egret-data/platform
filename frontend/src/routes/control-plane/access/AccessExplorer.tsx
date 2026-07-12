import { useMemo, useState, type CSSProperties, type ReactNode } from "react";
import { useQuery } from "@tanstack/react-query";
import { useSearchParams } from "react-router-dom";
import { Search, ShieldAlert } from "lucide-react";

import { getEffectiveAccess, type EffectiveAccess } from "@/api/access";
import { listPrincipals } from "@/api/policies";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Column, FilterableTable } from "@/components/explore/FilterableTable";

type PrincipalType = "user" | "group";

/** Narrow a react-query error into an HTTP status when one is present. */
function errorStatus(err: unknown): number | undefined {
  if (err && typeof err === "object" && "status" in err) {
    const s = (err as { status?: unknown }).status;
    if (typeof s === "number") return s;
  }
  return undefined;
}

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
      <h2 className="text-sm font-semibold">{title}</h2>
      {children}
    </section>
  );
}

const allowStyle: CSSProperties = {
  backgroundColor: "color-mix(in oklab, var(--success) 16%, transparent)",
  color: "var(--success)",
  borderColor: "color-mix(in oklab, var(--success) 32%, transparent)",
};
const denyStyle: CSSProperties = {
  backgroundColor: "color-mix(in oklab, var(--destructive) 16%, transparent)",
  color: "var(--destructive)",
  borderColor: "color-mix(in oklab, var(--destructive) 32%, transparent)",
};

/** allow/deny badge tinted with the semantic success/destructive token. */
function EffectBadge({ effect }: { effect: string }) {
  const deny = effect === "deny";
  return (
    <Badge className="border font-normal" style={deny ? denyStyle : allowStyle}>
      {effect}
    </Badge>
  );
}

/**
 * Control Plane → Access control → Access explorer.
 *
 * Read-only view of a user's or group's *effective* access — the platform
 * roles, resolved permissions, workspace memberships, shared objects, and
 * explicit denies that apply to them, each annotated with where it comes from
 * (direct vs. inherited via a group / parent role). The principal is chosen at
 * the top and deep-linked via `?user=<id>` / `?group=<id>`.
 */
export default function AccessExplorer() {
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
    <div className="space-y-5">
      <div>
        <h1 className="text-2xl font-semibold leading-tight">Access explorer</h1>
        <p className="text-sm text-muted-foreground">
          Inspect the effective access of a user or group — roles, permissions,
          workspaces, and shared objects — with full provenance.
        </p>
      </div>

      <Card>
        <CardHeader>
          <CardTitle className="text-lg">Principal</CardTitle>
        </CardHeader>
        <CardContent>
          <PrincipalPicker
            type={principalType ?? "user"}
            selected={principalId}
            onPick={(t, id) => setSp(t === "user" ? { user: id } : { group: id })}
          />
        </CardContent>
      </Card>

      {!principalId ? (
        <p className="py-10 text-center text-sm text-muted-foreground">
          Pick a user or group to see their effective access.
        </p>
      ) : q.isLoading ? (
        <p className="py-10 text-center text-sm text-muted-foreground">Loading…</p>
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
        <Report data={data} />
      ) : null}
    </div>
  );
}

/** The full effective-access report for a resolved principal. */
function Report({ data }: { data: EffectiveAccess }) {
  const { principal } = data;
  const isSuper = principal.is_platform_admin;

  return (
    <Card>
      <CardContent className="space-y-6 pt-6">
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
      </CardContent>
    </Card>
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
      emptyText="No permissions."
    />
  );
}

/**
 * Principal chooser: a User/Group toggle plus a search box with live
 * suggestions from the policy principal picker. Selecting a suggestion lifts
 * the choice to the caller (which writes it to the URL). The selected value is
 * the principal id (a user's email or a group's name).
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
  const [value, setValue] = useState(selected ?? "");
  const [showSuggest, setShowSuggest] = useState(false);

  const suggestQ = useQuery({
    queryKey: ["explorer-principals", type, value],
    queryFn: () => listPrincipals(value, type),
    enabled: showSuggest && value.trim().length > 0,
  });
  const suggestions = useMemo(() => suggestQ.data ?? [], [suggestQ.data]);

  return (
    <div className="flex items-start gap-2">
      <Select
        value={type}
        onValueChange={(v) => {
          setType(v as PrincipalType);
          setValue("");
          setShowSuggest(false);
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

      <div className="relative max-w-sm flex-1">
        <Search className="pointer-events-none absolute left-2.5 top-1/2 h-4 w-4 -translate-y-1/2 text-muted-foreground" />
        <Input
          className="h-9 pl-8"
          aria-label={type === "group" ? "Group name" : "User email"}
          placeholder={type === "group" ? "Search groups…" : "Search users…"}
          value={value}
          onChange={(e) => {
            setValue(e.target.value);
            setShowSuggest(true);
          }}
          onFocus={() => setShowSuggest(true)}
        />
        {showSuggest && value.trim().length > 0 && suggestions.length > 0 && (
          <ul className="absolute z-10 mt-1 max-h-48 w-full overflow-auto rounded-md border bg-popover p-1 shadow-md">
            {suggestions.map((p) => (
              <li key={p.name}>
                <button
                  type="button"
                  className="w-full rounded px-2 py-1.5 text-left text-sm hover:bg-accent"
                  onClick={() => {
                    setValue(p.name);
                    setShowSuggest(false);
                    onPick(type, p.name);
                  }}
                >
                  <span className="font-medium">{p.label}</span>
                  {p.label !== p.name && (
                    <span className="ml-1 text-xs text-muted-foreground">{p.name}</span>
                  )}
                </button>
              </li>
            ))}
          </ul>
        )}
      </div>
    </div>
  );
}
