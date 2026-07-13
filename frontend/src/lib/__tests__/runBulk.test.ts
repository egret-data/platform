import { runBulk } from "@/lib/runBulk";
vi.mock("sonner", () => ({ toast: Object.assign(vi.fn(), { success: vi.fn(), error: vi.fn() }) }));
import { toast } from "sonner";

beforeEach(() => vi.clearAllMocks());

it("runs op per item, refreshes, and reports success", async () => {
  const op = vi.fn().mockResolvedValue(undefined);
  const refresh = vi.fn();
  const res = await runBulk({ items: ["a", "b"], op, verb: "Disabled", noun: "users", refresh });
  expect(op).toHaveBeenCalledTimes(2);
  expect(refresh).toHaveBeenCalled();
  expect(res.ok).toEqual(["a", "b"]);
  expect(toast.success).toHaveBeenCalledWith("Disabled 2 users.", undefined);
});

it("reports partial failure and offers Undo over succeeded items only", async () => {
  const op = vi
    .fn()
    .mockImplementation((x: string) => (x === "b" ? Promise.reject(new Error()) : Promise.resolve()));
  const undo = vi.fn().mockResolvedValue(undefined);
  const refresh = vi.fn();
  const res = await runBulk({ items: ["a", "b"], op, verb: "Disabled", noun: "users", refresh, undo });
  expect(res.ok).toEqual(["a"]);
  expect(res.failed).toEqual(["b"]);
  const calls = (toast.error as unknown as { mock: { calls: unknown[][] } }).mock.calls;
  const [msg, cfg] = calls[calls.length - 1];
  expect(msg).toBe("Disabled 1 of 2 users; 1 failed.");
  await (cfg as { action: { onClick: () => Promise<void> } }).action.onClick();
  expect(undo).toHaveBeenCalledTimes(1);
  expect(undo).toHaveBeenCalledWith("a");
});
