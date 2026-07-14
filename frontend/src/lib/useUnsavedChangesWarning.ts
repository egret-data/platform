import { useEffect } from "react";

/**
 * Prompt the browser's native "leave site?" dialog before an unload
 * (refresh / tab-close / navigating to an external URL) while `when` is true —
 * e.g. a form has unsaved edits.
 *
 * NOTE: this does NOT intercept in-app SPA navigation (react-router `<Link>`,
 * the sidebar, or the browser Back button within the app). Blocking those
 * requires react-router's `useBlocker`, which is only available under a data
 * router (`createBrowserRouter`); this app mounts `<BrowserRouter>`. `beforeunload`
 * still covers the most common data-loss vectors (refresh / close) everywhere.
 */
export function useUnsavedChangesWarning(when: boolean): void {
  useEffect(() => {
    if (!when) return;
    const handler = (e: BeforeUnloadEvent) => {
      e.preventDefault();
      // Legacy browsers require returnValue to be set to trigger the prompt.
      e.returnValue = "";
    };
    window.addEventListener("beforeunload", handler);
    return () => window.removeEventListener("beforeunload", handler);
  }, [when]);
}
