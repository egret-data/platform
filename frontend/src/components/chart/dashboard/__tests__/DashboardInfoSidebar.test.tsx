import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { DashboardInfoSidebar } from "../DashboardInfoSidebar";

const wrap = (ui: React.ReactElement) => render(<MemoryRouter>{ui}</MemoryRouter>);

describe("DashboardInfoSidebar", () => {
  const base = {
    name: "Sales",
    description: "old",
    ownerLabel: "Ref User",
    createdAt: "2026-08-01T12:00:00Z",
    editorLabel: "Other User",
    updatedAt: "2026-08-05T12:00:00Z",
    folderName: "Ops",
    folderHref: "/explore/demo/dashboards?folder=f1",
    revisions: [],
    revisionsLoading: false,
    onRevert: () => {},
    reverting: false,
    onOpenHistory: () => {},
    certified: false,
    certifiedBy: "",
    certificationDetails: "",
    canCertify: false,
    certifying: false,
    onSetCertification: () => {},
  };
  it("shows both attribution lines by name and renders the description", () => {
    wrap(<DashboardInfoSidebar {...base} onSaveDescription={vi.fn()} onClose={vi.fn()} />);
    expect(screen.getByText(/^August [12], 2026 by Ref User$/)).toBeTruthy();
    expect(screen.getByText(/^August [56], 2026 by Other User$/)).toBeTruthy();
    expect((screen.getByLabelText("description") as HTMLTextAreaElement).value).toBe("old");
  });
  it("falls back to the creator when nothing has edited the dashboard since 0106", () => {
    wrap(<DashboardInfoSidebar {...base} editorLabel={null} onSaveDescription={vi.fn()} onClose={vi.fn()} />);
    expect(screen.getByText(/^August [56], 2026 by Ref User$/)).toBeTruthy();
  });
  it("omits the Added row for a dashboard with no recorded creation date", () => {
    wrap(<DashboardInfoSidebar {...base} createdAt={null} onSaveDescription={vi.fn()} onClose={vi.fn()} />);
    expect(screen.queryByText("Added")).toBeNull();
  });
  it("links Saved in at the folder's section on the list", () => {
    wrap(<DashboardInfoSidebar {...base} onSaveDescription={vi.fn()} onClose={vi.fn()} />);
    const link = screen.getByRole("link", { name: "Ops" });
    expect(link.getAttribute("href")).toBe("/explore/demo/dashboards?folder=f1");
  });
  it("says Uncategorized, with no link, for a dashboard in no folder", () => {
    wrap(<DashboardInfoSidebar {...base} folderName={null} folderHref={null} onSaveDescription={vi.fn()} onClose={vi.fn()} />);
    expect(screen.getByText("Uncategorized")).toBeTruthy();
    expect(screen.queryByRole("link")).toBeNull();
  });
  it("saves an edited description on blur", () => {
    const onSave = vi.fn();
    wrap(<DashboardInfoSidebar {...base} onSaveDescription={onSave} onClose={vi.fn()} />);
    const ta = screen.getByLabelText("description");
    fireEvent.change(ta, { target: { value: "new desc" } });
    fireEvent.blur(ta);
    expect(onSave).toHaveBeenCalledWith("new desc");
  });
  it("does not save when the description is unchanged", () => {
    const onSave = vi.fn();
    wrap(<DashboardInfoSidebar {...base} onSaveDescription={onSave} onClose={vi.fn()} />);
    fireEvent.blur(screen.getByLabelText("description"));
    expect(onSave).not.toHaveBeenCalled();
  });
});

describe("certification", () => {
  const base = {
    name: "Sales", description: "", ownerLabel: "Ref User",
    createdAt: "2026-08-01T12:00:00Z", editorLabel: "Ref User",
    updatedAt: "2026-08-05T12:00:00Z", folderName: null, folderHref: null,
    revisions: [], revisionsLoading: false, onRevert: () => {}, reverting: false,
    onOpenHistory: () => {}, onSaveDescription: () => {}, onClose: () => {},
    certified: false, certifiedBy: "", certificationDetails: "",
    canCertify: false, certifying: false, onSetCertification: () => {},
  };

  it("WITHHOLDS the control from someone without dashboard:certify", () => {
    // Withheld, not disabled. The whole point of the platform-scoped perm is that a workspace
    // editor cannot certify their own work; a greyed-out button would advertise the capability
    // and invite the question. This is the assertion that pins the permission split in the UI.
    wrap(<DashboardInfoSidebar {...base} canCertify={false} />);
    expect(screen.queryByRole("button", { name: /certify/i })).toBeNull();
  });

  it("offers it to a steward", () => {
    wrap(<DashboardInfoSidebar {...base} canCertify />);
    expect(screen.getByRole("button", { name: "Certify this dashboard" })).toBeTruthy();
  });

  it("passes the note along when certifying", () => {
    const onSet = vi.fn();
    wrap(<DashboardInfoSidebar {...base} canCertify onSetCertification={onSet} />);
    fireEvent.change(screen.getByLabelText("certification note"), { target: { value: "reviewed by finance" } });
    fireEvent.click(screen.getByRole("button", { name: "Certify this dashboard" }));
    expect(onSet).toHaveBeenCalledWith(true, "reviewed by finance");
  });

  it("shows who certified it, and offers withdrawal rather than a second certify", () => {
    wrap(<DashboardInfoSidebar {...base} certified certifiedBy="ana@egret.local"
      certificationDetails="reviewed by finance" canCertify />);
    expect(screen.getByText("ana@egret.local")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Withdraw certification" })).toBeTruthy();
    // The note input is gone once certified — there is nothing to justify about withdrawing.
    expect(screen.queryByLabelText("certification note")).toBeNull();
  });

  it("says nothing about certification when the dashboard is not certified", () => {
    wrap(<DashboardInfoSidebar {...base} />);
    expect(screen.queryByText(/^Certified$/)).toBeNull();
  });
});
