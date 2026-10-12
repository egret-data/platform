import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@/api/generated/perm/perm");

import * as permApi from "@/api/generated/perm/perm";
import { ApiError } from "@/api/client";
import type { MigrateResponse } from "@/api/generated/model";
import { migrateServicePolicies, undoServicePolicyMigration } from "@/api/policies";

/**
 * The migrate adapter's one decision: a 422 is the gate's ANSWER (same body
 * shape as a 200), so it resolves; every other refusal still throws.
 */
const NOT_PROVED: MigrateResponse = {
  dry_run: true,
  committed: false,
  equivalent: false,
  refusal: "proving this migration needs 15000 probe evaluations across 2 profile(s), 3100000 units of work (probe evaluations x the policies each visits), over the limit of 1300000. …",
  policies: [],
  profiles: [],
  probe_evaluations: 0,
  disagreements: [],
  compile: { services: [], created: 0, updated: 0, deleted: 0, unchanged: 0, skipped: [] },
  notices: [],
};

beforeEach(() => vi.clearAllMocks());

describe("migrateServicePolicies", () => {
  it("sends the names and the dry-run flag, and unwraps a 200", async () => {
    const ok = { ...NOT_PROVED, equivalent: true, refusal: null };
    vi.mocked(permApi.permMigrateServicePolicies).mockResolvedValue({
      status: 200,
      data: ok,
      headers: new Headers(),
    });
    await expect(migrateServicePolicies(7, ["a", "b"], true)).resolves.toEqual(ok);
    expect(permApi.permMigrateServicePolicies).toHaveBeenCalledWith(7, {
      policy_names: ["a", "b"],
      dry_run: true,
    });
  });

  it("resolves a 422 to the gate's answer instead of throwing", async () => {
    vi.mocked(permApi.permMigrateServicePolicies).mockRejectedValue(
      new ApiError(422, "Unprocessable Entity", undefined, NOT_PROVED),
    );
    await expect(migrateServicePolicies(7, ["a"], false)).resolves.toEqual(NOT_PROVED);
  });

  it("still throws a 409, and a 422 whose body is not a migration answer", async () => {
    vi.mocked(permApi.permMigrateServicePolicies).mockRejectedValueOnce(
      new ApiError(409, "conflict: already migrated"),
    );
    await expect(migrateServicePolicies(7, ["a"], true)).rejects.toMatchObject({ status: 409 });

    vi.mocked(permApi.permMigrateServicePolicies).mockRejectedValueOnce(
      new ApiError(422, "unprocessable", undefined, { error: "unprocessable" }),
    );
    await expect(migrateServicePolicies(7, ["a"], true)).rejects.toMatchObject({ status: 422 });
  });
});

describe("undoServicePolicyMigration", () => {
  it("sends the names and unwraps the response", async () => {
    const body = {
      policies: [],
      compile: NOT_PROVED.compile,
      notices: ["stays compiled"],
    };
    vi.mocked(permApi.permUndoServicePolicyMigration).mockResolvedValue({
      status: 200,
      data: body,
      headers: new Headers(),
    });
    await expect(undoServicePolicyMigration(7, ["old-sales"])).resolves.toEqual(body);
    expect(permApi.permUndoServicePolicyMigration).toHaveBeenCalledWith(7, {
      policy_names: ["old-sales"],
    });
  });
});
