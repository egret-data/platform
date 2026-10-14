import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes, useLocation } from "react-router-dom";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { TooltipProvider } from "@/components/ui/tooltip";
import NamespaceKvTab from "@/components/workflows/namespaces/NamespaceKvTab";
import type { KvEntryDto } from "@/api/generated/model";
import { headerUtf8, KV_WS, kvDetail, kvEntry, serveKv, type KvRequest, type KvWorld } from "./fakeKvServer";

vi.mock("@/components/monaco/MonacoEditor", () => import("@/routes/workflows/__tests__/monacoStub"));
const toast = vi.hoisted(() => ({ success: vi.fn(), error: vi.fn(), info: vi.fn() }));
vi.mock("sonner", () => ({ toast }));

/**
 * The KV tab (W7d spec §4.4) over W9c's routes, rendered with the props the
 * namespace frame passes. DATETIME is entered in the viewer's zone, pinned
 * here to one with an offset.
 */
const savedTz = process.env.TZ;
beforeAll(() => {
  process.env.TZ = "Asia/Ho_Chi_Minh";
});
afterAll(() => {
  process.env.TZ = savedTz;
});

const NS = "company.team.etl";

/** Not in key order, nor in any column's order. */
const KEYS: KvEntryDto[] = [
  kvEntry("zeta", { type: "STRING", description: "Last one", updated: "2026-10-02T00:00:00Z" }),
  kvEntry("alpha", { type: "NUMBER", description: "Big number", updated: "2026-10-03T00:00:00Z", expiresAt: "2099-11-01T00:00:00Z" }),
  kvEntry("mid", { type: "JSON", description: null, updated: "2026-10-01T00:00:00Z", expiresAt: "2099-10-20T00:00:00Z" }),
];

const VALUES = {
  alpha: kvDetail("NUMBER", "12345678901234567890.10", { description: "Big number", expiresAt: "2099-11-01T00:00:00Z" }),
  zeta: kvDetail("STRING", '"hello"', { description: "Last one" }),
  mid: kvDetail("JSON", '{"n": 98765432109876543210, "ok": true}'),
};

function Where() {
  const l = useLocation();
  return <output data-testid="where">{`${l.pathname}${l.search}`}</output>;
}

const at = (search = "") => `/explore/${KV_WS}/workflows/namespaces/${NS}/kv${search}`;

let requests: KvRequest[] = [];

function renderTab({ search = "", canManage = true, world = {} }: { search?: string; canManage?: boolean; world?: Partial<KvWorld> } = {}) {
  requests = serveKv({ namespace: NS, keys: KEYS.map((k) => ({ ...k })), values: VALUES, ...world });
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, refetchOnWindowFocus: false } } });
  render(
    <QueryClientProvider client={client}>
      <TooltipProvider>
        <MemoryRouter initialEntries={[at(search)]}>
          <Routes>
            <Route
              path="/explore/:workspace/workflows/namespaces/:namespace/:tab"
              element={
                <>
                  <NamespaceKvTab
                    ws={KV_WS}
                    namespace={NS}
                    canManage={canManage}
                    detail={{ namespace: NS, flows: 0, files: 0, kv_keys: KEYS.length, own: true, can_manage: canManage }}
                  />
                  <Where />
                </>
              }
            />
          </Routes>
        </MemoryRouter>
      </TooltipProvider>
    </QueryClientProvider>,
  );
}

const where = () => screen.getByTestId("where").textContent;
const keyColumn = () =>
  within(screen.getByTestId("kv-table"))
    .getAllByRole("row")
    .slice(1)
    .map((r) => r.getAttribute("data-key"));
const puts = () => requests.filter((r) => r.method === "PUT");
const deletes = () => requests.filter((r) => r.method === "DELETE");

async function openRowMenu(key: string) {
  const user = userEvent.setup();
  await user.click(await screen.findByRole("button", { name: `Actions for ${key}` }));
  return user;
}

beforeEach(() => {
  toast.success.mockReset();
  toast.error.mockReset();
  toast.info.mockReset();
});
afterEach(() => {
  vi.unstubAllGlobals();
});

