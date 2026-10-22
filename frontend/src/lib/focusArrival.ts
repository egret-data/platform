/**
 * After an action that removes the page holding focus (deleting the thing the
 * page shows, then navigating away), the browser leaves focus on `<body>`
 * (live result §6: after a rule delete and a classification delete). Call
 * this right before such a navigation: once the destination page has mounted
 * its `h1`, focus moves there (made focusable with `tabIndex = -1` if it is
 * not), so a keyboard or screen-reader user resumes from the new page's top.
 *
 * It never steals focus: once the new heading is there, it acts only while
 * focus is on `<body>` (or on nothing); it waits while focus is still in the
 * closing confirm dialog, follows a heading that is replaced while it holds
 * focus, and stops at `timeoutMs` or once focus moves on. The `h1` that was on screen when
 * it was called is skipped, so the old page's heading is never the target.
 */
export function focusHeadingOnArrival(timeoutMs = 10000): void {
  const before = document.querySelector("h1");
  const started = Date.now();
  // The heading this put focus on, if any. A page can render a heading for a
  // moment and replace it (the old page re-rendering before it unmounts, a
  // skeleton giving way): while focus falls back to <body> with it, the next
  // heading takes focus, until the deadline.
  let focused: HTMLElement | null = null;
  const tick = () => {
    // Nothing is decided until the new page's heading is there: until then,
    // focus may sit for a moment on the old page (a confirm dialog returning
    // it to its opener) before that page unmounts.
    const heading = [...document.querySelectorAll<HTMLElement>("h1")].find((h) => h !== before && h.isConnected);
    const active = document.activeElement;
    const inDialog = !!active?.closest('[role="dialog"], [role="alertdialog"]');
    const onBody = !active || active === document.body;
    if (focused && !onBody && active !== focused) return; // the user, or the page, moved focus on
    if (heading && !inDialog && (onBody || (!focused && !active?.isConnected))) {
      if (!heading.hasAttribute("tabindex")) heading.tabIndex = -1;
      heading.focus();
      focused = heading;
    } else if (heading && !inDialog && !focused && active && active.isConnected && !onBody) {
      // Focus the user (or the new page) put somewhere real is left alone.
      return;
    }
    if (Date.now() - started < timeoutMs) setTimeout(tick, 50);
  };
  setTimeout(tick, 0);
}
