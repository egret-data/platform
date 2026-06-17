import { render, screen } from "@testing-library/react";
import { Button } from "@/components/ui/button";

test("renders a button with its label", () => {
  render(<Button>Click me</Button>);
  expect(screen.getByRole("button", { name: "Click me" })).toBeInTheDocument();
});
