import { render, screen } from "@testing-library/react";
import type { ReactElement } from "react";
import { TooltipProvider } from "@/components/ui/tooltip";
import { PropertyField } from "./PropertyField";

// FieldLabel's help tooltip relies on the app-wide TooltipProvider (AppShell).
const renderWithProviders = (ui: ReactElement) =>
  render(<TooltipProvider>{ui}</TooltipProvider>);

test("enum renders a combobox with options", () => {
  renderWithProviders(
    <PropertyField
      prop={{
        key: "x",
        section: "s",
        label: "X",
        type: "enum",
        enumOptions: ["A", "B"],
        help: "h",
      }}
      value="A"
      onChange={() => {}}
    />,
  );
  expect(screen.getByRole("combobox")).toBeTruthy();
});

test("boolean renders a switch", () => {
  renderWithProviders(
    <PropertyField
      prop={{ key: "y", section: "s", label: "Y", type: "boolean", help: "h" }}
      value="true"
      onChange={() => {}}
    />,
  );
  expect(screen.getByRole("switch")).toBeTruthy();
});
