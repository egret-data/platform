import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, test, vi } from "vitest";

import { WorkspaceFeaturesTab } from "@/components/control-plane/WorkspaceFeaturesTab";
import * as featuresApi from "@/api/features";
import type { FeatureDto } from "@/api/generated/model/featureDto";

vi.mock("@/api/features");

const WORKSPACE_ID = "11111111-1111-1111-1111-111111111111";

const FEATURES: FeatureDto[] = [
  { key: "sql_lab", label: "SQL Lab", description: "Ad-hoc SQL queries.", enabled: true },
  { key: "datasets", label: "Datasets", description: "Curated, reusable datasets.", enabled: true },
  { key: "charts", label: "Charts", description: "Chart building.", enabled: true },
  { key: "dashboards", label: "Dashboards", description: "Dashboards of charts.", enabled: false },
  { key: "notebooks", label: "Notebooks", description: "Python/SQL notebooks.", enabled: true },
  {
    key: "data_api",
    label: "Data API",
    description: "Publish saved queries as HTTP endpoints.",
    enabled: true,
  },
];

function stubList(features: FeatureDto[] | undefined, isLoading = false) {
  return { features, isLoading, isError: false, error: null } as unknown as ReturnType<
    typeof featuresApi.useWorkspaceFeaturesList
  >;
}

const mutate = vi.fn();

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(featuresApi.useWorkspaceFeaturesList).mockReturnValue(stubList(FEATURES));
  vi.mocked(featuresApi.useSetWorkspaceFeatures).mockReturnValue({
    mutate,
    isPending: false,
  } as unknown as ReturnType<typeof featuresApi.useSetWorkspaceFeatures>);
});

