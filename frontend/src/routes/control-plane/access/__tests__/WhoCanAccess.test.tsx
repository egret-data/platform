import { render, screen } from "@testing-library/react";
import { MemoryRouter, Route, Routes, useLocation } from "react-router-dom";
import { describe, expect, test } from "vitest";

import WhoCanAccess from "@/routes/control-plane/access/WhoCanAccess";

/** Surfaces the current location so we can assert the redirect target. */
function LocationDisplay() {
  const loc = useLocation();
  return <div data-testid="location">{loc.pathname + loc.search}</div>;
}

/**
 * The object-direction view was merged into the bidirectional Access Explorer,
 * so the legacy `who-can-access` route now just redirects there in object
 * direction, preserving the old `?ws=&kind=&id=` deep-link params.
 */
describe("WhoCanAccess (legacy redirect)", () => {
  test("redirects to access-explorer in object direction, forwarding params", () => {
    render(
      <MemoryRouter initialEntries={["/control-plane/who-can-access?ws=demo&kind=chart&id=abc"]}>
        <Routes>
          <Route path="/control-plane/who-can-access" element={<WhoCanAccess />} />
          <Route path="*" element={<LocationDisplay />} />
        </Routes>
      </MemoryRouter>,
    );

    const location = screen.getByTestId("location").textContent ?? "";
    expect(location).toContain("/control-plane/access-explorer");
    expect(location).toContain("dir=object");
    expect(location).toContain("ws=demo");
    expect(location).toContain("kind=chart");
    expect(location).toContain("id=abc");
  });
});
