import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("../generated/control-plane-access/control-plane-access", async () => {
  const actual = await vi.importActual<
    typeof import("../generated/control-plane-access/control-plane-access")
  >("../generated/control-plane-access/control-plane-access");
  return {
    ...actual,
    controlPlaneAccessListServiceAccounts: vi.fn(),
    controlPlaneAccessGetServiceAccount: vi.fn(),
    controlPlaneAccessCreateServiceAccount: vi.fn(),
    controlPlaneAccessDisableServiceAccount: vi.fn(),
    controlPlaneAccessEnableServiceAccount: vi.fn(),
    controlPlaneAccessDeleteServiceAccount: vi.fn(),
    controlPlaneAccessAddServiceAccountGroup: vi.fn(),
    controlPlaneAccessRemoveServiceAccountGroup: vi.fn(),
    controlPlaneAccessAddServiceAccountRole: vi.fn(),
    controlPlaneAccessRemoveServiceAccountRole: vi.fn(),
    controlPlaneAccessListCredentials: vi.fn(),
    controlPlaneAccessCreateCredential: vi.fn(),
    controlPlaneAccessRotateCredential: vi.fn(),
    controlPlaneAccessRevokeCredential: vi.fn(),
    controlPlaneAccessSetCredentialExpiry: vi.fn(),
  };
});

import * as gen from "../generated/control-plane-access/control-plane-access";
import * as sa from "../serviceAccounts";

/** Envelope as orval returns it; the helpers must hand back only `data`. */
const envelope = (data: unknown) => ({ status: 200, data, headers: new Headers() });

type Case = {
  name: string;
  call: () => Promise<unknown>;
  fn: ReturnType<typeof vi.fn>;
  args: unknown[];
};

const cases: Case[] = [
  { name: "listServiceAccounts", call: () => sa.listServiceAccounts({}), fn: vi.mocked(gen.controlPlaneAccessListServiceAccounts), args: [{}] },
  { name: "getServiceAccount", call: () => sa.getServiceAccount("s1"), fn: vi.mocked(gen.controlPlaneAccessGetServiceAccount), args: ["s1"] },
  { name: "disableServiceAccount", call: () => sa.disableServiceAccount("s1"), fn: vi.mocked(gen.controlPlaneAccessDisableServiceAccount), args: ["s1"] },
  { name: "enableServiceAccount", call: () => sa.enableServiceAccount("s1"), fn: vi.mocked(gen.controlPlaneAccessEnableServiceAccount), args: ["s1"] },
  { name: "deleteServiceAccount", call: () => sa.deleteServiceAccount("s1"), fn: vi.mocked(gen.controlPlaneAccessDeleteServiceAccount), args: ["s1"] },
  { name: "removeServiceAccountGroup", call: () => sa.removeServiceAccountGroup("s1", "g1"), fn: vi.mocked(gen.controlPlaneAccessRemoveServiceAccountGroup), args: ["s1", "g1"] },
  { name: "removeServiceAccountRole", call: () => sa.removeServiceAccountRole("s1", "r1"), fn: vi.mocked(gen.controlPlaneAccessRemoveServiceAccountRole), args: ["s1", "r1"] },
  { name: "listCredentials", call: () => sa.listCredentials("s1"), fn: vi.mocked(gen.controlPlaneAccessListCredentials), args: ["s1"] },
  { name: "rotateCredential", call: () => sa.rotateCredential("s1", "c1"), fn: vi.mocked(gen.controlPlaneAccessRotateCredential), args: ["s1", "c1"] },
  { name: "revokeCredential", call: () => sa.revokeCredential("s1", "c1"), fn: vi.mocked(gen.controlPlaneAccessRevokeCredential), args: ["s1", "c1"] },
];

describe("serviceAccounts api", () => {
  beforeEach(() => vi.clearAllMocks());

  it.each(cases)("$name calls the generated fn and unwraps data", async ({ call, fn, args }) => {
    const payload = { marker: "payload" };
    fn.mockResolvedValue(envelope(payload));
    await expect(call()).resolves.toBe(payload);
    expect(fn).toHaveBeenCalledWith(...args);
  });

  it("createServiceAccount passes the body and unwraps data", async () => {
    const body = { name: "svc" } as Parameters<typeof sa.createServiceAccount>[0];
    const payload = { id: "s1" };
    vi.mocked(gen.controlPlaneAccessCreateServiceAccount).mockResolvedValue(envelope(payload) as never);
    await expect(sa.createServiceAccount(body)).resolves.toBe(payload);
    expect(gen.controlPlaneAccessCreateServiceAccount).toHaveBeenCalledWith(body);
  });

  it("addServiceAccountGroup / addServiceAccountRole pass id + body", async () => {
    const gBody = { group_id: "g1" } as Parameters<typeof sa.addServiceAccountGroup>[1];
    const rBody = { role_id: "r1" } as Parameters<typeof sa.addServiceAccountRole>[1];
    vi.mocked(gen.controlPlaneAccessAddServiceAccountGroup).mockResolvedValue(envelope("g") as never);
    vi.mocked(gen.controlPlaneAccessAddServiceAccountRole).mockResolvedValue(envelope("r") as never);
    await expect(sa.addServiceAccountGroup("s1", gBody)).resolves.toBe("g");
    await expect(sa.addServiceAccountRole("s1", rBody)).resolves.toBe("r");
    expect(gen.controlPlaneAccessAddServiceAccountGroup).toHaveBeenCalledWith("s1", gBody);
    expect(gen.controlPlaneAccessAddServiceAccountRole).toHaveBeenCalledWith("s1", rBody);
  });

  it("createCredential / setCredentialExpiry pass ids + body", async () => {
    const cBody = {} as Parameters<typeof sa.createCredential>[1];
    const eBody = {} as Parameters<typeof sa.setCredentialExpiry>[2];
    vi.mocked(gen.controlPlaneAccessCreateCredential).mockResolvedValue(envelope("c") as never);
    vi.mocked(gen.controlPlaneAccessSetCredentialExpiry).mockResolvedValue(envelope("e") as never);
    await expect(sa.createCredential("s1", cBody)).resolves.toBe("c");
    await expect(sa.setCredentialExpiry("s1", "c1", eBody)).resolves.toBe("e");
    expect(gen.controlPlaneAccessCreateCredential).toHaveBeenCalledWith("s1", cBody);
    expect(gen.controlPlaneAccessSetCredentialExpiry).toHaveBeenCalledWith("s1", "c1", eBody);
  });

  it("query keys are stable and parameterized", () => {
    expect(sa.serviceAccountsQueryKey()).toEqual(sa.serviceAccountsQueryKey());
    expect(sa.serviceAccountsQueryKey({})).toEqual(sa.serviceAccountsQueryKey({}));
    expect(sa.serviceAccountQueryKey("a")).toEqual(sa.serviceAccountQueryKey("a"));
    expect(sa.serviceAccountQueryKey("a")).not.toEqual(sa.serviceAccountQueryKey("b"));
    expect(sa.credentialsQueryKey("a")).not.toEqual(sa.credentialsQueryKey("b"));
    expect(sa.credentialsQueryKey("a")).not.toEqual(sa.serviceAccountQueryKey("a"));
  });
});