describe("WorkspaceFeaturesTab", () => {
  test("renders a switch per module reflecting its current enabled state", () => {
    render(<WorkspaceFeaturesTab workspaceId={WORKSPACE_ID} />);
    expect(screen.getByRole("switch", { name: "SQL Lab" })).toHaveAttribute("aria-checked", "true");
    expect(screen.getByRole("switch", { name: "Datasets" })).toHaveAttribute("aria-checked", "true");
    expect(screen.getByRole("switch", { name: "Charts" })).toHaveAttribute("aria-checked", "true");
    expect(screen.getByRole("switch", { name: "Dashboards" })).toHaveAttribute("aria-checked", "false");
    expect(screen.getByRole("switch", { name: "Notebooks" })).toHaveAttribute("aria-checked", "true");
  });

  test("Save is disabled until a switch is toggled", async () => {
    const user = userEvent.setup();
    render(<WorkspaceFeaturesTab workspaceId={WORKSPACE_ID} />);

    expect(screen.getByRole("button", { name: "Save" })).toBeDisabled();
    await user.click(screen.getByRole("switch", { name: "Dashboards" }));
    expect(screen.getByRole("button", { name: "Save" })).toBeEnabled();
  });

  test("Save posts the full feature map, not just the toggled key", async () => {
    const user = userEvent.setup();
    render(<WorkspaceFeaturesTab workspaceId={WORKSPACE_ID} />);

    await user.click(screen.getByRole("switch", { name: "Dashboards" }));
    await user.click(screen.getByRole("switch", { name: "Notebooks" }));
    await user.click(screen.getByRole("button", { name: "Save" }));

    expect(mutate).toHaveBeenCalledWith(
      {
        id: WORKSPACE_ID,
        data: {
          features: {
            sql_lab: true,
            datasets: true,
            charts: true,
            dashboards: true,
            notebooks: false,
            data_api: true,
          },
        },
      },
      expect.anything(),
    );
  });

  test("toggling Charts off forces Dashboards off and disables its switch", async () => {
    // Start from a consistent on/on state so turning Charts off is the thing
    // under test, not the starting fixture's dashboards:false.
    vi.mocked(featuresApi.useWorkspaceFeaturesList).mockReturnValue(
      stubList(FEATURES.map((f) => (f.key === "dashboards" ? { ...f, enabled: true } : f))),
    );
    const user = userEvent.setup();
    render(<WorkspaceFeaturesTab workspaceId={WORKSPACE_ID} />);

    expect(screen.getByRole("switch", { name: "Dashboards" })).toHaveAttribute(
      "aria-checked",
      "true",
    );
    expect(screen.getByRole("switch", { name: "Dashboards" })).toBeEnabled();

    await user.click(screen.getByRole("switch", { name: "Charts" }));

    const dashboardsSwitch = screen.getByRole("switch", { name: "Dashboards" });
    expect(dashboardsSwitch).toHaveAttribute("aria-checked", "false");
    expect(dashboardsSwitch).toBeDisabled();
    // Requirement is surfaced inline, not just a disabled control with no
    // explanation. (Rendered twice — narrow-screen and Details-column
    // copies of the same help text — so assert at least one is present.)
    expect(screen.getAllByText(/requires charts/i).length).toBeGreaterThan(0);
  });

  test("Dashboards cannot be switched on while Charts is off, and turning Charts back on re-enables it", async () => {
    vi.mocked(featuresApi.useWorkspaceFeaturesList).mockReturnValue(
      stubList(FEATURES.map((f) => (f.key === "charts" ? { ...f, enabled: false } : f))),
    );
    const user = userEvent.setup();
    render(<WorkspaceFeaturesTab workspaceId={WORKSPACE_ID} />);

    const dashboardsSwitch = screen.getByRole("switch", { name: "Dashboards" });
    expect(dashboardsSwitch).toBeDisabled();

    // A click on a disabled switch is a no-op: it stays off.
    await user.click(dashboardsSwitch);
    expect(screen.getByRole("switch", { name: "Dashboards" })).toHaveAttribute(
      "aria-checked",
      "false",
    );

    await user.click(screen.getByRole("switch", { name: "Charts" }));
    expect(screen.getByRole("switch", { name: "Dashboards" })).toBeEnabled();
  });

  test("toggling Dashboards on also turns Charts on", async () => {
    const user = userEvent.setup();
    render(<WorkspaceFeaturesTab workspaceId={WORKSPACE_ID} />);

    await user.click(screen.getByRole("switch", { name: "Dashboards" }));
    await user.click(screen.getByRole("button", { name: "Save" }));

    expect(mutate).toHaveBeenCalledWith(
      {
        id: WORKSPACE_ID,
        data: {
          features: expect.objectContaining({ charts: true, dashboards: true }),
        },
      },
      expect.anything(),
    );
  });

  test("Save never posts dashboards:true with charts:false", async () => {
    // Start from dashboards:true, charts:true; turning Charts off must carry
    // Dashboards off with it into the posted map, not just on screen.
    vi.mocked(featuresApi.useWorkspaceFeaturesList).mockReturnValue(
      stubList(FEATURES.map((f) => (f.key === "dashboards" ? { ...f, enabled: true } : f))),
    );
    const user = userEvent.setup();
    render(<WorkspaceFeaturesTab workspaceId={WORKSPACE_ID} />);

    await user.click(screen.getByRole("switch", { name: "Charts" }));
    await user.click(screen.getByRole("button", { name: "Save" }));

    const call = mutate.mock.calls[0][0];
    const posted = call.data.features as Record<string, boolean>;
    expect(posted.charts).toBe(false);
    expect(posted.dashboards).toBe(false);
    expect(posted.dashboards && !posted.charts).toBe(false);
  });

  test("explains this is an entitlement affecting everyone, admins included", () => {
    render(<WorkspaceFeaturesTab workspaceId={WORKSPACE_ID} />);
    expect(
      screen.getByText(/hides it .* for everyone|blocks its API|everyone in this workspace/i),
    ).toBeInTheDocument();
  });

  test("warns that disabling Data API does not stop serving", () => {
    render(<WorkspaceFeaturesTab workspaceId={WORKSPACE_ID} />);
    // Rendered twice — narrow-screen and Details-column copies of the same
    // help text — so assert at least one is present.
    expect(screen.getAllByText(/live endpoints keep serving/i).length).toBeGreaterThan(0);
    // It's specific to the Data API row, not sprayed onto every module.
    const notebooksRow = screen.getByRole("switch", { name: "Notebooks" }).closest("tr");
    expect(notebooksRow).not.toHaveTextContent(/live endpoints keep serving/i);
  });

  test("shows a loading state before the features have arrived", () => {
    vi.mocked(featuresApi.useWorkspaceFeaturesList).mockReturnValue(stubList(undefined, true));
    render(<WorkspaceFeaturesTab workspaceId={WORKSPACE_ID} />);
    expect(screen.queryByRole("switch")).not.toBeInTheDocument();
  });
});
