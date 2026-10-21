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
    await user.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(served.bodies["PATCH /tags/t-pii"]).toEqual([{ description: "Changed" }]), T);
    await waitFor(() => expect(served.asked.filter((a) => a === "GET /tags")).toHaveLength(2), T);
    expect(toast.success).toHaveBeenCalledWith("Classification saved.");
    expect(where()).toBe(URL_PII);
    await waitFor(() => expect(screen.queryByLabelText("Description")).not.toBeInTheDocument());
  });

  it("Cancel restores the saved values and sends nothing", async () => {
    const user = userEvent.setup();
    const served = serve({ "/tags": ok([PII]) });
    openArea(URL_PII, STEWARD);
    await user.click(await screen.findByRole("button", { name: "Edit" }, T));
    await user.clear(screen.getByLabelText("Name"));
    await user.type(screen.getByLabelText("Name"), "Renamed");
    await user.click(screen.getByRole("button", { name: "Cancel" }));
    expect(screen.getByRole("heading", { level: 1, name: "PII" })).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Edit" }));
    expect(screen.getByLabelText("Name")).toHaveValue("PII");
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
    await user.type(screen.getByLabelText("Name"), "!");
    expect(beforeUnloadPrevented()).toBe(true);
    await user.click(screen.getByRole("button", { name: "Cancel" }));
    expect(beforeUnloadPrevented()).toBe(false);
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
