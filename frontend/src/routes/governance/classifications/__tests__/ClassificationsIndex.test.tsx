import { afterEach, describe, expect, it, vi } from "vitest";
import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import {
  ADMIN,
  created,
  fail,
  ok,
  openArea,
  serve,
  STEWARD,
  tag,
  TAG_MANAGER,
  TAG_READER,
  where,
} from "./harness";

afterEach(() => vi.unstubAllGlobals());

const T = { timeout: 3000 };
const INDEX = "/governance/classifications";

/**
 * Not sorted by name, and shaped so a plausible wrong reading shows: a tag
 * with columns only and one with BI objects only are both used; only
 * `internal` carries nothing anywhere.
 */
const TAGS = [
  tag({
    id: "t-pii",
    key: "pii",
    name: "PII",
    description: "Personally identifiable information",
    catalog_tables: 1,
    catalog_columns: 1,
    assignment_count: 2,
    required_roles: ["cleared"],
  }),
  tag({ id: "t-int", key: "internal", name: "Internal" }),
  tag({ id: "t-fin", key: "finance", name: "Finance", assignment_count: 1, required_groups: ["g1", "g2"] }),
  tag({
    id: "t-col",
    key: "colonly",
    name: "Columns only",
    catalog_columns: 2,
    required_roles: ["a"],
    required_groups: ["g1"],
  }),
];

const rowOf = (name: string) => screen.getByRole("link", { name }).closest("tr") as HTMLElement;

