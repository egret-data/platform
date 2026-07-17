import { fireEvent, render, screen } from "@testing-library/react";
import { vi } from "vitest";

import type { MiniContainer } from "@/api/controlPlane";
import { ContainerListEditor } from "./ContainersMountsSection";
import { TolerationsEditor } from "./PlacementSection";
import type { TolerationForm } from "../useEngineConfigForm";

// These two editors are extracted from the Trino config sections so the
// StarRocks Settings tab can reuse them. The tests pin their standalone
// prop surface (rows / onChange) and the add-row callback.

test("TolerationsEditor renders a row and adds one on click", () => {
  const onChange = vi.fn();
  const rows: TolerationForm[] = [
    { key: "dedicated", operator: "Exists", value: "", effect: "", tolerationSeconds: "" },
  ];
  render(<TolerationsEditor rows={rows} onChange={onChange} />);

  // The existing row's key is shown.
  expect(screen.getByDisplayValue("dedicated")).toBeTruthy();

  fireEvent.click(screen.getByRole("button", { name: /add toleration/i }));
  expect(onChange).toHaveBeenCalledTimes(1);
  expect(onChange.mock.calls[0][0]).toHaveLength(2);
});

test("ContainerListEditor renders a container and adds one on click", () => {
  const onChange = vi.fn();
  const rows: MiniContainer[] = [{ name: "wait", image: "busybox" }];
  render(<ContainerListEditor rows={rows} onChange={onChange} />);

  // The existing container's image is shown.
  expect(screen.getByDisplayValue("busybox")).toBeTruthy();

  fireEvent.click(screen.getByRole("button", { name: /add container/i }));
  expect(onChange).toHaveBeenCalledTimes(1);
  expect(onChange.mock.calls[0][0]).toHaveLength(2);
});
