import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { toast } from "sonner";
import { ADMIN, fail, ok, openArea, serve, STEWARD, tag, TAG_MANAGER, TAG_READER, where } from "./harness";

vi.mock("sonner", () => ({ toast: { success: vi.fn(), error: vi.fn(), warning: vi.fn() } }));

beforeEach(() => vi.clearAllMocks());
afterEach(() => vi.unstubAllGlobals());

const T = { timeout: 3000 };
const PII = tag({
  id: "t-pii",
  key: "pii",
  name: "PII",
  description: "Personally identifiable information",
  color: "#e11d48",
});
const GATED = tag({ id: "t-sec", key: "sec", name: "Secret", required_roles: ["cleared"] });
const OTHER = tag({ id: "t-oth", key: "other", name: "Other" });
const URL_PII = "/governance/classifications/t-pii";

const header = () => screen.getByRole("region", { name: "Classification" });
const tabNames = () => screen.getAllByRole("tab").map((t) => t.textContent);
const beforeUnloadPrevented = () => {
  const e = new Event("beforeunload", { cancelable: true });
  window.dispatchEvent(e);
  return e.defaultPrevented;
};

describe("a classification's page: the header", () => {
  it("shows the swatch, name, key and description; no clearance badge when ungated", async () => {
    serve({ "/tags": ok([OTHER, PII]) });
    openArea(URL_PII, TAG_READER);
    expect(await screen.findByRole("heading", { level: 1, name: "PII" }, T)).toBeInTheDocument();
    expect(within(header()).getByText("pii")).toHaveClass("font-mono");
    expect(within(header()).getByText("Personally identifiable information")).toBeInTheDocument();
    expect(header().querySelector('[aria-hidden="true"][style]')).toHaveStyle({ backgroundColor: "#e11d48" });
    expect(within(header()).queryByText("Clearance")).not.toBeInTheDocument();
  });

  it("marks a gated classification with the word Clearance and a lock", async () => {
    serve({ "/tags": ok([GATED]) });
    openArea("/governance/classifications/t-sec", TAG_READER);
    await screen.findByRole("heading", { level: 1, name: "Secret" }, T);
    const badge = within(header()).getByText("Clearance");
    expect(badge.closest("div,span")?.querySelector("svg")).not.toBeNull();
  });

  it("offers Edit only with tag:manage or platform admin", async () => {
    serve({ "/tags": ok([PII]) });
    const { unmount } = openArea(URL_PII, TAG_READER);
    await screen.findByRole("heading", { level: 1, name: "PII" }, T);
    expect(screen.queryByRole("button", { name: "Edit" })).not.toBeInTheDocument();
    unmount();
    for (const as of [TAG_MANAGER, ADMIN]) {
      const r = openArea(URL_PII, as);
      expect(await screen.findByRole("button", { name: "Edit" }, T)).toBeInTheDocument();
      r.unmount();
    }
  });

  it("saves only the changed fields, refreshes the list, toasts and stays", async () => {
    const user = userEvent.setup();
    const served = serve({ "/tags": ok([PII]), "PATCH /tags/t-pii": ok({ ...PII, description: "Changed" }) });
    openArea(URL_PII, STEWARD);
    await user.click(await screen.findByRole("button", { name: "Edit" }, T));
    const description = screen.getByLabelText("Description");
    await user.clear(description);
    await user.type(description, "Changed");
    // Opening the form reads the list again (the rename check uses it fresh); saving refreshes it once more.
    await waitFor(() => expect(served.asked.filter((a) => a === "GET /tags")).toHaveLength(2), T);
    await user.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(served.bodies["PATCH /tags/t-pii"]).toEqual([{ description: "Changed" }]), T);
    await waitFor(() => expect(served.asked.filter((a) => a === "GET /tags")).toHaveLength(3), T);
    expect(toast.success).toHaveBeenCalledWith("Classification saved.");
    expect(where()).toBe(URL_PII);
    await waitFor(() => expect(screen.queryByLabelText("Description")).not.toBeInTheDocument());
  });

  it("Cancel restores the saved values and sends nothing", async () => {
    const user = userEvent.setup();
    const served = serve({ "/tags": ok([PII]) });
    openArea(URL_PII, STEWARD);
    await user.click(await screen.findByRole("button", { name: "Edit" }, T));
    await user.clear(screen.getByLabelText(/^Name/));
    await user.type(screen.getByLabelText(/^Name/), "Renamed");
    await user.click(screen.getByRole("button", { name: "Cancel" }));
    expect(screen.getByRole("heading", { level: 1, name: "PII" })).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Edit" }));
    expect(screen.getByLabelText(/^Name/)).toHaveValue("PII");
    expect(served.asked.some((a) => a.startsWith("PATCH"))).toBe(false);
  });

  it("closing the form returns focus to Edit, after Cancel and after Save (live check D5)", async () => {
    const user = userEvent.setup();
    serve({ "/tags": ok([PII]), "PATCH /tags/t-pii": ok({ ...PII, description: "Changed" }) });
    openArea(URL_PII, STEWARD);
    await user.click(await screen.findByRole("button", { name: "Edit" }, T));
    await user.click(screen.getByRole("button", { name: "Cancel" }));
    expect(screen.getByRole("button", { name: "Edit" })).toHaveFocus();
    await user.click(screen.getByRole("button", { name: "Edit" }));
    await user.type(screen.getByLabelText("Description"), "Changed");
    await user.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(screen.getByRole("button", { name: "Edit" })).toHaveFocus(), T);
  });

  it("a refused save keeps the form and every edit, with the server's message", async () => {
    const user = userEvent.setup();
    serve({ "/tags": ok([PII]), "PATCH /tags/t-pii": fail(400, "name must not be empty") });
    openArea(URL_PII, STEWARD);
    await user.click(await screen.findByRole("button", { name: "Edit" }, T));
    await user.clear(screen.getByLabelText("Description"));
    await user.type(screen.getByLabelText("Description"), "Kept");
    await user.click(screen.getByRole("button", { name: "Save" }));
    expect(await screen.findByText("name must not be empty", {}, T)).toBeInTheDocument();
    expect(screen.getByLabelText("Description")).toHaveValue("Kept");
    expect(toast.success).not.toHaveBeenCalled();
  });

  it("warns before leaving with unsaved edits, and only then", async () => {
    const user = userEvent.setup();
    serve({ "/tags": ok([PII]) });
    openArea(URL_PII, STEWARD);
    await user.click(await screen.findByRole("button", { name: "Edit" }, T));
    expect(beforeUnloadPrevented()).toBe(false);
    await user.type(screen.getByLabelText(/^Name/), "!");
    expect(beforeUnloadPrevented()).toBe(true);
    await user.click(screen.getByRole("button", { name: "Cancel" }));
    expect(beforeUnloadPrevented()).toBe(false);
  });
});

