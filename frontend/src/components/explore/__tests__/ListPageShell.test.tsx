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

  it("wraps the action group, not only the header row", () => {
    // The header's own `flex-wrap` only breaks the title away from the button
    // group. Endpoints puts three controls in that group (bulk toggle,
    // "Download OpenAPI", "+ Endpoint"), which overflows a narrow viewport on
    // its own — the exact overflow the shell's comment claims to fix.
    const { container } = render(
      <ListPageShell title="Endpoints" toolbar={<button type="button">tb</button>} action={<button type="button">+ Endpoint</button>}>
        <p>body</p>
      </ListPageShell>,
    );
    const actions = container.querySelector("header")!.lastElementChild;
    expect(actions).toHaveTextContent("+ Endpoint");
    expect(actions).toHaveClass("flex-wrap");
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

  it("centers the header row when there is no description, and top-aligns it when there is", () => {
    // Checking a class name is a weaker assertion than checking behaviour, so
    // keep it narrow: assert only the alignment class, not the whole
    // `className` string, or this breaks on every unrelated class change.
    const { container: withoutDescription } = render(
      <ListPageShell title="Endpoints"><p>body</p></ListPageShell>,
    );
    const headerWithoutDescription = withoutDescription.querySelector("header");
    expect(headerWithoutDescription).not.toBeNull();
    expect(headerWithoutDescription).toHaveClass("items-center");
    expect(headerWithoutDescription).not.toHaveClass("items-start");

    const { container: withDescription } = render(
      <ListPageShell title="Endpoints" description="Some description"><p>body</p></ListPageShell>,
    );
    const headerWithDescription = withDescription.querySelector("header");
    expect(headerWithDescription).not.toBeNull();
    expect(headerWithDescription).toHaveClass("items-start");
    expect(headerWithDescription).not.toHaveClass("items-center");
  });
});