describe("the table", () => {
  it("has the five columns, sorted by key at first", async () => {
    renderTab();
    const table = await screen.findByTestId("kv-table");
    const headers = within(table).getAllByRole("columnheader").map((h) => h.textContent?.trim());
    expect(headers).toEqual(expect.arrayContaining(["Key", "Type", "Description", "Last modified", "Expiration"]));
    expect(keyColumn()).toEqual(["alpha", "mid", "zeta"]);
    expect(within(table).getByRole("columnheader", { name: /^Key/ })).toHaveAttribute("aria-sort", "ascending");
    const zeta = within(table).getByRole("row", { name: /zeta/ });
    expect(zeta).toHaveTextContent("STRING");
    expect(zeta).toHaveTextContent("Last one");
    expect(zeta).toHaveTextContent("Never");
    expect(screen.getByText("3 keys")).toBeInTheDocument();
  });

  it("sorts by every column, both ways, and keeps the sort in the URL", async () => {
    const user = userEvent.setup();
    renderTab();
    await screen.findByTestId("kv-table");

    await user.click(screen.getByRole("button", { name: "Last modified" }));
    expect(keyColumn()).toEqual(["mid", "zeta", "alpha"]);
    expect(where()).toContain("sort=updated");
    await user.click(screen.getByRole("button", { name: "Last modified" }));
    expect(keyColumn()).toEqual(["alpha", "zeta", "mid"]);
    expect(where()).toContain("dir=desc");

    // Never expires sorts after every date.
    await user.click(screen.getByRole("button", { name: "Expiration" }));
    expect(keyColumn()).toEqual(["mid", "alpha", "zeta"]);
    await user.click(screen.getByRole("button", { name: "Type" }));
    expect(keyColumn()).toEqual(["mid", "alpha", "zeta"]);
    // No description sorts after every description.
    await user.click(screen.getByRole("button", { name: "Description" }));
    expect(keyColumn()).toEqual(["alpha", "zeta", "mid"]);
    await user.click(screen.getByRole("button", { name: "Key" }));
    expect(keyColumn()).toEqual(["alpha", "mid", "zeta"]);
  });

  it("restores a sort from the URL", async () => {
    renderTab({ search: "?sort=updated&dir=desc" });
    await screen.findByTestId("kv-table");
    expect(keyColumn()).toEqual(["alpha", "zeta", "mid"]);
    expect(screen.getByRole("columnheader", { name: /^Last modified/ })).toHaveAttribute("aria-sort", "descending");
  });

  it("filters by key, kept in the URL", async () => {
    const user = userEvent.setup();
    renderTab();
    await screen.findByTestId("kv-table");
    await user.type(screen.getByLabelText("Filter keys"), "ET");
    expect(keyColumn()).toEqual(["zeta"]);
    expect(where()).toContain("key=ET");
  });

  it("says when the filter matches nothing, and clears it", async () => {
    const user = userEvent.setup();
    renderTab({ search: "?key=nothing" });
    expect(await screen.findByText(/No keys match/)).toBeInTheDocument();
    expect(screen.getByLabelText("Filter keys")).toHaveValue("nothing");
    await user.click(screen.getByRole("button", { name: "Clear filter" }));
    expect(keyColumn()).toEqual(["alpha", "mid", "zeta"]);
    expect(where()).not.toContain("key=");
  });

  it("shows an empty namespace as empty, with the way to add a key", async () => {
    renderTab({ world: { keys: [] } });
    expect(await screen.findByText("No KV keys yet")).toBeInTheDocument();
    expect(screen.getAllByRole("button", { name: "Add a key" }).length).toBeGreaterThan(0);
  });
});