describe("a classification's page: rename checks (C10)", () => {
  const PUBLIC = tag({ id: "t-pub", key: "public", name: "Public" });

  it("warns about a rename onto another classification's name, case-insensitively, with a link", async () => {
    const user = userEvent.setup();
    const served = serve({ "/tags": ok([PII, PUBLIC]) });
    openArea(URL_PII, STEWARD);
    await user.click(await screen.findByRole("button", { name: "Edit" }, T));
    await user.clear(screen.getByLabelText(/^Name/));
    await user.type(screen.getByLabelText(/^Name/), "PUBLIC");
    expect(screen.getByLabelText(/^Name/)).toHaveAttribute("aria-invalid", "true");
    expect(screen.getByLabelText(/^Name/)).toHaveAccessibleDescription("A classification named Public already exists.");
    expect(screen.getByRole("link", { name: "Public" })).toHaveAttribute("href", "/governance/classifications/t-pub");
    const save = screen.getByRole("button", { name: "Save" });
    expect(save).toHaveAttribute("aria-disabled", "true");
    await user.click(save);
    expect(served.asked.some((a) => a.startsWith("PATCH"))).toBe(false);
  });

  it("marks Name required, and an empty name makes Save wait with the reason tied", async () => {
    const user = userEvent.setup();
    const served = serve({ "/tags": ok([PII]) });
    openArea(URL_PII, STEWARD);
    await user.click(await screen.findByRole("button", { name: "Edit" }, T));
    expect(screen.getByText("(required)")).toBeInTheDocument();
    await user.clear(screen.getByLabelText(/^Name/));
    const save = screen.getByRole("button", { name: "Save" });
    expect(save).toHaveAttribute("aria-disabled", "true");
    expect(save).toHaveAccessibleDescription("Enter a name.");
    await user.click(save);
    expect(served.asked.some((a) => a.startsWith("PATCH"))).toBe(false);
  });

  it("changing only the case of its own name is not a duplicate", async () => {
    const user = userEvent.setup();
    serve({ "/tags": ok([PII, PUBLIC]) });
    openArea(URL_PII, STEWARD);
    await user.click(await screen.findByRole("button", { name: "Edit" }, T));
    await user.clear(screen.getByLabelText(/^Name/));
    await user.type(screen.getByLabelText(/^Name/), "pii");
    expect(screen.getByLabelText(/^Name/)).not.toHaveAttribute("aria-invalid");
    expect(screen.getByRole("button", { name: "Save" })).not.toHaveAttribute("aria-disabled");
  });

  it("a duplicate pair that already exists does not block editing other fields", async () => {
    const user = userEvent.setup();
    const twin = tag({ id: "t-pii2", key: "pii2", name: "pii" });
    const served = serve({ "/tags": ok([PII, twin]), "PATCH /tags/t-pii": ok({ ...PII, description: "x" }) });
    openArea(URL_PII, STEWARD);
    await user.click(await screen.findByRole("button", { name: "Edit" }, T));
    expect(screen.getByLabelText(/^Name/)).not.toHaveAttribute("aria-invalid");
    await user.clear(screen.getByLabelText("Description"));
    await user.type(screen.getByLabelText("Description"), "x");
    await user.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(served.bodies["PATCH /tags/t-pii"]).toEqual([{ description: "x" }]), T);
  });

  it("a tag_name_taken refusal sits under Name with a link, every edit kept", async () => {
    const user = userEvent.setup();
    serve({
      "/tags": ok([PII]),
      "PATCH /tags/t-pii": {
        status: 409,
        body: {
          error: 'A classification named "Public" already exists (key public).',
          code: "tag_name_taken",
          name: "Public",
          existing: { id: "t-pub", key: "public", name: "Public" },
        },
      },
    });
    openArea(URL_PII, STEWARD);
    await user.click(await screen.findByRole("button", { name: "Edit" }, T));
    await user.clear(screen.getByLabelText(/^Name/));
    await user.type(screen.getByLabelText(/^Name/), "Public");
    await user.click(screen.getByRole("button", { name: "Save" }));
    expect(await screen.findByRole("link", { name: "Public" }, T)).toHaveAttribute("href", "/governance/classifications/t-pub");
    expect(screen.getByLabelText(/^Name/)).toHaveAttribute("aria-invalid", "true");
    expect(screen.getByLabelText(/^Name/)).toHaveValue("Public");
    expect(within(screen.getByRole("form", { name: "Edit classification" })).queryByRole("alert")).not.toBeInTheDocument();
    expect(toast.success).not.toHaveBeenCalled();
  });

  it("a rename refused for a mask on a whole table says so under Name", async () => {
    const user = userEvent.setup();
    serve({
      "/tags": ok([PII]),
      "PATCH /tags/t-pii": {
        status: 400,
        body: {
          error: "raw",
          code: "table_tag_mask_conflict",
          tag: "Card",
          tagId: "t-pii",
          warehouseId: null,
          schema: "sales",
          table: "orders",
          mask: "Hash card",
          maskOrigin: "rule",
          moreTables: false,
        },
      },
    });
    openArea(URL_PII, STEWARD);
    await user.click(await screen.findByRole("button", { name: "Edit" }, T));
    await user.clear(screen.getByLabelText(/^Name/));
    await user.type(screen.getByLabelText(/^Name/), "Card");
    await user.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(screen.getByLabelText(/^Name/)).toHaveAttribute("aria-invalid", "true"), T);
    expect(screen.getByLabelText(/^Name/)).toHaveAccessibleDescription(
      "Card is masked by Hash card while it sits on the whole table sales.orders; attach it to columns first.",
    );
  });

  it("labels the color and states its value as text", async () => {
    const user = userEvent.setup();
    serve({ "/tags": ok([PII]) });
    openArea(URL_PII, STEWARD);
    await user.click(await screen.findByRole("button", { name: "Edit" }, T));
    const color = screen.getByLabelText("Color");
    expect(color).toHaveAttribute("type", "color");
    expect(color).toHaveAccessibleDescription("Color of the swatch shown next to the name. #e11d48");
  });
});

