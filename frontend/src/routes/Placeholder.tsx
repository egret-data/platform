import { StatePage } from "@/components/layout/StatePage";

/**
 * A page whose feature is not built yet. It says so plainly in the shared
 * state frame (FE-STATE-5) rather than looking like a feature with nothing in
 * it (KI-227).
 */
export function Placeholder({
  title,
  link = { to: "/", label: "Go to the home page" },
}: {
  title: string;
  link?: { to: string; label: string };
}) {
  return (
    <StatePage eyebrow="Not available yet" title={title} link={link}>
      <p>This feature is not available yet. Nothing on this page works today.</p>
    </StatePage>
  );
}
