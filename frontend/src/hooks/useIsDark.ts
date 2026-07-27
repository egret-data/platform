import { useEffect, useState } from "react";

/**
 * Tracks whether the app is in dark mode by observing the `dark` class the
 * ThemeProvider toggles on `<html>`. Deliberately reads the DOM instead of the
 * theme context so it works in any tree (e.g. Monaco embeds rendered in tests
 * without a ThemeProvider) and resolves `system` → concrete automatically.
 */
export function useIsDark(): boolean {
  const [isDark, setIsDark] = useState(() =>
    typeof document !== "undefined" &&
    document.documentElement.classList.contains("dark"),
  );

  useEffect(() => {
    const root = document.documentElement;
    const sync = () => setIsDark(root.classList.contains("dark"));
    sync();
    const observer = new MutationObserver(sync);
    observer.observe(root, { attributes: true, attributeFilter: ["class"] });
    return () => observer.disconnect();
  }, []);

  return isDark;
}
