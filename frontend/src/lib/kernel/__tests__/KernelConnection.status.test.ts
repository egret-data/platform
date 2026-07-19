import { describe, it, expect, vi, beforeEach } from "vitest";

const interruptMock = vi.fn();
const restartMock = vi.fn();
vi.mock("@/api/generated/kernel/kernel", () => ({
  kernelConnect: vi.fn(),
  kernelKernelInterrupt: (...a: unknown[]) => interruptMock(...a),
  kernelKernelRestart: (...a: unknown[]) => restartMock(...a),
}));

import { KernelConnection } from "../KernelConnection";

describe("KernelConnection status stream", () => {
  beforeEach(() => {
    interruptMock.mockReset().mockResolvedValue({});
    restartMock.mockReset().mockResolvedValue({});
  });

  it("emits the current status immediately and stops after unsubscribe", () => {
    const k = new KernelConnection("ws", "nb");
    const seen: string[] = [];
    const off = k.onStatusChange((s) => seen.push(s));
    expect(seen).toEqual(["disconnected"]);
    off();
    k.disconnect();
    expect(seen).toEqual(["disconnected"]);
  });

  it("restart() emits a transient 'restarting' then settles 'disconnected' with no socket", async () => {
    const k = new KernelConnection("ws", "nb");
    const seen: string[] = [];
    k.onStatusChange((s) => seen.push(s));
    await k.restart();
    expect(seen).toContain("restarting");
    expect(k.status).toBe("disconnected");
  });
});
