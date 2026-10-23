import { useParams } from "react-router-dom";
import { useMyWorkspaces } from "@/api/hooks";
import { ApiError } from "@/api/client";
import { StateActions, StatePage } from "@/components/layout/StatePage";

const HOME = { to: "/", label: "Go to the home page" };

/**
 * Whether `error` is the server refusing the caller (403). The route guards
 * and the client's view of permissions only decide what is SHOWN; this is the
 * answer of the gate itself, so a page whose main read is refused renders
 * `Forbidden` instead of a red box with the API's word in it (KI-231).
 */
export function isForbidden(error: unknown): error is ApiError {
  return error instanceof ApiError && error.status === 403;
}

/**
 * The reason to show for a refusal the server sent: a module switched off for
 * the workspace says so; any other refusal is a missing permission, which the
 * caller names (`fallback`) because the body only says "forbidden".
 */
export function refusalReason(error: unknown, fallback?: string): string | undefined {
  if (error instanceof ApiError && error.code === "feature_disabled") {
    return "This module is switched off for this workspace.";
  }
  return fallback;
}

/**
 * The link onwards from a refusal: a workspace the reader can open. The
 * current one when they belong to it (the refusal is about one page in it),
 * else their first; with none, the home page.
 */
function useOnwardLink(): { to: string; label: string } {
  const { workspace } = useParams();
  const { data } = useMyWorkspaces(true);
  const list = data ?? [];
  const target = list.find((w) => w.slug === workspace) ?? list[0];
  if (!target) return HOME;
  return { to: `/explore/${target.slug}`, label: `Open ${target.name}` };
}

/**
 * The one "no access" state (FE-STATE-5; KI-230, KI-231). Every refusal uses
 * it: a route guard, a page whose main read the server refused (`isForbidden`),
 * and a settings section.
 *
 * - `level="page"` (the default): the whole page, inside the app shell, with
 *   the page's `h1`.
 * - `level="section"`: one section of a page that otherwise opens (a
 *   Workspace settings section); an `h2` in a panel, the page's title stays.
 *
 * It says the reader has no access, why when the caller knows (`reason`), and
 * offers a way out: back, and a workspace they can open (`back` overrides that
 * link). "403 — Forbidden" stays as the eyebrow: it is the status code a
 * reader may quote to an admin, and routing tests key on it.
 */
export function Forbidden({
  reason,
  back,
  level = "page",
}: {
  reason?: string;
  back?: { to: string; label: string };
  level?: "page" | "section";
}) {
  const onward = useOnwardLink();
  const link = back ?? onward;
  if (level === "section") {
    return (
      <section className="space-y-2 rounded-md border bg-muted/40 p-6" aria-labelledby="no-access-title">
        <p className="text-xs font-medium text-muted-foreground">403 — Forbidden</p>
        <h2 id="no-access-title" className="text-base font-semibold">
          You don&apos;t have access to this section.
        </h2>
        {reason ? <p className="text-sm text-muted-foreground">{reason}</p> : null}
        <StateActions link={link} />
      </section>
    );
  }
  return (
    <StatePage eyebrow="403 — Forbidden" title="You don't have access to this page." link={link}>
      {reason ? <p>{reason}</p> : null}
    </StatePage>
  );
}