describe("the Classification index", () => {
  it("is a list page titled Classifications, rows sorted by name", async () => {
    serve({ "/tags": ok(TAGS) });
    openArea(INDEX, TAG_READER);
    expect(await screen.findByRole("heading", { level: 1, name: "Classifications" }, T)).toBeInTheDocument();
    await screen.findByRole("link", { name: "PII" }, T);
    const names = screen.getAllByRole("row").slice(1).map((r) => within(r).getAllByRole("cell")[0].textContent);
    expect(names).toEqual(["Columns only", "Finance", "Internal", "PII"]);
  });

  it("summarises the catalogue: total, with clearance, unused (no Data and no BI)", async () => {
    serve({ "/tags": ok(TAGS) });
    openArea(INDEX, TAG_READER);
    await screen.findByRole("link", { name: "PII" }, T);
    expect(screen.getByText("classifications")).toHaveTextContent("4classifications");
    expect(screen.getByText("with clearance")).toHaveTextContent("3with clearance");
    expect(screen.getByText("unused")).toHaveTextContent("1unused");
  });

  it("writes the singular for one classification", async () => {
    serve({ "/tags": ok([TAGS[1]]) });
    openArea(INDEX, TAG_READER);
    await screen.findByRole("link", { name: "Internal" }, T);
    expect(screen.getByText("classification")).toHaveTextContent("1classification");
  });

  it("shows name, key, description, Data counts, BI objects and clearance per row", async () => {
    serve({ "/tags": ok(TAGS) });
    openArea(INDEX, TAG_READER);
    await screen.findByRole("link", { name: "PII" }, T);
    expect(screen.getAllByRole("columnheader").map((h) => h.textContent)).toEqual([
      "Name",
      "Key",
      "Description",
      "Data",
      "BI objects",
      "Clearance",
    ]);
    const pii = within(rowOf("PII")).getAllByRole("cell").map((c) => c.textContent);
    expect(pii).toEqual(["PII", "pii", "Personally identifiable information", "1 table · 1 column", "2", "1 role"]);
    const internal = within(rowOf("Internal")).getAllByRole("cell").map((c) => c.textContent);
    expect(internal).toEqual(["Internal", "internal", "—", "—", "0", "None"]);
    expect(within(rowOf("Columns only")).getAllByRole("cell")[3]).toHaveTextContent("0 tables · 2 columns");
    expect(within(rowOf("Columns only")).getAllByRole("cell")[5]).toHaveTextContent("1 role · 1 group");
    expect(within(rowOf("Finance")).getAllByRole("cell")[5]).toHaveTextContent("2 groups");
  });

  it("marks a clearance with a lock icon beside the words, never colour alone", async () => {
    serve({ "/tags": ok(TAGS) });
    openArea(INDEX, TAG_READER);
    await screen.findByRole("link", { name: "PII" }, T);
    const gated = within(rowOf("PII")).getAllByRole("cell")[5];
    expect(gated.querySelector("svg")).not.toBeNull();
    expect(gated).toHaveTextContent("1 role");
    expect(within(rowOf("Internal")).getAllByRole("cell")[5].querySelector("svg")).toBeNull();
  });

  it("links each name to its classification page", async () => {
    serve({ "/tags": ok(TAGS) });
    openArea(INDEX, TAG_READER);
    expect(await screen.findByRole("link", { name: "PII" }, T)).toHaveAttribute("href", "/governance/classifications/t-pii");
  });

  it("hides New classification without tag:manage or platform admin", async () => {
    serve({ "/tags": ok(TAGS) });
    openArea(INDEX, TAG_READER);
    await screen.findByRole("link", { name: "PII" }, T);
    expect(screen.queryByRole("button", { name: "New classification" })).not.toBeInTheDocument();
  });

  it.each([
    ["tag:manage", TAG_MANAGER],
    ["platform admin", ADMIN],
  ])("offers New classification to a %s holder", async (_who, as) => {
    serve({ "/tags": ok(TAGS) });
    openArea(INDEX, as);
    expect(await screen.findByRole("button", { name: "New classification" }, T)).toBeInTheDocument();
  });

  it("derives the key from the name until the key is edited", async () => {
    const user = userEvent.setup();
    serve({ "/tags": ok(TAGS) });
    openArea(INDEX, STEWARD);
    await user.click(await screen.findByRole("button", { name: "New classification" }, T));
    await user.type(screen.getByLabelText("Name"), "Personally Identifiable Info");
    expect(screen.getByLabelText("Key")).toHaveValue("personally-identifiable-info");
    await user.clear(screen.getByLabelText("Key"));
    await user.type(screen.getByLabelText("Key"), "pi");
    await user.type(screen.getByLabelText("Name"), " v2");
    expect(screen.getByLabelText("Key")).toHaveValue("pi");
  });

  it("creates with the entered fields and opens the new classification's page (201 is success)", async () => {
    const user = userEvent.setup();
    const served = serve({
      "/tags": ok(TAGS),
      "POST /tags": created(tag({ id: "t-new", key: "confidential", name: "Confidential" })),
    });
    openArea(INDEX, STEWARD);
    await user.click(await screen.findByRole("button", { name: "New classification" }, T));
    await user.type(screen.getByLabelText("Name"), "Confidential");
    await user.type(screen.getByLabelText("Description"), "Highly sensitive");
    await user.click(screen.getByRole("button", { name: "Create classification" }));
    await waitFor(() => expect(where()).toBe("/governance/classifications/t-new"), T);
    expect(served.bodies["POST /tags"]).toEqual([
      expect.objectContaining({ key: "confidential", name: "Confidential", description: "Highly sensitive" }),
    ]);
  });

  it("shows a 409 beside Key with the dialog open and every input kept", async () => {
    const user = userEvent.setup();
    serve({ "/tags": ok(TAGS), "POST /tags": fail(409, "tag key 'pii' already exists") });
    openArea(INDEX, STEWARD);
    await user.click(await screen.findByRole("button", { name: "New classification" }, T));
    await user.type(screen.getByLabelText("Name"), "PII");
    await user.type(screen.getByLabelText("Description"), "dup");
    await user.click(screen.getByRole("button", { name: "Create classification" }));
    const message = await screen.findByText("tag key 'pii' already exists", {}, T);
    // Beside Key: the Key input names it as its description, not a banner over the form.
    expect(message.id).not.toBe("");
    expect(screen.getByLabelText("Key").getAttribute("aria-describedby")?.split(" ")).toContain(message.id);
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    expect(screen.getByLabelText("Name")).toHaveValue("PII");
    expect(screen.getByLabelText("Key")).toHaveValue("pii");
    expect(screen.getByLabelText("Description")).toHaveValue("dup");
    expect(where()).toBe(INDEX);
  });

  it("shows a failing list as an error with Retry, never as the empty state", async () => {
    const user = userEvent.setup();
    const served = serve({ "/tags": fail(500) });
    openArea(INDEX, STEWARD);
    expect(
      await screen.findByText("The list of classifications could not be loaded. Retry, or reload the page.", {}, T),
    ).toBeInTheDocument();
    expect(screen.queryByText("No classification yet.")).not.toBeInTheDocument();
    const before = served.asked.filter((a) => a === "GET /tags").length;
    await user.click(screen.getByRole("button", { name: "Retry" }));
    await waitFor(() => expect(served.asked.filter((a) => a === "GET /tags").length).toBe(before + 1));
  });

  it("shows a skeleton while the list loads", async () => {
    serve({ "/tags": "never" });
    openArea(INDEX, TAG_READER);
    expect(await screen.findByTestId("list-skeleton", {}, T)).toBeInTheDocument();
  });

  it("shows the empty state, with New classification when allowed", async () => {
    serve({ "/tags": ok([]) });
    openArea(INDEX, STEWARD);
    expect(await screen.findByText("No classification yet.", {}, T)).toBeInTheDocument();
    expect(screen.getAllByRole("button", { name: "New classification" })).toHaveLength(2);
  });

  it("says when the search matches nothing, and clears it", async () => {
    const user = userEvent.setup();
    serve({ "/tags": ok(TAGS) });
    openArea(INDEX, TAG_READER);
    await screen.findByRole("link", { name: "PII" }, T);
    await user.type(screen.getByRole("searchbox", { name: "Search classifications" }), "zzz");
    expect(screen.getByText("No classification matches this search.")).toBeInTheDocument();
    expect(screen.queryByText("No classification yet.")).not.toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Clear search" }));
    expect(screen.getByRole("link", { name: "PII" })).toBeInTheDocument();
    expect(screen.getByRole("searchbox", { name: "Search classifications" })).toHaveValue("");
  });

  it("keeps the search in ?q=", async () => {
    serve({ "/tags": ok(TAGS) });
    openArea(`${INDEX}?q=fin`, TAG_READER);
    await screen.findByRole("link", { name: "Finance" }, T);
    expect(screen.queryByRole("link", { name: "PII" })).not.toBeInTheDocument();
    expect(screen.getByRole("searchbox", { name: "Search classifications" })).toHaveValue("fin");
  });
});