describe("row actions", () => {
  it("views the value with every digit, and its details", async () => {
    renderTab();
    const user = await openRowMenu("alpha");
    await user.click(screen.getByRole("menuitem", { name: "View value" }));
    const sheet = await screen.findByRole("dialog", { name: "alpha" });
    expect(await within(sheet).findByLabelText("Value")).toHaveTextContent("12345678901234567890.10");
    expect(sheet).toHaveTextContent("NUMBER");
    expect(sheet).toHaveTextContent("Big number");
    expect(sheet).toHaveTextContent("Revision 3");
    expect(requests.some((r) => r.method === "GET" && r.path.endsWith("/kv/alpha"))).toBe(true);
  });

  it("reads a key named inheritance at a path the inheritance listing does not take", async () => {
    renderTab({
      world: { keys: [kvEntry("inheritance")], values: { inheritance: kvDetail("STRING", '"mine"') } },
    });
    const user = await openRowMenu("inheritance");
    await user.click(screen.getByRole("menuitem", { name: "View value" }));
    const sheet = await screen.findByRole("dialog", { name: "inheritance" });
    expect(await within(sheet).findByLabelText("Value")).toHaveTextContent("mine");
    expect(requests.some((r) => r.method === "GET" && r.path.endsWith("/kv/inheritanc%65"))).toBe(true);
  });

  it("edits a key: the value read back, the key locked, the description and TTL kept", async () => {
    // An hour left, from now: the edit re-sends what is left as the TTL.
    const inAnHour = new Date(Date.now() + 3_600_000).toISOString();
    renderTab({
      world: {
        keys: KEYS.map((k) => (k.key === "alpha" ? { ...k, expiresAt: inAnHour } : { ...k })),
        values: { ...VALUES, alpha: kvDetail("NUMBER", "12345678901234567890.10", { description: "Big number", expiresAt: inAnHour }) },
      },
    });
    const user = await openRowMenu("alpha");
    await user.click(screen.getByRole("menuitem", { name: "Edit" }));
    const sheet = await screen.findByRole("dialog", { name: "Edit alpha" });
    expect(within(sheet).getByLabelText("Key")).toHaveValue("alpha");
    expect(within(sheet).getByLabelText("Key")).toHaveAttribute("readonly");
    expect(await within(sheet).findByLabelText("Value")).toHaveValue("12345678901234567890.10");
    expect(within(sheet).getByLabelText("Description")).toHaveValue("Big number");
    expect(within(sheet).getByRole("combobox", { name: "Expiration" })).toHaveTextContent("Keep the current expiration");

    await user.click(within(sheet).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(puts()).toHaveLength(1));
    const put = puts()[0];
    expect(put.path).toBe(`/api/ws/${KV_WS}/workflows/namespaces/${NS}/kv/alpha`);
    expect(put.body).toBe("12345678901234567890.10");
    expect(headerUtf8(put.headers.get("description"))).toBe("Big number");
    // What is left of the hour, in whole seconds: the test's own run time is the gap.
    const ttl = /^PT(\d+)S$/.exec(put.headers.get("ttl") ?? "");
    expect(ttl).not.toBeNull();
    expect(Number(ttl![1])).toBeGreaterThan(3_500);
    expect(Number(ttl![1])).toBeLessThanOrEqual(3_600);
    // No overwrite question: this is the key being edited.
    expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument();
  });

  it("edits a JSON value without rounding its numbers", async () => {
    renderTab();
    const user = await openRowMenu("mid");
    await user.click(screen.getByRole("menuitem", { name: "Edit" }));
    const sheet = await screen.findByRole("dialog", { name: "Edit mid" });
    const editor = await within(sheet).findByLabelText("Value");
    expect((editor as HTMLTextAreaElement).value).toContain("98765432109876543210");
    await user.click(within(sheet).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(puts()).toHaveLength(1));
    expect(puts()[0].body).toContain("98765432109876543210");
    expect(JSON.parse(puts()[0].body!.replace("98765432109876543210", "1"))).toEqual({ n: 1, ok: true });
    expect(puts()[0].headers.get("ttl")).toBeNull();
  });

  it("copies the expression a flow reads the key with", async () => {
    const writeText = vi.fn(async () => {});
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    renderTab();
    const user = await openRowMenu("zeta");
    // user-event installs its own clipboard on setup; put the spy back.
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    await user.click(screen.getByRole("menuitem", { name: "Copy expression" }));
    await waitFor(() => expect(writeText).toHaveBeenCalledWith("{{ kv('zeta') }}"));
    expect(toast.success).toHaveBeenCalledWith("Copied {{ kv('zeta') }}.");
  });

  it("deletes one key after asking, and refreshes the list", async () => {
    renderTab();
    const user = await openRowMenu("zeta");
    await user.click(screen.getByRole("menuitem", { name: "Delete" }));
    const confirm = await screen.findByRole("alertdialog", { name: "Delete “zeta”?" });
    expect(deletes()).toHaveLength(0);
    await user.click(within(confirm).getByRole("button", { name: "Delete key" }));
    await waitFor(() => expect(deletes()).toHaveLength(1));
    expect(deletes()[0].path).toBe(`/api/ws/${KV_WS}/workflows/namespaces/${NS}/kv/zeta`);
    await waitFor(() => expect(keyColumn()).toEqual(["alpha", "mid"]));
    expect(toast.success).toHaveBeenCalledWith("Deleted the key “zeta”.");
  });

  it("cancels a delete without sending it", async () => {
    renderTab();
    const user = await openRowMenu("zeta");
    await user.click(screen.getByRole("menuitem", { name: "Delete" }));
    await user.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Cancel" }));
    expect(deletes()).toHaveLength(0);
  });
});