describe("a classification's page: not found, error, loading", () => {
  it("says an unknown id names no classification, with the way back", async () => {
    serve({ "/tags": ok([PII]) });
    openArea("/governance/classifications/nope", TAG_READER);
    expect(await screen.findByText("No classification with this id.", {}, T)).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Classifications" })).toHaveAttribute("href", "/governance/classifications");
    expect(screen.queryByRole("tab")).not.toBeInTheDocument();
  });

  it("shows a failing tag list as an error with Retry, never as not found", async () => {
    const user = userEvent.setup();
    const served = serve({ "/tags": fail(500) });
    openArea(URL_PII, TAG_READER);
    expect(
      await screen.findByText("The classification could not be loaded. Retry, or reload the page.", {}, T),
    ).toBeInTheDocument();
    expect(screen.queryByText("No classification with this id.")).not.toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Retry" }));
    await waitFor(() => expect(served.asked.filter((a) => a === "GET /tags")).toHaveLength(2));
  });

  it("shows a skeleton while the tag list loads", async () => {
    serve({ "/tags": "never" });
    openArea(URL_PII, TAG_READER);
    expect(await screen.findByTestId("list-skeleton", {}, T)).toBeInTheDocument();
  });
});

describe("a classification's page: the tabs", () => {
  it("names the tabs with the glossary, in order, Where used first", async () => {
    serve({ "/tags": ok([PII]) });
    openArea(URL_PII, TAG_READER);
    await screen.findByRole("heading", { level: 1, name: "PII" }, T);
    expect(tabNames()).toEqual(["Where used", "Clearance", "Rules", "Views", "Manual policies"]);
    for (const retired of ["Mask & filter", "View exposure", "Hand-written"]) {
      expect(screen.queryByRole("tab", { name: retired })).not.toBeInTheDocument();
    }
    expect(screen.getByRole("tab", { name: "Where used" })).toHaveAttribute("aria-selected", "true");
  });

  it.each([
    ["clearance", "Clearance"],
    ["rules", "Rules"],
    ["views", "Views"],
    ["handwritten", "Manual policies"],
  ])("?tab=%s selects %s", async (param, name) => {
    serve({ "/tags": ok([PII]) });
    openArea(`${URL_PII}?tab=${param}`, TAG_READER);
    expect(await screen.findByRole("tab", { name }, T)).toHaveAttribute("aria-selected", "true");
  });

  it("falls back to Where used for an unknown tab", async () => {
    serve({ "/tags": ok([PII]) });
    openArea(`${URL_PII}?tab=bogus`, TAG_READER);
    expect(await screen.findByRole("tab", { name: "Where used" }, T)).toHaveAttribute("aria-selected", "true");
  });

  it("writes the selected tab into ?tab=, and the first tab as no parameter", async () => {
    const user = userEvent.setup();
    serve({ "/tags": ok([PII]) });
    openArea(URL_PII, TAG_READER);
    await user.click(await screen.findByRole("tab", { name: "Clearance" }, T));
    expect(where()).toBe(`${URL_PII}?tab=clearance`);
    await user.click(screen.getByRole("tab", { name: "Where used" }));
    expect(where()).toBe(URL_PII);
  });
});
