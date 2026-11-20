import type { ReactNode } from "react";
import { Link, useLocation, useNavigate } from "react-router-dom";
import { ArrowLeft } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";
import { PageContainer } from "./PageContainer";
import { PageTitle } from "./PageTitle";

/**
 * The frame of a page that has nothing of its own to show: a refusal (403), a
 * missing page (404), a feature that is not there yet (FE-STATE-5).
 *
 * It renders INSIDE the app shell, as any page does, at the list's left edge
 * with a `PageTitle`; the reader keeps the header and the sidebar (KI-230). Its
 * actions are the way out: `Go back` when there is a page to go back to, and
 * one link to somewhere the reader can open.
 *
 *   EYEBROW (the status code a reader may quote to an admin)
 *   Title
 *   body
 *   [Go back] [link]
 */
export function StatePage({
  eyebrow,
  title,
  children,
  link,
}: {
  eyebrow?: string;
  title: string;
  children?: ReactNode;
  link: { to: string; label: string };
}) {
  return (
    <PageContainer>
      <div className="max-w-xl space-y-2 py-6" data-state-page>
        {eyebrow ? <p className="text-xs font-medium text-muted-foreground">{eyebrow}</p> : null}
        <PageTitle>{title}</PageTitle>
        {children ? <div className="space-y-2 text-sm text-muted-foreground">{children}</div> : null}
        <StateActions link={link} />
      </div>
    </PageContainer>
  );
}

/** `Go back` (when there is somewhere to go back to) and one link onwards. */
export function StateActions({ link }: { link: { to: string; label: string } }) {
  const navigate = useNavigate();
  // react-router gives the first location of a tab the key "default": there
  // is no page of this app to go back to, and `navigate(-1)` would leave it.
  const canGoBack = useLocation().key !== "default";
  return (
    <div className="flex flex-wrap items-center gap-2 pt-2">
      {canGoBack ? (
        <Button variant="outline" size="sm" onClick={() => navigate(-1)}>
          <ArrowLeft className="h-4 w-4" aria-hidden="true" />
          Go back
        </Button>
      ) : null}
      <Button asChild size="sm">
        <Link to={link.to}>{link.label}</Link>
      </Button>
    </div>
  );
}

/**
 * The state frame for a page that renders BEFORE the app shell exists: sign-in,
 * "No workspaces yet" and the bare loading state (KI-279). Same look as
 * `StatePage` (a `max-w-xl` block with a `PageTitle`, a muted body, actions
 * below) on the page background, centred in the viewport because there is no
 * shell or list edge to align to.
 */
export function StandaloneStatePage({
  title,
  children,
  actions,
  width = "xl",
}: {
  title?: string;
  children?: ReactNode;
  actions?: ReactNode;
  width?: "sm" | "xl";
}) {
  return (
    <div className="flex min-h-screen items-center justify-center bg-background p-4">
      <div
        className={`w-full space-y-2 py-6 ${width === "sm" ? "max-w-sm" : "max-w-xl"}`}
        data-state-page
        data-standalone
      >
        {title ? <PageTitle>{title}</PageTitle> : null}
        {children ? <div className="space-y-2 text-sm text-muted-foreground">{children}</div> : null}
        {actions ? <div className="flex flex-wrap items-center gap-2 pt-2">{actions}</div> : null}
      </div>
    </div>
  );
}

/** The loading state before the shell exists: the standalone frame with a skeleton, no bare text. */
export function StandaloneLoading() {
  return (
    <StandaloneStatePage>
      <div role="status" aria-label="Loading" className="space-y-2">
        <Skeleton className="h-6 w-48" />
        <Skeleton className="h-4 w-72" />
      </div>
    </StandaloneStatePage>
  );
}