describe("bulk delete", () => {
  it("deletes the selected keys in one request", async () => {
    const user = userEvent.setup();
    renderTab();
    await screen.findByTestId("kv-table");
    expect(screen.getByRole("button", { name: "Delete selected" })).toBeDisabled();
    await user.click(screen.getByRole("checkbox", { name: "Select zeta" }));
    await user.click(screen.getByRole("checkbox", { name: "Select alpha" }));
    await user.click(screen.getByRole("button", { name: "Delete selected (2)" }));
    const confirm = await screen.findByRole("alertdialog", { name: "Delete 2 keys?" });
    expect(confirm).toHaveTextContent("alpha");
    expect(confirm).toHaveTextContent("zeta");
    await user.click(within(confirm).getByRole("button", { name: "Delete 2 keys" }));
    await waitFor(() => expect(deletes()).toHaveLength(1));
    expect(deletes()[0].path).toBe(`/api/ws/${KV_WS}/workflows/namespaces/${NS}/kv`);
    expect(JSON.parse(deletes()[0].body!)).toEqual({ keys: ["alpha", "zeta"] });
    await waitFor(() => expect(keyColumn()).toEqual(["mid"]));
    expect(toast.success).toHaveBeenCalledWith("Deleted 2 keys.");
  });

  it("acts only on the selected keys the filter still shows", async () => {
    const user = userEvent.setup();
    renderTab();
    await screen.findByTestId("kv-table");
    await user.click(screen.getByRole("checkbox", { name: "Select all" }));
    await user.type(screen.getByLabelText("Filter keys"), "mid");
    await user.click(screen.getByRole("button", { name: "Delete selected (1)" }));
    await user.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Delete 1 key" }));
    await waitFor(() => expect(deletes()).toHaveLength(1));
    expect(JSON.parse(deletes()[0].body!)).toEqual({ keys: ["mid"] });
  });
});

/** Open "Add a key", fill the key, pick the type and fill the value with `fill`. */
async function addKey(key: string, type: string, fill: (user: ReturnType<typeof userEvent.setup>, sheet: HTMLElement) => Promise<void>) {
  const user = userEvent.setup();
  await screen.findByTestId("kv-table");
  await user.click(screen.getAllByRole("button", { name: "Add a key" })[0]);
  const sheet = await screen.findByRole("dialog", { name: "Add a key" });
  await user.type(within(sheet).getByLabelText("Key"), key);
  if (type !== "STRING") {
    await user.click(within(sheet).getByRole("combobox", { name: "Type" }));
    await user.click(await screen.findByRole("option", { name: type }));
  }
  await fill(user, sheet);
  return { user, sheet };
}

