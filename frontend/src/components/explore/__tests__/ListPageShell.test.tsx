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

  it("lets a long description shrink the title block instead of pushing the action under it (KI-183)", () => {
    // jsdom does no layout, so this pins the rule that keeps the action top-right:
    // the title block grows and shrinks from a 20rem basis rather than sizing to its
    // prose (which, with a long description, filled the row and wrapped the action).
    const { container } = render(
      <ListPageShell title="New service account" description={"x ".repeat(200)} action={<button type="button">Create</button>}>
        <p>body</p>
      </ListPageShell>,
    );
    const titleBlock = container.querySelector("header")!.firstElementChild;
    expect(titleBlock).toHaveClass("flex-[1_1_20rem]", "min-w-0");
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

  it("renders `back` above the title and outside the action group", () => {
    // The slot exists because a create page's way OUT must not sit in the same
    // group as its commit: `EndpointCreate` had `← Endpoints` in `toolbar`,
    // which renders beside `action`, one control from "Create endpoint". Every
    // detail screen puts that link top-left, above the name.
    const { container } = render(
      <ListPageShell
        title="New endpoint"
        back={<a href="/endpoints">← Endpoints</a>}
        action={<button type="button">Create endpoint</button>}
      >
        <p>body</p>
      </ListPageShell>,
    );
    const back = screen.getByRole("link", { name: "← Endpoints" });
    const h1 = screen.getByRole("heading", { level: 1 });
    // Above the title, not merely present.
    expect(back.compareDocumentPosition(h1) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    const actions = container.querySelector("header")!.lastElementChild as HTMLElement;
    expect(actions).toContainElement(screen.getByRole("button", { name: "Create endpoint" }));
    expect(actions).not.toContainElement(back);
  });

  it("emits nothing for `back` when there is none", () => {
    // The same rule as `description`: an omitted slot renders no element at
    // all, so the six pages that do not pass one are byte-identical to what
    // they were before the slot existed.
    const { container } = render(
      <ListPageShell title="Endpoints"><p>body</p></ListPageShell>,
    );
    expect(container.querySelector("header a")).toBeNull();
    expect(container.querySelector("header")!.firstElementChild!.firstElementChild!.tagName).toBe("H1");
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
