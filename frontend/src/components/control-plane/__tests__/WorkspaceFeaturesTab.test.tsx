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
          },
        },
      },
      expect.anything(),
    );
  });

  test("explains this is an entitlement affecting everyone, admins included", () => {
    render(<WorkspaceFeaturesTab workspaceId={WORKSPACE_ID} />);
    expect(
      screen.getByText(/hides it .* for everyone|blocks its API|everyone in this workspace/i),
    ).toBeInTheDocument();
  });

  test("shows a loading state before the features have arrived", () => {
    vi.mocked(featuresApi.useWorkspaceFeaturesList).mockReturnValue(stubList(undefined, true));
    render(<WorkspaceFeaturesTab workspaceId={WORKSPACE_ID} />);
    expect(screen.queryByRole("switch")).not.toBeInTheDocument();
  });
});
