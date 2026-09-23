import { describe, expect, it } from "vitest";
import type { Me } from "@/api/hooks";
import {
  appForPath,
  canEnterControlPlane,
  canEnterGovernance,
  CONTROL_PLANE_PERMS,
  GOVERNANCE_PERMS,
} from "@/auth/appAccess";

const user = (platform_roles: string[], permissions: string[]): Me => ({
  user_id: "u",
  email: "u@example.com",
  platform_roles,
  permissions,
});

// What `data_steward` holds after migrations 0003, 0059, 0080, 0108, 0109, 0148, 0153
// (0060 later dropped `database:read`). It spans both apps. 0148 granted every
// platform role the `catalog_registry:*` counterpart of each `warehouse:*` it
// held, which is where the steward's `catalog_registry:read` originally came
// from; 0153 (SP3c-2, Task 7) then deleted `warehouse:read` itself, so today's
// steward holds `catalog_registry:read` alone, with no `warehouse:*` alongside it.
const STEWARD = user(
  ["data_steward"],
  [
    "workspace:read", "user:read", "policy:read", "policy:create", "policy:update",
    "policy:delete", "policy:seed", "catalog_registry:read", "storage:read",
    "engine:read", "group:read", "role:read", "tag:read", "tag:manage", "dashboard:certify",
    "dataset:certify",
  ],
);

describe("canEnterControlPlane / canEnterGovernance", () => {
  it("admits platform_admin to both with no permissions at all", () => {
    const admin = user(["platform_admin"], []);
    expect(canEnterControlPlane(admin)).toBe(true);
    expect(canEnterGovernance(admin)).toBe(true);
  });

  it("admits a data_steward to both", () => {
    expect(canEnterControlPlane(STEWARD)).toBe(true);
    expect(canEnterGovernance(STEWARD)).toBe(true);
  });

  it("admits an infra-only user to Control Plane and not to Governance", () => {
    const infra = user([], ["engine:read"]);
    expect(canEnterControlPlane(infra)).toBe(true);
    expect(canEnterGovernance(infra)).toBe(false);
  });

  it("admits a policy-only user to Governance and not to Control Plane", () => {
    const policyOnly = user([], ["policy:read"]);
    expect(canEnterGovernance(policyOnly)).toBe(true);
    expect(canEnterControlPlane(policyOnly)).toBe(false);
  });

  // The lock-out SP3c D3 exists to avoid. `catalog_registry:read` is what gates
  // the Catalogs section, and `CONTROL_PLANE_PERMS` decides entry to the whole
  // Control Plane app — not just one page. A user re-gated onto the new key
  // before it joined this list would lose Engines, Storage, Secrets and the
  // rest along with Catalogs, so the key enters the list in the same change
  // that adds the section. (`warehouse:read` left the list in SP3c-2, once
  // `warehouse_admin` was re-gated onto `catalog_registry:*`, Task 7.)
  it("admits a user holding only catalog_registry:read to Control Plane", () => {
    const catalogOnly = user([], ["catalog_registry:read"]);
    expect(canEnterControlPlane(catalogOnly)).toBe(true);
    expect(canEnterGovernance(catalogOnly)).toBe(false);
  });

  it("admits a tag-only user to Governance — the permission both old lists missed", () => {
    const tagOnly = user([], ["tag:read"]);
    expect(canEnterGovernance(tagOnly)).toBe(true);
    expect(canEnterControlPlane(tagOnly)).toBe(false);
  });

  it("does not count a platform role other than platform_admin", () => {
    const roleOnly = user(["data_steward"], []);
    expect(canEnterControlPlane(roleOnly)).toBe(false);
    expect(canEnterGovernance(roleOnly)).toBe(false);
  });

  it("admits nobody without a session or with unrelated permissions", () => {
    for (const me of [null, undefined, user([], ["dashboard:certify", "policy:create"])]) {
      expect(canEnterControlPlane(me)).toBe(false);
      expect(canEnterGovernance(me)).toBe(false);
    }
  });

  it("keeps governance permissions out of the Control Plane list", () => {
    for (const perm of GOVERNANCE_PERMS) expect(CONTROL_PLANE_PERMS).not.toContain(perm);
  });
});

describe("appForPath", () => {
  it.each([
    ["/governance", "governance"],
    ["/governance/", "governance"],
    ["/governance/policies/trino/access/new", "governance"],
    ["/control-plane", "control-plane"],
    ["/control-plane/audit-log", "control-plane"],
    ["/explore/eng/sql-lab", "explore"],
    ["/", "explore"],
    ["/governance-reports", "explore"],
  ])("%s belongs to %s", (pathname, app) => {
    expect(appForPath(pathname)).toBe(app);
  });
});
