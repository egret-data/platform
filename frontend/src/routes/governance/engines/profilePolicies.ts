import { usePolicyAdminListPolicies } from "@/api/generated/policy-admin/policy-admin";
import type { PolicySummary } from "@/api/generated/model";

/** Who owns a policy row (`policy.authz_policies.managed_by`, spec premise 1). */
export type PolicySource = "compiler" | "infra" | "operator";

export interface ProfilePolicies {
  /** The profile's rows, then its tag service's; empty until both reads answered. */
  rows: PolicySummary[];
  isLoading: boolean;
  isError: boolean;
  refetch: () => void;
  /** Whether a row is hand-written (`operator`), which shows the Migrate tab (EP7). */
  hasHandWritten: boolean;
}

/**
 * Every policy of a profile and of its tag service (spec premise 6, §6.2):
 * `GET /policies?service=<profile>` and, when the profile names a tag service,
 * `?service=<tag service>`. A Trino profile's tag-based policies live only in
 * the second.
 */
export function useProfilePolicies(profile: string | null, tagService: string | null): ProfilePolicies {
  const main = usePolicyAdminListPolicies({ service: profile ?? "" }, { query: { enabled: !!profile } });
  const tag = usePolicyAdminListPolicies({ service: tagService ?? "" }, { query: { enabled: !!tagService } });
  const mainRows = main.data?.status === 200 ? main.data.data : null;
  const tagRows = tagService ? (tag.data?.status === 200 ? tag.data.data : null) : [];
  const rows = mainRows && tagRows ? [...mainRows, ...tagRows] : [];
  return {
    rows,
    isLoading: !!profile && (!mainRows || !tagRows) && !main.isError && !tag.isError,
    isError: main.isError || (!!tagService && tag.isError),
    refetch: () => {
      void main.refetch();
      if (tagService) void tag.refetch();
    },
    hasHandWritten: rows.some((r) => r.managed_by === "operator"),
  };
}