const typeValue = (text: string) => async (user: ReturnType<typeof userEvent.setup>, sheet: HTMLElement) => {
  const field = within(sheet).getByLabelText("Value");
  await user.clear(field);
  if (text) await user.type(field, text.replace(/[{[]/g, (c) => c + c));
};

describe("the create drawer, for each type", () => {
  it.each([
    ["STRING", typeValue('say "hi"'), '"say \\"hi\\""'],
    ["NUMBER", typeValue("12345678901234567890.50"), "12345678901234567890.50"],
    [
      "BOOLEAN",
      async (user: ReturnType<typeof userEvent.setup>, sheet: HTMLElement) => {
        await user.click(within(sheet).getByRole("combobox", { name: "Value" }));
        await user.click(await screen.findByRole("option", { name: "false" }));
      },
      "false",
    ],
    ["DATETIME", typeValue("2026-10-14T09:30"), "2026-10-14T02:30:00.000Z"],
    ["DATE", typeValue("2026-10-14"), "2026-10-14"],
    ["DURATION", typeValue("PT1H30M"), '"PT1H30M"'],
    ["JSON", typeValue('{"a": [1, 2.50]}'), '{"a": [1, 2.50]}'],
  ])("sends a valid %s as its exact body, text/plain, with no TTL", async (type, fill, body) => {
    renderTab();
    const { user, sheet } = await addKey("fresh", type, fill);
    await user.click(within(sheet).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(puts()).toHaveLength(1));
    const put = puts()[0];
    expect(put.path).toBe(`/api/ws/${KV_WS}/workflows/namespaces/${NS}/kv/fresh`);
    expect(put.body).toBe(body);
    expect(put.headers.get("content-type")).toBe("text/plain");
    expect(put.headers.get("ttl")).toBeNull();
    expect(put.headers.get("description")).toBeNull();
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Add a key" })).not.toBeInTheDocument());
    expect(toast.success).toHaveBeenCalledWith("Saved the key “fresh”.");
  });

  it.each([
    ["STRING", typeValue("PT1H"), /reads as a duration.*Choose the DURATION type/],
    ["NUMBER", typeValue("1,5"), /Enter a number/],
    ["BOOLEAN", async () => {}, /Choose true or false/],
    ["DATETIME", typeValue(""), /Enter a date and a time/],
    ["DATE", typeValue(""), /Enter a date/],
    ["DURATION", typeValue("PT1.5S"), /Enter a duration/],
    ["JSON", typeValue("42"), /object.*or an array/],
  ])("refuses an invalid %s beside the field, and sends nothing", async (type, fill, message) => {
    renderTab();
    const { user, sheet } = await addKey("fresh", type, fill);
    await user.click(within(sheet).getByRole("button", { name: "Save" }));
    const error = await within(sheet).findByText(message);
    expect(error).toHaveAttribute("id", "kv-value-error");
    expect(puts()).toHaveLength(0);
  });

  it("refuses a key that breaks the key rule", async () => {
    renderTab();
    const { user, sheet } = await addKey("_bad key", "STRING", typeValue("x"));
    await user.click(within(sheet).getByRole("button", { name: "Save" }));
    expect(await within(sheet).findByText(/starts with a letter or a digit/)).toHaveAttribute("id", "kv-key-error");
    expect(puts()).toHaveLength(0);
  });

  it("sends a description in any language as its UTF-8 bytes", async () => {
    renderTab();
    const { user, sheet } = await addKey("fresh", "STRING", typeValue("x"));
    await user.type(within(sheet).getByLabelText("Description"), "Mã dùng chung");
    await user.click(within(sheet).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(puts()).toHaveLength(1));
    expect(headerUtf8(puts()[0].headers.get("description"))).toBe("Mã dùng chung");
  });
});

describe("TTL and overwriting", () => {
  it("sends a chosen TTL", async () => {
    renderTab();
    const { user, sheet } = await addKey("fresh", "STRING", typeValue("x"));
    await user.click(within(sheet).getByRole("combobox", { name: "Expiration" }));
    await user.click(await screen.findByRole("option", { name: "1 hour" }));
    await user.click(within(sheet).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(puts()).toHaveLength(1));
    expect(puts()[0].headers.get("ttl")).toBe("PT1H");
  });

  it("sends a custom TTL, and refuses a negative one", async () => {
    renderTab();
    const { user, sheet } = await addKey("fresh", "STRING", typeValue("x"));
    await user.click(within(sheet).getByRole("combobox", { name: "Expiration" }));
    await user.click(await screen.findByRole("option", { name: "Custom TTL" }));
    await user.type(within(sheet).getByLabelText("TTL"), "-PT1H");
    await user.click(within(sheet).getByRole("button", { name: "Save" }));
    expect(await within(sheet).findByText(/cannot be negative/)).toHaveAttribute("id", "kv-ttl-error");
    expect(puts()).toHaveLength(0);
    await user.clear(within(sheet).getByLabelText("TTL"));
    await user.type(within(sheet).getByLabelText("TTL"), "P2DT6H");
    await user.click(within(sheet).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(puts()).toHaveLength(1));
    expect(puts()[0].headers.get("ttl")).toBe("P2DT6H");
  });

  it("asks before overwriting a key that exists, and sends nothing on Cancel", async () => {
    renderTab();
    const { user, sheet } = await addKey("zeta", "STRING", typeValue("new"));
    await user.click(within(sheet).getByRole("button", { name: "Save" }));
    const ask = await screen.findByRole("alertdialog", { name: "Overwrite “zeta”?" });
    expect(puts()).toHaveLength(0);
    await user.click(within(ask).getByRole("button", { name: "Cancel" }));
    expect(puts()).toHaveLength(0);
    // The form is still there, as typed.
    expect(within(sheet).getByLabelText("Value")).toHaveValue("new");

    await user.click(within(sheet).getByRole("button", { name: "Save" }));
    await user.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Overwrite" }));
    await waitFor(() => expect(puts()).toHaveLength(1));
    expect(puts()[0].body).toBe('"new"');
  });
});

describe("inherited keys", () => {
  it("lists the parents' keys, the nearest parent winning, and marks the ones this namespace overrides", async () => {
    const user = userEvent.setup();
    renderTab({
      world: {
        // As the server sends them: deepest namespace first.
        inherited: [
          { ...kvEntry("shared", { type: "NUMBER" }), namespace: "company.team" },
          { ...kvEntry("zeta", { type: "STRING", description: "From the top" }), namespace: "company" },
        ],
      },
    });
    await screen.findByTestId("kv-table");
    await user.click(screen.getByRole("button", { name: "Inherited keys" }));
    const sheet = await screen.findByRole("dialog", { name: "Inherited KV" });
    const rows = await within(sheet).findAllByRole("row");
    expect(rows.slice(1).map((r) => r.getAttribute("data-key"))).toEqual(["shared", "zeta"]);
    const zeta = within(sheet).getByRole("row", { name: /zeta/ });
    expect(zeta).toHaveTextContent("Overridden by this namespace");
    expect(within(zeta).getByRole("link", { name: "company" })).toHaveAttribute(
      "href",
      `/explore/${KV_WS}/workflows/namespaces/company/kv`,
    );
    expect(within(sheet).getByRole("row", { name: /shared/ })).not.toHaveTextContent("Overridden");
  });
});

describe("the inherited drawer's nearest-parent rule", () => {
  it("keeps the deepest namespace's copy of a key the parents both hold", async () => {
    const user = userEvent.setup();
    renderTab({
      world: {
        inherited: [
          { ...kvEntry("token", { type: "STRING", description: "team" }), namespace: "company.team" },
          { ...kvEntry("token", { type: "STRING", description: "top" }), namespace: "company" },
        ],
      },
    });
    await screen.findByTestId("kv-table");
    await user.click(screen.getByRole("button", { name: "Inherited keys" }));
    const sheet = await screen.findByRole("dialog", { name: "Inherited KV" });
    const rows = (await within(sheet).findAllByRole("row")).slice(1);
    expect(rows).toHaveLength(1);
    expect(rows[0]).toHaveTextContent("company.team");
    expect(rows[0]).toHaveTextContent("team");
    expect(rows[0]).not.toHaveTextContent("top");
  });
});

describe("a read-only viewer", () => {
  it("sees every key and value, with each write disabled and its reason shown", async () => {
    renderTab({ canManage: false });
    await screen.findByTestId("kv-table");
    const reason = "Changing KV keys needs the workflow:manage permission";
    const add = screen.getByRole("button", { name: "Add a key" });
    expect(add).toBeDisabled();
    expect(add).toHaveAccessibleDescription(reason);
    expect(screen.getByRole("button", { name: /^Delete selected/ })).toHaveAccessibleDescription(reason);
    expect(screen.queryByRole("checkbox")).not.toBeInTheDocument();

    const user = await openRowMenu("alpha");
    expect(screen.getByRole("menuitem", { name: /^Edit/ })).toHaveAttribute("aria-disabled", "true");
    expect(screen.getByRole("menuitem", { name: /^Edit/ })).toHaveTextContent(reason);
    expect(screen.getByRole("menuitem", { name: /^Delete/ })).toHaveAttribute("aria-disabled", "true");
    await user.click(screen.getByRole("menuitem", { name: "View value" }));
    const sheet = await screen.findByRole("dialog", { name: "alpha" });
    expect(await within(sheet).findByLabelText("Value")).toHaveTextContent("12345678901234567890.10");
    expect(within(sheet).getByRole("button", { name: "Edit" })).toBeDisabled();
  });
});

describe("errors", () => {
  it("shows a refused namespace's message (an empty segment) and what to do", async () => {
    renderTab({
      world: {
        listReply: { status: 400, body: { error: "invalid input: namespace 'a..b' has an empty segment" } },
      },
    });
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("invalid input: namespace 'a..b' has an empty segment");
    expect(alert).toHaveTextContent(/cannot hold KV keys/);
    expect(screen.queryByTestId("kv-table")).not.toBeInTheDocument();
  });

  it("offers a retry when the store is unavailable", async () => {
    const user = userEvent.setup();
    renderTab({ world: { listReply: { status: 503, body: { error: "internal storage is not configured" } } } });
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("The KV keys could not be loaded: internal storage is not configured.");
    expect(alert).toHaveTextContent(/try again/i);
    await user.click(within(alert).getByRole("button", { name: "Retry" }));
    await waitFor(() => expect(requests.filter((r) => r.method === "GET").length).toBe(2));
  });

  it("shows a refused write's message in the drawer and keeps what was typed", async () => {
    renderTab({ world: { putReply: { status: 403, body: { error: "forbidden" } } } });
    const { user, sheet } = await addKey("fresh", "STRING", typeValue("kept"));
    await user.click(within(sheet).getByRole("button", { name: "Save" }));
    const alert = await within(sheet).findByRole("alert");
    expect(alert).toHaveTextContent("The key was not saved: forbidden.");
    expect(alert).toHaveTextContent("needs the workflow:manage permission");
    expect(within(sheet).getByLabelText("Value")).toHaveValue("kept");
    expect(within(sheet).getByLabelText("Key")).toHaveValue("fresh");
  });

  it("says when the server could not be reached", async () => {
    renderTab({ world: { putReply: new TypeError("Failed to fetch") } });
    const { user, sheet } = await addKey("fresh", "STRING", typeValue("x"));
    await user.click(within(sheet).getByRole("button", { name: "Save" }));
    expect(await within(sheet).findByRole("alert")).toHaveTextContent(
      "The key was not saved: the server could not be reached. Check your connection and try again.",
    );
  });

  it("shows a value that can no longer be read", async () => {
    renderTab({ world: { values: {} } });
    const user = await openRowMenu("zeta");
    await user.click(screen.getByRole("menuitem", { name: "View value" }));
    const sheet = await screen.findByRole("dialog", { name: "zeta" });
    expect(await within(sheet).findByRole("alert")).toHaveTextContent(
      "The value could not be read: No value found for key 'zeta' in namespace 'company.team.etl'. It may have been deleted or have expired; refresh the list.",
    );
  });
});
