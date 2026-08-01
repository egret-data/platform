import { describe, it, expect } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { ObjectDetail } from "../ObjectDetail";

const cols = [{ name: "id", type: "int" }, { name: "plan", type: "text" }];
const rows = [[1, "Basic"], [2, "Premium"]];

describe("ObjectDetail", () => {
  it("renders the first record's field/value pairs", () => {
    render(<ObjectDetail columns={cols} rows={rows} />);
    expect(screen.getByText("id")).toBeTruthy();
    expect(screen.getByText("plan")).toBeTruthy();
    expect(screen.getByText("Basic")).toBeTruthy();
    expect(screen.getByText("1 / 2")).toBeTruthy();
  });

  it("pages to the next record and clamps the buttons", () => {
    render(<ObjectDetail columns={cols} rows={rows} />);
    const prev = screen.getByLabelText("previous record") as HTMLButtonElement;
    const next = screen.getByLabelText("next record") as HTMLButtonElement;
    expect(prev.disabled).toBe(true);
    fireEvent.click(next);
    expect(screen.getByText("Premium")).toBeTruthy();
    expect(screen.getByText("2 / 2")).toBeTruthy();
    expect((screen.getByLabelText("next record") as HTMLButtonElement).disabled).toBe(true);
  });

  it("hides the nav bar for a single record", () => {
    render(<ObjectDetail columns={cols} rows={[[1, "Basic"]]} />);
    expect(screen.queryByLabelText("next record")).toBeNull();
  });

  it("shows 'No data' for empty rows", () => {
    render(<ObjectDetail columns={cols} rows={[]} />);
    expect(screen.getByText("No data")).toBeTruthy();
  });

  /**
   * Metabase prints the word "Empty" in a muted tone for an absent value. egret rendered "", a
   * blank row — which in a view of ONE record reads as a rendering fault rather than as "this
   * field has no value", the single most important thing a detail view has to be able to say.
   */
  it("says Empty rather than leaving the row blank", () => {
    render(<ObjectDetail columns={cols} rows={[[null, ""]]} />);
    expect(screen.getAllByText("Empty")).toHaveLength(2);
  });

  // A JSON column arrives as one long line. Metabase pretty-prints it in a <pre>; egret ran it
  // through the plain formatter, so a nested object was an unreadable single row.
  it("pretty-prints a JSON value", () => {
    const { container } = render(
      <ObjectDetail columns={[{ name: "meta", type: "text" }]} rows={[['{"a":1,"b":[2,3]}']]} />,
    );
    const pre = container.querySelector("pre");
    expect(pre).toBeTruthy();
    expect(pre!.textContent).toContain('"a": 1');
    expect(pre!.textContent!.split("\n").length).toBeGreaterThan(3);
  });

  it("leaves a string that only looks like JSON alone", () => {
    const { container } = render(
      <ObjectDetail columns={[{ name: "note", type: "text" }]} rows={[["{not json}"]]} />,
    );
    expect(container.querySelector("pre")).toBeNull();
    expect(screen.getByText("{not json}")).toBeTruthy();
  });

  // One 4,000-character note otherwise pushes every other field off the card.
  it("folds a long value behind Show more, and unfolds it", () => {
    const long = "x".repeat(400);
    render(<ObjectDetail columns={[{ name: "note", type: "text" }]} rows={[[long]]} />);
    expect(screen.queryByText(long)).toBeNull();
    fireEvent.click(screen.getByText("Show more"));
    expect(screen.getByText(long, { exact: false })).toBeTruthy();
    expect(screen.getByText("Show less")).toBeTruthy();
  });

  // Paging must not carry one record's expanded state onto a different record's value.
  it("collapses an expanded value when the record changes", () => {
    const long = "y".repeat(400);
    render(<ObjectDetail columns={[{ name: "note", type: "text" }]} rows={[[long], ["short"]]} />);
    fireEvent.click(screen.getByText("Show more"));
    expect(screen.getByText("Show less")).toBeTruthy();
    fireEvent.click(screen.getByLabelText("next record"));
    fireEvent.click(screen.getByLabelText("previous record"));
    expect(screen.getByText("Show more")).toBeTruthy();
  });

  it("renders an image URL as the image", () => {
    const { container } = render(
      <ObjectDetail columns={[{ name: "avatar", type: "text" }]} rows={[["https://x.test/a.png"]]} />,
    );
    expect(container.querySelector("img")?.getAttribute("src")).toBe("https://x.test/a.png");
  });

  // A plain link is NOT an image. Replacing a readable URL with a broken-image icon is worse than
  // showing the URL, so the match is deliberately narrow.
  it("leaves a non-image URL as text", () => {
    const { container } = render(
      <ObjectDetail columns={[{ name: "site", type: "text" }]} rows={[["https://x.test/page"]]} />,
    );
    expect(container.querySelector("img")).toBeNull();
    expect(screen.getByText("https://x.test/page")).toBeTruthy();
  });
});
