import { describe, it, expect, vi, beforeEach } from "vitest";
import { toast } from "sonner";
import {
  AUTO_CONNECT_TOAST_MS,
  AUTO_CONNECT_UNDO_TOAST_MS,
  applyAutoConnect,
  autoConnectDoneMessage,
  autoConnectPrompt,
  findAutoConnectTargets,
  mappableColumns,
  offerAutoConnect,
  revertAutoConnect,
  type AutoConnectCard,
} from "../autoConnect";

vi.mock("sonner", () => ({ toast: vi.fn() }));

const card = (over: Partial<AutoConnectCard>): AutoConnectCard => ({
  id: "c1", card_type: "chart", options: {}, tabKey: "t1", ...over,
});

const cards: AutoConnectCard[] = [
  card({ id: "src" }),
  card({ id: "same-col" }),
  card({ id: "other-col" }),
  card({ id: "already", options: { paramMappings: { p1: "region" } } }),
  card({ id: "other-tab", tabKey: "t2" }),
  card({ id: "text", card_type: "text" }),
];

const columnsOf = (id: string) => (id === "other-col" ? ["country"] : ["region", "total"]);

const find = () =>
  findAutoConnectTargets({ cards, columnsOf, paramId: "p1", column: "region", sourceCardId: "src", tabKey: "t1" });

describe("findAutoConnectTargets", () => {
  it("takes chart cards on this tab that have the column and no mapping yet", () => {
    expect(find()).toEqual([{ cardId: "same-col", column: "region" }]);
  });

  it("excludes the source card even when it has the column", () => {
    expect(find().some((t) => t.cardId === "src")).toBe(false);
  });

  it("returns nothing when the column is not mappable anywhere else", () => {
    expect(findAutoConnectTargets({
      cards, columnsOf, paramId: "p1", column: "total", sourceCardId: "src", tabKey: "t1",
    })).toEqual([{ cardId: "same-col", column: "total" }]);
    expect(findAutoConnectTargets({
      cards, columnsOf, paramId: "p1", column: "nope", sourceCardId: "src", tabKey: "t1",
    })).toEqual([]);
  });
});

describe("applyAutoConnect / revertAutoConnect", () => {
  const targets = [{ cardId: "same-col", column: "region" }];

  it("writes the mapping without disturbing the card's other options", () => {
    const before = [card({ id: "same-col", options: { autoHeight: true, paramMappings: { p9: "x" } } })];
    const after = applyAutoConnect(before, "p1", targets);
    expect(after[0].options).toEqual({ autoHeight: true, paramMappings: { p9: "x", p1: "region" } });
    expect(before[0].options).toEqual({ autoHeight: true, paramMappings: { p9: "x" } });  // no mutation
  });

  it("undo removes exactly the mappings it added and nothing else", () => {
    const before = [card({ id: "same-col", options: { paramMappings: { p9: "x" } } })];
    const after = revertAutoConnect(applyAutoConnect(before, "p1", targets), "p1", targets);
    expect(after[0].options).toEqual({ paramMappings: { p9: "x" } });
  });

  it("undo leaves a card the user had mapped by hand alone", () => {
    // `targets` never contains an already-mapped card, so a blanket delete-by-param-id would
    // destroy a hand-made mapping that auto-connect never touched.
    const before = [card({ id: "already", options: { paramMappings: { p1: "region" } } })];
    expect(revertAutoConnect(before, "p1", targets)[0].options).toEqual({ paramMappings: { p1: "region" } });
  });
});

describe("mappableColumns", () => {
  const spec = { dimensions: ["region"], columns: ["city"], temporal_units: { created_at: "month" } };
  it("offers dimensions and columns, or the temporally grouped columns for a time-grouping filter", () => {
    expect(mappableColumns(spec, false)).toEqual(["region", "city"]);
    expect(mappableColumns(spec, true)).toEqual(["created_at"]);
    expect(mappableColumns({ dimensions: [], columns: [] }, true)).toEqual([]);
  });
});

describe("the toast copy", () => {
  it("matches Metabase's wording and timeouts", () => {
    // toasts.ts:41-43 and constants.ts:1-2.
    expect(autoConnectPrompt("Region", false)).toBe("Auto-connect this filter to all questions containing “Region”?");
    expect(autoConnectPrompt("Region", true)).toBe("Auto-connect this filter to all questions containing “Region”, in the current tab?");
    expect(autoConnectDoneMessage("Region")).toBe("The filter was auto-connected to all questions containing “Region”.");
    expect(AUTO_CONNECT_TOAST_MS).toBe(12000);
    expect(AUTO_CONNECT_UNDO_TOAST_MS).toBe(8000);
  });
});

/** sonner types `action` as `Action | ReactNode`, so the callback is not reachable without
 *  narrowing. Narrow it here rather than in the module: the module's job is to hand sonner an
 *  Action, and this is the test reaching back in to press the button a user would press. */
const actionOf = (opts: unknown): { label: string; onClick: (e: MouseEvent) => void } =>
  (opts as { action: { label: string; onClick: (e: MouseEvent) => void } }).action;

describe("offerAutoConnect", () => {
  beforeEach(() => vi.mocked(toast).mockClear());

  it("offers the connection, and only after the user accepts does it connect and offer undo", () => {
    const connect = vi.fn();
    const revert = vi.fn();
    offerAutoConnect({
      targets: [{ cardId: "same-col", column: "region" }],
      columnName: "Region", hasMultipleTabs: false, connect, revert,
    });

    expect(connect).not.toHaveBeenCalled();          // nothing happens until the user asks
    const [msg, opts] = vi.mocked(toast).mock.calls[0];
    expect(msg).toBe("Auto-connect this filter to all questions containing “Region”?");
    expect(opts?.duration).toBe(AUTO_CONNECT_TOAST_MS);

    expect(actionOf(opts).label).toBe("Auto-connect");
    actionOf(opts).onClick(new MouseEvent("click"));
    expect(connect).toHaveBeenCalledTimes(1);

    const [undoMsg, undoOpts] = vi.mocked(toast).mock.calls[1];
    expect(undoMsg).toBe("The filter was auto-connected to all questions containing “Region”.");
    expect(undoOpts?.duration).toBe(AUTO_CONNECT_UNDO_TOAST_MS);
    expect(actionOf(undoOpts).label).toBe("Undo");
    actionOf(undoOpts).onClick(new MouseEvent("click"));
    expect(revert).toHaveBeenCalledTimes(1);
  });
});
