import { useParams } from "react-router-dom";
import { useQuery } from "@tanstack/react-query";
import { Bot } from "lucide-react";
import { useAdminWorkspacesListWs } from "@/api/generated/admin-workspaces/admin-workspaces";
import { getServiceAccount, serviceAccountQueryKey, type ServiceAccountDetailDto } from "@/api/serviceAccounts";
import { DetailFact } from "@/components/layout/DetailFact";
import { DetailHeader } from "@/components/layout/DetailHeader";
import { PageContainer } from "@/components/layout/PageContainer";
import { Skeleton } from "@/components/ui/skeleton";
import { StatusBadge } from "@/components/ui/status-badge";
import { Badge } from "@/components/ui/badge";
import { CredentialsPanel } from "@/routes/governance/service-accounts/CredentialsPanel";
import { MembershipSection } from "@/routes/governance/service-accounts/MembershipSection";
import { SERVICE_ACCOUNTS_BASE } from "@/routes/governance/service-accounts/serviceAccountsUrl";
import { Section, TabError } from "@/routes/governance/tabParts";

/** The generated client types the body as the DTO or an error body; only the DTO is a success. */
async function loadAccount(id: string): Promise<ServiceAccountDetailDto> {
  const body = await getServiceAccount(id);
  if (!("user_id" in body)) throw new Error("The service account could not be loaded.");
  return body;
}

const BACK = { to: SERVICE_ACCOUNTS_BASE, label: "Service accounts" };

/**
 * One service account (SA3): identity, membership, and (Task 4) credentials.
 * Name and description are read-only: the API has no route that updates them.
 */
export default function ServiceAccountDetail() {
  const { id = "" } = useParams();
  const account = useQuery({ queryKey: serviceAccountQueryKey(id), queryFn: () => loadAccount(id), enabled: id !== "" });
  const workspaces = useAdminWorkspacesListWs(undefined, { query: { retry: false } });

  if (account.isPending) {
    return (
      <PageContainer>
        <DetailHeader back={BACK} title={<span className="sr-only">Service account</span>} />
        <Skeleton className="h-32 w-full" />
      </PageContainer>
    );
  }
  if (account.isError) {
    return (
      <PageContainer>
        <DetailHeader back={BACK} title={<span className="sr-only">Service account</span>} />
        <TabError what="the service account" onRetry={() => void account.refetch()} detail={account.error.message} />
      </PageContainer>
    );
  }

  const sa = account.data;
  const workspaceRows = workspaces.data?.status === 200 ? workspaces.data.data : [];
  const workspaceName = sa.workspace_id ? (workspaceRows.find((w) => w.id === sa.workspace_id)?.name ?? sa.workspace_id) : null;

  return (
    <PageContainer>
      <DetailHeader
        back={BACK}
        title={sa.name}
        status={<StatusBadge tone={sa.status === "active" ? "success" : "inactive"}>{sa.status}</StatusBadge>}
        description={sa.description || "No description."}
      />
      <div className="space-y-8">
        <Section title="Identity">
          <p className="text-sm text-muted-foreground">
            A machine identity: it never signs in and can never hold platform_admin.
          </p>
          <div className="flex flex-wrap items-center gap-x-6 gap-y-2 text-sm">
            <span className="inline-flex items-center gap-2">
              <span className="text-xs font-medium uppercase text-muted-foreground">Scope</span>
              <Badge variant="outline">{sa.scope === "workspace" ? "Workspace" : "Platform"}</Badge>
            </span>
            {workspaceName ? <DetailFact icon={Bot} label="Workspace" value={workspaceName} /> : null}
            <DetailFact icon={Bot} label="Account id" value={sa.user_id} />
          </div>
        </Section>
        <MembershipSection account={sa} />
        <CredentialsPanel account={sa} />
      </div>
    </PageContainer>
  );
}
