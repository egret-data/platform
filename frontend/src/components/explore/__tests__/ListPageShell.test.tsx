import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
import { ListPageShell } from "@/components/explore/ListPageShell";

describe("ListPageShell", () => {
  it("renders exactly one h1, and it is the title", () => {
    render(
      <ListPageShell title="Endpoints" toolbar={<button type="button">tb</button>} action={<button type="button">+ Endpoint</button>}>
        <p>body</p>
      </ListPageShell>,
    );
    // Every Explore page must have exactly one document-level heading. In the
    // previous phase, removing a shell silently took a route's only `h1` with it
    // and no test noticed — the browser did.
    const h1s = screen.getAllByRole("heading", { level: 1 });
    expect(h1s).toHaveLength(1);
    expect(h1s[0]).toHaveTextContent("Endpoints");
  });

  it("renders toolbar, action and children", () => {
    render(
      <ListPageShell title="Endpoints" toolbar={<button type="button">tb</button>} action={<button type="button">+ Endpoint</button>}>
        <p>body</p>
      </ListPageShell>,
    );
    expect(screen.getByRole("button", { name: "tb" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "+ Endpoint" })).toBeInTheDocument();
    expect(screen.getByText("body")).toBeInTheDocument();
  });

  it("omits the description element entirely when there is none", () => {
    const { container } = render(
      <ListPageShell title="Endpoints"><p>body</p></ListPageShell>,
    );
    // Not "renders an empty <p>" — an empty paragraph still takes vertical space
    // and pushes the table down on every page that has no description, which is
    // all six of them today.
    expect(container.querySelector("header p")).toBeNull();
  });
});
