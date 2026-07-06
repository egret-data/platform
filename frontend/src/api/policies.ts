import { apiJson } from "./client";

// ── Policies (Ranger-backed / control-plane) ─────────────────────────────────

export interface PolicySummary {
  id: number;
  name: string;
  service: string;
  policy_type: "access" | "masking" | "rowfilter";
  resource_path: string;
  principals: string[];
  permissions: string[];
  is_enabled: boolean;
}

export interface ProfileEngine {
  id: string;
  name: string;
  status: string;
}

export interface Profile {
  id: number;
  name: string;
  type: string;
  policy_version: number;
  engines: ProfileEngine[];
}

export function listProfiles(): Promise<Profile[]> {
  return apiJson<Profile[]>("/control-plane/policies/services");
}

export function listPolicies(service: string, type?: string): Promise<PolicySummary[]> {
  const p = new URLSearchParams({ service });
  if (type) p.set("type", type);
  return apiJson<PolicySummary[]>(`/control-plane/policies?${p.toString()}`);
}
