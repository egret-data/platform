import { StatePage } from "@/components/layout/StatePage";

/** A URL that names no page: the shared state frame, with a way back (FE-STATE-5). */
export function NotFound() {
  return (
    <StatePage eyebrow="404 — Not found" title="This page doesn't exist." link={{ to: "/", label: "Go to the home page" }}>
      <p>The address may be mistyped, or the page may have moved.</p>
    </StatePage>
  );
}
