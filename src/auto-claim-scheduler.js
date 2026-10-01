// Scheduling only: account cooldowns, verification and claims remain in the
// existing round. A busy deadline stays pending until the round can run.
export function createAutoClaimScheduler({
  runRound, isBusy = () => false, now = Date.now,
  setTimer = setTimeout, clearTimer = clearTimeout, onError = () => {},
  intervalMs = 10 * 60 * 1000, startDelayMs = 1000, busyDelayMs = 5000,
}) {
  let enabled = false, running = false, timer = null, due = null;
  let phase = 'disabled', nextAt = null;

  function clear() {
    if (timer !== null) clearTimer(timer);
    timer = null;
    nextAt = null;
  }
  function arm(delay, nextPhase = 'waiting') {
    clear();
    if (!enabled) return;
    phase = nextPhase;
    nextAt = now() + Math.max(0, delay);
    timer = setTimer(() => { timer = null; nextAt = null; void tick(); }, Math.max(0, delay));
  }
  async function tick() {
    if (!enabled) return;
    if (running || isBusy()) { arm(busyDelayMs, 'busy'); return; }
    running = true;
    due = null;
    phase = 'running';
    try { await runRound(); }
    catch (error) { onError(error); }
    finally {
      running = false;
      if (enabled) {
        due ??= now() + intervalMs;
        arm(Math.max(0, due - now()));
      } else {
        clear();
        phase = 'disabled';
      }
    }
  }
  function setEnabled(value) {
    value = !!value;
    if (value === enabled) return;
    enabled = value;
    clear();
    due = value ? now() + startDelayMs : null;
    if (!value) phase = 'disabled';
    else if (!running) arm(startDelayMs);
  }
  function request() {
    if (!enabled) return;
    due = Math.min(due ?? Infinity, now() + startDelayMs);
    if (!running) arm(Math.max(0, due - now()));
  }
  return {
    setEnabled, request,
    getStatus: () => ({ enabled, running, phase, nextAt }),
    dispose: () => setEnabled(false),
  };
}
