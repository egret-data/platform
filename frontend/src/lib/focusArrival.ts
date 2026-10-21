/**
 * After an action that removes the page holding focus (deleting the thing the
 * page shows, then navigating away), the browser leaves focus on `<body>`
 * (live result §6: after a rule delete and a classification delete). Call
 * this right before such a navigation: once the destination page has mounted
 * its `h1`, focus moves there (made focusable with `tabIndex = -1` if it is
 * not), so a keyboard or screen-reader user resumes from the new page's top.
 *
 * It never steals focus: it acts only while focus is on `<body>` (or on
 * nothing), waits while focus is still in the closing confirm dialog, and
 * gives up after `timeoutMs`. The `h1` that was on screen when
 * it was called is skipped, so the old page's heading is never the target.
 */
export function focusHeadingOnArrival(timeoutMs = 3000): void {
  const before = document.querySelector("h1");
  const started = Date.now();
  const tick = () => {
    const active = document.activeElement;
    // Focus the user (or the new page) put somewhere real is left alone; focus
    // still inside the closing confirm dialog is waited out.
    const inDialog = !!active?.closest('[role="dialog"], [role="alertdialog"]');
    if (active && active !== document.body && active.isConnected && !inDialog) return;
    const heading = [...document.querySelectorAll<HTMLElement>("h1")].find((h) => h !== before && h.isConnected);
    if (heading && !inDialog) {
      if (!heading.hasAttribute("tabindex")) heading.tabIndex = -1;
      heading.focus();
      return;
    }
    if (Date.now() - started < timeoutMs) setTimeout(tick, 50);
  };
  setTimeout(tick, 0);
}
