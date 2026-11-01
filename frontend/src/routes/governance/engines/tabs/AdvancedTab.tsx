import { useState } from "react";
import { Link } from "react-router-dom";
import { useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";
import { ChevronDown, FileText, Loader2, Plus, Sprout, TriangleAlert } from "lucide-react";
import {
  getPolicyAdminListPoliciesQueryKey,
  usePolicyAdminSeedDefaults,
} from "@/api/generated/policy-admin/policy-admin";
import type { PolicySummary } from "@/api/generated/model";
import { EmptyState } from "@/components/explore/EmptyState";
import { ListSkeleton } from "@/components/explore/ListSkeleton";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { refusalMessage, usePolicyPermissions } from "@/routes/governance/policyWrites";
import { Section, STACKED_TABLE, TabError } from "@/routes/governance/tabParts";
import { editorUrl } from "@/routes/governance/engines/engineUrl";
import type { TabContext } from "@/routes/governance/engines/PageFrame";
import { ENTITY_LINK_CLASS } from "@/components/ui/entity-link";
import { cn } from "@/lib/utils";
import { TagServiceSection } from "@/routes/governance/engines/tabs/TagServiceSection";

export const ADVANCED_WARNING =
  "Manual policies are outside egret's model: egret does not recompile them, does not check overlaps with grants, and Data and People pages will mark them manual. Prefer Engine permissions or grants on Data.";
export const GENERATED_REFUSES =
  "egret compiles this profile, so new manual policies are refused. Existing ones can still be edited, enabled, disabled and deleted.";
export const SEED_EXPLAINED =
  "Adds egret's default Trino set (group admin: all access). Existing policies are not changed.";

const TYPE_LABEL: Record<string, string> = { access: "Access", masking: "Masking", rowfilter: "Row filter" };

const plural = (n: number, one: string, many: string) => `${n} ${n === 1 ? one : many}`;

/** What a seed answered, said on the tab (KI-135): the rows land on Policies, not in this tab's list. */
export function seedResultSentence(created: number): string {
  if (created === 0) return "No default policy was added: the profile already has policies.";
  return `Added ${plural(created, "default policy", "default policies")}. They are egret-seeded, so they are listed on the Policies tab, not here.`;
}

/**
 * Under "no manual policy": the profile's egret-compiled and egret-seeded
 * policies, which this tab does not list, and where they are (KI-135).
 */
export function otherPoliciesSentence(rows: PolicySummary[]): string | undefined {
  const compiled = rows.filter((r) => r.managed_by === "compiler").length;
  const seeded = rows.filter((r) => r.managed_by === "infra").length;
  const parts = [
    compiled ? plural(compiled, "egret-compiled policy", "egret-compiled policies") : null,
    seeded ? plural(seeded, "egret-seeded policy", "egret-seeded policies") : null,
  ].filter((p): p is string => !!p);
  if (parts.length === 0) return undefined;
  const verb = compiled + seeded === 1 ? "is" : "are";
  return `Its ${parts.join(" and ")} ${verb} listed on the Policies tab.`;
}

/**
 * Advanced (spec §6.6, EP2, EP9): the raw Ranger policies a person wrote by
 * hand, behind a fixed warning, with Create policy and Seed default policies.
 * Both writes are refused by the server once the profile is compiled
 * (`origin = 'generated'`, spec premise 13), so they are offered on a manual
 * profile only; editing, toggling and deleting existing rows stay in the
 * Policies tab and the editor. Each write control checks its own permission
 * (`policy:create`, `policy:seed`), as the old profile page did.
 */
export function AdvancedTab({ ctx }: { ctx: TabContext }) {
  const can = usePolicyPermissions();
  const queryClient = useQueryClient();
  const [seedError, setSeedError] = useState<string | null>(null);
  const [seedResult, setSeedResult] = useState<string | null>(null);
  const profile = ctx.profile;
  const policies = ctx.policies;

  const seed = usePolicyAdminSeedDefaults({
    mutation: {
      onSuccess: (res) => {
        setSeedError(null);
        void queryClient.invalidateQueries({ queryKey: getPolicyAdminListPoliciesQueryKey() });
        void queryClient.invalidateQueries({ queryKey: ["control-plane", "policies"] });
        const n = res.status === 200 ? res.data.created : 0;
        // The toast alone was missed (KI-135): the result also stays on the tab.
        setSeedResult(seedResultSentence(n));
        toast.success(
          n === 0
            ? "No default policy was added: the profile already has policies."
            : n === 1
              ? "Added 1 default policy."
              : `Added ${n} default policies.`,
        );
      },
      onError: (e) => {
        setSeedResult(null);
        setSeedError(refusalMessage(e, "policy:seed", "The default policies could not be added."));
      },
    },
  });

  const warning = (
    <Alert>
      <TriangleAlert className="h-4 w-4" aria-hidden />
      <AlertDescription>{ADVANCED_WARNING}</AlertDescription>
    </Alert>
  );

  if (!profile) {
    return (
      <div className="space-y-4">
        {warning}
        <EmptyState icon={FileText} title="This engine downloads no policy profile, so it has no policy." />
      </div>
    );
  }

  // The origin is known once the profile list answered; until then no write is offered.
  const origin = profile.dto?.origin ?? null;
  const manual = origin === "manual";
  const canCreate = manual && can("policy:create");
  const canSeed = manual && profile.type === "trino" && can("policy:seed");

  const handWritten = policies.rows
    .filter((r) => r.managed_by === "operator")
    .sort((a, b) => a.name.localeCompare(b.name));

  let list;
  if (policies.isError) {
    list = <TabError what="policies of this profile" onRetry={policies.refetch} />;
  } else if (policies.isLoading) {
    list = <ListSkeleton rows={3} cols={4} />;
  } else if (handWritten.length === 0) {
    list = (
      <EmptyState
        icon={FileText}
        title="This profile has no manual policy."
        description={otherPoliciesSentence(policies.rows)}
      />
    );
  } else {
    list = <HandWrittenTable rows={handWritten} ctx={ctx} />;
  }

  return (
    <div className="space-y-6">
      {warning}
      {origin === "generated" ? <p className="text-sm text-muted-foreground">{GENERATED_REFUSES}</p> : null}
      {canCreate || canSeed ? (
        <div className="flex flex-wrap items-start gap-4">
          {canCreate ? <CreatePolicyMenu ctx={ctx} /> : null}
          {canSeed ? (
            <div className="space-y-1">
              <Button
                size="sm"
                variant="secondary"
                disabled={seed.isPending}
                onClick={() => seed.mutate({ service: profile.name })}
              >
                {seed.isPending ? (
                  <Loader2 className="mr-1 h-4 w-4 animate-spin" aria-hidden />
                ) : (
                  <Sprout className="mr-1 h-4 w-4" aria-hidden />
                )}
                Seed default policies
              </Button>
              <p className="max-w-prose text-sm text-muted-foreground">{SEED_EXPLAINED}</p>
            </div>
          ) : null}
        </div>
      ) : null}
      {canSeed ? (
        <p role="status" className="text-sm">
          {seedResult}
        </p>
      ) : null}
      {seedError ? (
        <Alert variant="destructive">
          <AlertDescription>{seedError}</AlertDescription>
        </Alert>
      ) : null}
      <Section title="Manual policies">{list}</Section>
      {profile.type === "trino" ? <TagServiceSection profile={profile} /> : null}
    </div>
  );
}

/** Create policy: one link per kind, and By tag when the profile has a tag service (FE-PAGE-3: links). */
function CreatePolicyMenu({ ctx }: { ctx: TabContext }) {
  const kinds: Array<[string, string]> = [
    ["access", "Access"],
    ["masking", "Masking"],
    ["rowfilter", "Row filter"],
  ];
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button size="sm">
          <Plus className="mr-1 h-4 w-4" aria-hidden />
          Create policy
          <ChevronDown className="ml-1 h-4 w-4" aria-hidden />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start">
        {kinds.map(([kind, label]) => (
          <DropdownMenuItem key={kind} asChild>
            <Link to={editorUrl(ctx.subject, kind)}>{label}</Link>
          </DropdownMenuItem>
        ))}
        {ctx.profile?.tagService ? (
          <DropdownMenuItem asChild>
            <Link to={`${editorUrl(ctx.subject, "access")}?scope=tag`}>By tag</Link>
          </DropdownMenuItem>
        ) : null}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

function HandWrittenTable({ rows, ctx }: { rows: PolicySummary[]; ctx: TabContext }) {
  const tagService = ctx.profile?.tagService ?? null;
  return (
    <Table aria-label="Manual policies" className={STACKED_TABLE}>
      <TableHeader>
        <TableRow>
          <TableHead>Name</TableHead>
          <TableHead>Type</TableHead>
          <TableHead>Resource</TableHead>
          <TableHead>Enabled</TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        {rows.map((r) => (
          <TableRow key={`${r.service}/${r.id}`}>
            <TableCell>
              {/* Every edit goes through the page's profile with the row's own
                  type, so a tag-service policy opens in the By-tag editor. */}
              <Link
                to={editorUrl(ctx.subject, r.policy_type, r.id)}
                className={cn(ENTITY_LINK_CLASS, "break-all focus-visible:ring-2 focus-visible:ring-ring")}
              >
                {r.name}
              </Link>
            </TableCell>
            <TableCell data-label="Type">
              {TYPE_LABEL[r.policy_type] ?? r.policy_type}
              {tagService && r.service === tagService ? " (by tag)" : ""}
            </TableCell>
            <TableCell data-label="Resource" className="break-all font-mono text-xs">
              {r.resource_path || "—"}
            </TableCell>
            <TableCell data-label="Enabled">{r.is_enabled ? "Enabled" : "Disabled"}</TableCell>
          </TableRow>
        ))}
      </TableBody>
    </Table>
  );
}
