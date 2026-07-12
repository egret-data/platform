import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { FacetSelect } from "@/components/explore/FacetSelect";

it("renders the All option plus provided options and calls onChange on select", async () => {
  const user = userEvent.setup();
  const onChange = vi.fn();
  render(
    <FacetSelect
      label="Status"
      value=""
      onChange={onChange}
      options={[
        { value: "active", label: "Active" },
        { value: "disabled", label: "Disabled" },
      ]}
    />,
  );
  await user.click(screen.getByLabelText("Status"));
  expect(await screen.findByRole("option", { name: "All" })).toBeInTheDocument();
  await user.click(await screen.findByRole("option", { name: "Active" }));
  expect(onChange).toHaveBeenCalledWith("active");
});
