/**
 * Yield until the browser has had a chance to paint.
 *
 * `requestAnimationFrame` is a *frame* request, not a timer: Chromium suspends
 * the callbacks entirely while the tab's renderer is not being composited — a
 * backgrounded or occluded window, a hidden webview, or an automated/headless
 * session. `document.visibilityState` can still read `'visible'` in those
 * cases, so visibility alone is not a usable guard. Startup code that awaits a
 * frame therefore waits forever: the Web client froze on "loading" until the
 * tab was actually drawn (forcing compositing made the album appear at once).
 *
 * Racing the frame against a short timer keeps the paint-deferral intent on a
 * live tab (about one frame) and guarantees progress when frames never arrive.
 */
export function afterNextPaint(timeoutMs = 100): Promise<void> {
  return new Promise<void>((resolve) => {
    let settled = false;
    const timers: Array<ReturnType<typeof setTimeout>> = [];
    const finish = () => {
      if (settled) return;
      settled = true;
      timers.forEach((id) => clearTimeout(id));
      resolve();
    };
    if (typeof requestAnimationFrame === 'function') {
      requestAnimationFrame(() => timers.push(setTimeout(finish, 0)));
    }
    timers.push(setTimeout(finish, timeoutMs));
  });
}
