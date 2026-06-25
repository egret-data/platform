import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, test } from "vitest";
import { RowActionsMenu } from "@/components/explore/RowActionsMenu";
import { DropdownMenuItem } from "@/components/ui/dropdown-menu";

test("opens to reveal its items", async () => {
  const user = userEvent.setup();
  render(<RowActionsMenu><DropdownMenuItem>Delete</DropdownMenuItem></RowActionsMenu>);
  await user.click(screen.getByRole("button", { name: "Actions" }));
  expect(await screen.findByText("Delete")).toBeTruthy();
});
