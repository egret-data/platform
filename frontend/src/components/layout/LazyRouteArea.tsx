import React, { Suspense, type ReactNode } from "react";
import { RotateCw } from "lucide-react";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";

/**
 * Catches a lazy route area whose chunk fails to load, so the failure costs
 * that area and not the whole app.
 *
 * A rejected `lazy()` import throws during render, and with no boundary the
 * whole React tree unmounts: `#root` goes empty and the user gets a blank
 * white page. `MonacoEditor.tsx` records that as measured, and names the
 * routine trigger: a deploy replaces a content-hashed chunk while someone has
 * the SPA open, and their next click asks for a file that no longer exists.
 * Every deploy that touches a lazy area changes its chunk's name, so this is
 * the ordinary case, not an exotic one.
 *
 * Modelled on `MonacoErrorBoundary`. A reload is the remedy because it fetches
 * the new `index.html`, which names the chunks that do exist.
 */
class LazyAreaErrorBoundary extends React.Component<
  { area: string; children: ReactNode },
  { error: Error | null }
> {
  state = { error: null as Error | null };
  static getDerivedStateFromError(error: Error) {
    return { error };
  }
  componentDidCatch(error: Error, info: React.ErrorInfo) {
    console.error(`The ${this.props.area} pages failed to load:`, error, info.componentStack);
  }
  render() {
    if (this.state.error) {
      return (
        <div className="p-4">
          <Alert variant="destructive">
            <AlertDescription className="flex flex-wrap items-center gap-3">
              <span>
                The {this.props.area} pages could not be loaded. The app may have been updated
                since you opened it. Reload the page to try again.
              </span>
              <Button variant="outline" size="sm" onClick={() => window.location.reload()}>
                <RotateCw aria-hidden="true" />
                Reload page
              </Button>
            </AlertDescription>
          </Alert>
        </div>
      );
    }
    return this.props.children;
  }
}

/**
 * The wrapper every lazy route area in `App.tsx` mounts inside: an error
 * boundary around `Suspense`, so a slow chunk shows `fallback` and a failed
 * one shows a reload, never a blank page (FE-ROUTE-2).
 *
 * `area` names the area in the error ("The Workflows pages could not be
 * loaded"), so the reader knows what failed and that the rest of the app is
 * fine.
 */
export function LazyRouteArea({
  area,
  fallback,
  children,
}: {
  area: string;
  fallback: ReactNode;
  children: ReactNode;
}) {
  return (
    <LazyAreaErrorBoundary area={area}>
      <Suspense fallback={fallback}>{children}</Suspense>
    </LazyAreaErrorBoundary>
  );
}
