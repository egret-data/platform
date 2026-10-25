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
import { ACTIONS_CELL } from "@/routes/governance/tabParts";

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
    expect(
      screen.getByText("Classifications label data and BI objects, and set the clearance each one requires."),
    ).toBeInTheDocument();
    expect(screen.queryByText(/Tags that classify/)).not.toBeInTheDocument();
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

  it("drops Key, Description and BI objects below md, keeping Name, Data and Clearance", async () => {
    serve({ "/tags": ok(TAGS) });
    openArea(INDEX, STEWARD);
    await screen.findByRole("link", { name: "PII" }, T);
    const table = screen.getByRole("table");
    for (const name of ["Key", "Description", "BI objects"]) {
      expect(within(table).getByRole("columnheader", { name: new RegExp(name) })).toHaveClass("hidden", "md:table-cell");
    }
    // The cells follow their header, so a hidden column leaves no orphan cell.
    expect(within(rowOf("PII")).getByText("pii").closest("td")).toHaveClass("hidden", "md:table-cell");
    for (const name of ["Name", "Data", "Clearance"]) {
      expect(within(table).getByRole("columnheader", { name: new RegExp(name) })).not.toHaveClass("hidden");
    }
  });

  // #729 follow-up (U3): at 390 px the actions sat past the table's scroll edge (measured at x 625 in a
  // 244 px scroller). jsdom has no layout: this pins the class that keeps them in view, as on other governance tables.
  it("pins the actions column to the right edge of the table's scroller", async () => {
    serve({ "/tags": ok(TAGS) });
    openArea(INDEX, STEWARD);
    const actions = await screen.findByRole("button", { name: "Actions for PII" }, T);
    // As sized for one menu button: the pinned column's fixed width gives way to `w-auto`.
    const pinned = ACTIONS_CELL.split(" ").filter((c) => c !== "w-24");
    expect(actions.closest("td")).toHaveClass(...pinned);
    const headers = within(screen.getByRole("table")).getAllByRole("columnheader");
    expect(headers[headers.length - 1]).toHaveClass(...pinned);
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

  it("closing New classification returns focus to the button that opened it (live check D4)", async () => {
    const user = userEvent.setup();
    serve({ "/tags": ok(TAGS) });
    openArea(INDEX, STEWARD);
    const opener = await screen.findByRole("button", { name: "New classification" }, T);
    await user.click(opener);
    expect(await screen.findByRole("dialog", { name: "New classification" }, T)).toBeInTheDocument();
    await user.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    expect(opener).toHaveFocus();
  });

  it("derives the key from the name until the key is edited", async () => {
    const user = userEvent.setup();
    serve({ "/tags": ok(TAGS) });
    openArea(INDEX, STEWARD);
    await user.click(await screen.findByRole("button", { name: "New classification" }, T));
    await user.type(screen.getByLabelText(/^Name/), "Personally Identifiable Info");
    expect(screen.getByLabelText(/^Key/)).toHaveValue("personally-identifiable-info");
    await user.clear(screen.getByLabelText(/^Key/));
    await user.type(screen.getByLabelText(/^Key/), "pi");
    await user.type(screen.getByLabelText(/^Name/), " v2");
    expect(screen.getByLabelText(/^Key/)).toHaveValue("pi");
  });

  it("creates with the entered fields and opens the new classification's page (201 is success)", async () => {
    const user = userEvent.setup();
    const served = serve({
      "/tags": ok(TAGS),
      "POST /tags": created(tag({ id: "t-new", key: "confidential", name: "Confidential" })),
    });
    openArea(INDEX, STEWARD);
    await user.click(await screen.findByRole("button", { name: "New classification" }, T));
    await user.type(screen.getByLabelText(/^Name/), "Confidential");
    await user.type(screen.getByLabelText("Description"), "Highly sensitive");
    await user.click(screen.getByRole("button", { name: "Create classification" }));
    await waitFor(() => expect(where()).toBe("/governance/classifications/t-new"), T);
    expect(served.bodies["POST /tags"]).toEqual([
      expect.objectContaining({ key: "confidential", name: "Confidential", description: "Highly sensitive" }),
    ]);
  });

  it("an uncoded refusal shows the server's message over the form, every input kept", async () => {
    const user = userEvent.setup();
    serve({ "/tags": ok(TAGS), "POST /tags": fail(409, "something else went wrong") });
    openArea(INDEX, STEWARD);
    await user.click(await screen.findByRole("button", { name: "New classification" }, T));
    await user.type(screen.getByLabelText(/^Name/), "Secret");
    await user.type(screen.getByLabelText("Description"), "dup");
    await user.click(screen.getByRole("button", { name: "Create classification" }));
    expect(await screen.findByText("something else went wrong", {}, T)).toBeInTheDocument();
    expect(screen.getByLabelText(/^Name/)).toHaveValue("Secret");
    expect(screen.getByLabelText(/^Key/)).toHaveValue("secret");
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

  it("explains the three steps when there is no classification yet", async () => {
    serve({ "/tags": ok([]) });
    openArea(INDEX, STEWARD);
    const steps = await screen.findByRole("list", { name: "How classification works" }, T);
    expect(within(steps).getAllByRole("listitem").map((li) => li.textContent)).toEqual([
      "Create a classification.",
      "Attach it to tables or columns in Data.",
      "Add a rule (mask or row filter) or set clearance on the classification's page.",
    ]);
    expect(within(steps).getByRole("link", { name: "Data" })).toHaveAttribute("href", "/governance/data");
    expect(screen.queryByText("Creating needs the tag:manage permission.")).not.toBeInTheDocument();
    // The steward cannot attach in Data, and is told so.
    expect(screen.getByText("Attaching needs the catalog_registry:update permission.")).toBeInTheDocument();
  });

  it("says nothing of a missing permission to a platform admin", async () => {
    serve({ "/tags": ok([]) });
    openArea(INDEX, ADMIN);
    await screen.findByRole("list", { name: "How classification works" }, T);
    expect(screen.queryByText(/needs the/)).not.toBeInTheDocument();
  });

  it("without tag:manage, the steps say what creating needs", async () => {
    serve({ "/tags": ok([]) });
    openArea(INDEX, TAG_READER);
    await screen.findByRole("list", { name: "How classification works" }, T);
    expect(screen.getByText("Creating needs the tag:manage permission.")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "New classification" })).not.toBeInTheDocument();
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

describe("New classification: rules said inline (C10)", () => {
  it("slugs without underscores and flags a key the server would refuse as you type", async () => {
    const user = userEvent.setup();
    serve({ "/tags": ok([]) });
    openArea(INDEX, STEWARD);
    await user.click(await screen.findByRole("button", { name: "New classification" }, T));
    await user.type(screen.getByLabelText(/^Name/), "Card_no data");
    expect(screen.getByLabelText(/^Key/)).toHaveValue("card-no-data");
    expect(screen.getByLabelText(/^Key/)).not.toHaveAttribute("aria-invalid");
    await user.clear(screen.getByLabelText(/^Key/));
    await user.type(screen.getByLabelText(/^Key/), "a_b");
    expect(screen.getByLabelText(/^Key/)).toHaveAttribute("aria-invalid", "true");
    // The error names what is wrong; the hint, read after it, says what is allowed. Neither repeats the other.
    expect(screen.getByLabelText(/^Key/)).toHaveAccessibleDescription(
      "Not allowed in a key: _. Fixed once created. Follows the name until you edit it. Lowercase letters, digits, - and :.",
    );
    expect(screen.getByRole("button", { name: "Create classification" })).toHaveAttribute("aria-disabled", "true");
    expect(screen.getByText("-", { selector: "code" })).toBeInTheDocument();
    expect(screen.getByText(":", { selector: "code" })).toBeInTheDocument();
    expect(screen.queryByText("_", { selector: "code" })).not.toBeInTheDocument();
    await user.type(screen.getByLabelText(/^Key/), " C");
    expect(screen.getByText(/^Not allowed in a key/)).toHaveTextContent("Not allowed in a key: _, space, C.");
  });

  it("marks Name and Key required, and says why Create waits while they are empty", async () => {
    const user = userEvent.setup();
    const served = serve({ "/tags": ok([]) });
    openArea(INDEX, STEWARD);
    await user.click(await screen.findByRole("button", { name: "New classification" }, T));
    const create = screen.getByRole("button", { name: "Create classification" });
    expect(create).toHaveAttribute("aria-disabled", "true");
    expect(create).toHaveAccessibleDescription("Enter a name and a key.");
    await user.click(create);
    expect(served.asked).not.toContain("POST /tags");
    await user.type(screen.getByLabelText(/^Name/), "Secret");
    expect(create).not.toHaveAttribute("aria-disabled");
  });

  it("warns about a duplicate name before submit, case-insensitively, with a link to it", async () => {
    const user = userEvent.setup();
    const served = serve({ "/tags": ok([tag({ id: "t-pub", key: "public", name: "Public" })]) });
    openArea(INDEX, STEWARD);
    await user.click(await screen.findByRole("button", { name: "New classification" }, T));
    await user.type(screen.getByLabelText(/^Name/), " public ");
    expect(screen.getByText(/A classification named/)).toHaveTextContent("A classification named Public already exists.");
    expect(screen.getByRole("link", { name: "Public" })).toHaveAttribute("href", "/governance/classifications/t-pub");
    expect(screen.getByLabelText(/^Name/)).toHaveAttribute("aria-invalid", "true");
    expect(screen.getByLabelText(/^Name/)).toHaveAccessibleDescription(/A classification named Public already exists\./);
    const create = screen.getByRole("button", { name: "Create classification" });
    expect(create).toHaveAttribute("aria-disabled", "true");
    await user.click(create);
    expect(served.asked).not.toContain("POST /tags");
  });

  it("puts each server code beside its own field", async () => {
    const user = userEvent.setup();
    serve({
      "/tags": ok([]),
      "POST /tags": {
        status: 409,
        body: { error: 'A classification with the key "pii" already exists (PII).', code: "tag_key_taken", key: "pii", existing: { id: "t1", name: "PII" } },
      },
    });
    openArea(INDEX, STEWARD);
    await user.click(await screen.findByRole("button", { name: "New classification" }, T));
    await user.type(screen.getByLabelText(/^Name/), "pii");
    await user.click(screen.getByRole("button", { name: "Create classification" }));
    const message = await screen.findByText(/is taken by/, {}, T);
    expect(message).toHaveTextContent("Key pii is taken by PII.");
    expect(within(message).getByRole("link", { name: "PII" })).toHaveAttribute("href", "/governance/classifications/t1");
    expect(screen.getByLabelText(/^Key/)).toHaveAttribute("aria-invalid", "true");
    expect(screen.getByLabelText(/^Key/)).toHaveAccessibleDescription(/Key pii is taken by PII\./);
    expect(screen.getByLabelText(/^Name/)).toHaveValue("pii");
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("a tag_name_taken refusal (a name the list did not have) sits under Name with a link", async () => {
    const user = userEvent.setup();
    serve({
      "/tags": ok([]),
      "POST /tags": {
        status: 409,
        body: {
          error: 'A classification named "Public" already exists (key public).',
          code: "tag_name_taken",
          name: "Public",
          existing: { id: "t-pub", key: "public", name: "Public" },
        },
      },
    });
    openArea(INDEX, STEWARD);
    await user.click(await screen.findByRole("button", { name: "New classification" }, T));
    await user.type(screen.getByLabelText(/^Name/), "public");
    await user.click(screen.getByRole("button", { name: "Create classification" }));
    expect(await screen.findByRole("link", { name: "Public" }, T)).toHaveAttribute("href", "/governance/classifications/t-pub");
    expect(screen.getByLabelText(/^Name/)).toHaveAttribute("aria-invalid", "true");
    expect(screen.getByLabelText(/^Name/)).toHaveAccessibleDescription(/A classification named Public already exists\./);
  });

  it("a tag_key_invalid refusal sits under Key in the server's words", async () => {
    const user = userEvent.setup();
    serve({
      "/tags": ok([]),
      "POST /tags": {
        status: 400,
        body: { error: "A classification key may hold lowercase letters, digits, ':' and '-' only: \"x\"", code: "tag_key_invalid", key: "x", allowed: "a-z 0-9 : -" },
      },
    });
    openArea(INDEX, STEWARD);
    await user.click(await screen.findByRole("button", { name: "New classification" }, T));
    await user.type(screen.getByLabelText(/^Name/), "x");
    await user.click(screen.getByRole("button", { name: "Create classification" }));
    await waitFor(() => expect(screen.getByLabelText(/^Key/)).toHaveAttribute("aria-invalid", "true"), T);
    expect(screen.getByLabelText(/^Key/)).toHaveAccessibleDescription(/may hold lowercase letters, digits, ':' and '-' only/);
  });

  it("labels the color and states its value as text", async () => {
    const user = userEvent.setup();
    serve({ "/tags": ok([]) });
    openArea(INDEX, STEWARD);
    await user.click(await screen.findByRole("button", { name: "New classification" }, T));
    const color = screen.getByLabelText("Color");
    expect(color).toHaveAttribute("type", "color");
    expect(color).toHaveAccessibleDescription(/Color of the swatch shown next to the name\. #6366f1/);
  });
});
