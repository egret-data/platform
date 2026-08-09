import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
import { IFrameCard } from "../IFrameCard";
import { IFRAME_SANDBOX } from "../iframe-url";

describe("IFrameCard (allowed content)", () => {
  it("renders the frame with the sandbox, referrer policy and src", () => {
    const { container } = render(<IFrameCard iframe="https://www.loom.com/share/abc123" />);
    const frame = container.querySelector("iframe");
    expect(frame).not.toBeNull();
    // Compare against the exported constant, NOT a literal: the constant's own contents are
    // pinned token by token in iframe-url.test.ts, and duplicating the string here would let a
    // change pass by being made in two places at once.
    expect(frame).toHaveAttribute("sandbox", IFRAME_SANDBOX);
    expect(frame).toHaveAttribute("referrerpolicy", "strict-origin-when-cross-origin");
    expect(frame).toHaveAttribute("src", "https://www.loom.com/embed/abc123");
  });

  it("sets no allow attribute when the input asked for none", () => {
    const { container } = render(<IFrameCard iframe="https://www.loom.com/share/abc123" />);
    expect(container.querySelector("iframe")).not.toHaveAttribute("allow");
  });

  it("carries the filtered allow list through from a snippet", () => {
    const { container } = render(
      <IFrameCard iframe='<iframe src="https://www.youtube.com/embed/x" allow="autoplay; camera" allowfullscreen></iframe>' />,
    );
    const frame = container.querySelector("iframe");
    expect(frame).toHaveAttribute("allow", "autoplay");
    expect(frame).toHaveAttribute("allowfullscreen");
  });

  it("gives the frame an accessible name", () => {
    // WCAG 4.1.2 / technique H64: a frame with no title is unlabelled to a screen reader.
    render(<IFrameCard iframe="https://www.loom.com/share/abc123" />);
    expect(screen.getByTitle("Embedded content")).toBeInTheDocument();
  });

  it("does not swallow the click that opens the card settings while editing", () => {
    // A live iframe eats pointer events, so in edit mode the click would never reach the card
    // wrapper that opens the settings panel (DashboardView.tsx).
    const { container } = render(<IFrameCard iframe="https://www.loom.com/share/abc123" editing />);
    expect(container.querySelector("iframe")).toHaveClass("pointer-events-none");
  });
});

describe("IFrameCard (fails closed)", () => {
  it("renders no iframe at all for an unset card", () => {
    const { container } = render(<IFrameCard iframe="" editing />);
    expect(container.querySelector("iframe")).toBeNull();
    expect(screen.getByText("No embed set")).toBeInTheDocument();
  });

  it("renders no iframe for a javascript: URL, and says something is wrong", () => {
    const { container } = render(<IFrameCard iframe="javascript://youtube.com/%0aalert(1)" editing />);
    expect(container.querySelector("iframe")).toBeNull();
    expect(screen.getByText("There was a problem rendering this content.")).toBeInTheDocument();
  });

  it("renders no iframe for a data: URL, and states the reason", () => {
    // The reason assertion is NOT decoration. Absence of an <iframe> is also what a stubbed
    // `if (!resolved.ok) return <div />` produces, so on its own this test survived that exact
    // mutation while its four siblings died. An assertion that a stub satisfies is not evidence.
    const { container } = render(<IFrameCard iframe="data://youtube.com/x" editing />);
    expect(container.querySelector("iframe")).toBeNull();
    expect(screen.getByText("There was a problem rendering this content.")).toBeInTheDocument();
  });

  it("names the blocked host while editing", () => {
    const { container } = render(<IFrameCard iframe="https://example.com/x" editing />);
    expect(container.querySelector("iframe")).toBeNull();
    expect(screen.getByText("example.com can't be embedded in an iframe card.")).toBeInTheDocument();
  });

  it("does NOT name the blocked host in view mode", () => {
    // Metabase draws the same line (IFrameViz.tsx:134-139): a viewer who cannot change the card
    // gains nothing from the hostname, and a dashboard is shareable.
    const { container } = render(<IFrameCard iframe="https://example.com/x" />);
    expect(container.querySelector("iframe")).toBeNull();
    expect(screen.queryByText(/example\.com/)).toBeNull();
    expect(screen.getByText("There was a problem rendering this content.")).toBeInTheDocument();
  });
});
