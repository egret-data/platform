import { useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { ShieldCheck } from "lucide-react";
import { type Profile, type PolicySummary, listProfiles, listPolicies } from "@/api/policies";
import { ApiError } from "@/api/client";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import {
  Table, TableBody, TableCell, TableHead, TableHeader, TableRow,
} from "@/components/ui/table";

const TYPES = [
  { key: "access", label: "Access" },
  { key: "masking", label: "Masking" },
  { key: "rowfilter", label: "Row-filter" },
] as const;

export default function Policies() {
  const profiles = useQuery<Profile[], ApiError>({
    queryKey: ["control-plane", "profiles"],
    queryFn: listProfiles,
    retry: false,
  });
  const [service, setService] = useState<string | null>(null);
  const [policyType, setPolicyType] = useState<(typeof TYPES)[number]["key"]>("access");

  const activeService = service ?? profiles.data?.[0]?.name ?? null;

  const policies = useQuery<PolicySummary[], ApiError>({
    queryKey: ["control-plane", "policies", activeService, policyType],
    queryFn: () => listPolicies(activeService ?? "", policyType),
    enabled: !!activeService,
    retry: false,
  });

  const rows = policies.data ?? [];

  return (
    <div className="space-y-4 p-6">
      <div className="flex items-center gap-2">
        <ShieldCheck className="h-5 w-5" />
        <h1 className="text-xl font-semibold">Policies</h1>
      </div>

      <Card>
        <CardHeader>
          <CardTitle>Profiles</CardTitle>
          <CardDescription>Ranger policy sets. Each engine enforces one profile.</CardDescription>
        </CardHeader>
        <CardContent className="flex flex-wrap gap-2">
          {profiles.isError ? (
            <Alert variant="destructive">
              <AlertDescription>
                {profiles.error?.message ?? "Failed to load profiles."}
              </AlertDescription>
            </Alert>
          ) : profiles.isLoading ? (
            <span className="text-sm text-muted-foreground">Loading…</span>
          ) : (
            (profiles.data ?? []).map((p) => (
              <button
                key={p.id}
                onClick={() => setService(p.name)}
                className={`rounded-md border px-3 py-1.5 text-sm ${
                  p.name === activeService ? "bg-accent text-accent-foreground" : "text-muted-foreground"
                }`}
              >
                {p.name}
                {p.engines.length > 0 && (
                  <span className="ml-2 text-xs opacity-70">{p.engines.map((e) => e.name).join(", ")}</span>
                )}
              </button>
            ))
          )}
        </CardContent>
      </Card>

      <div className="flex gap-1 border-b" role="tablist">
        {TYPES.map((t) => (
          <button
            key={t.key}
            type="button"
            role="tab"
            aria-selected={policyType === t.key}
            onClick={() => setPolicyType(t.key)}
            className={`px-3 py-2 text-sm ${policyType === t.key ? "border-b-2 border-primary font-medium" : "text-muted-foreground"}`}
          >
            {t.label}
          </button>
        ))}
      </div>

      <Table>
        <TableHeader>
          <TableRow>
            <TableHead>Name</TableHead>
            <TableHead>Resource</TableHead>
            <TableHead>Principals</TableHead>
            <TableHead>Permissions</TableHead>
            <TableHead>Enabled</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {rows.map((r) => (
            <TableRow key={r.id}>
              <TableCell className="font-medium">{r.name}</TableCell>
              <TableCell className="font-mono text-xs">{r.resource_path}</TableCell>
              <TableCell>{r.principals.join(", ")}</TableCell>
              <TableCell>{r.permissions.join(", ")}</TableCell>
              <TableCell>
                <Badge variant={r.is_enabled ? "secondary" : "outline"}>
                  {r.is_enabled ? "enabled" : "disabled"}
                </Badge>
              </TableCell>
            </TableRow>
          ))}
          {rows.length === 0 && (
            <TableRow className="hover:bg-transparent">
              <TableCell colSpan={5} className="text-center text-muted-foreground">
                {policies.isError ? (
                  <span className="text-destructive">
                    {policies.error?.message ?? "Failed to load policies."}
                  </span>
                ) : !activeService || policies.isLoading ? (
                  "Loading…"
                ) : (
                  `No ${policyType} policies for this profile.`
                )}
              </TableCell>
            </TableRow>
          )}
        </TableBody>
      </Table>
    </div>
  );
}
