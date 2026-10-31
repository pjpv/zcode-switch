import assert from 'node:assert/strict';
import test from 'node:test';
import { createAutoClaimScheduler } from '../src/auto-claim-scheduler.js';

// Virtual time only: these tests never invoke Tauri, account APIs or real timers.
function fakeClock() {
  let time = 0;
  let nextId = 1;
  const timers = new Map();
  const flush = async () => { for (let i = 0; i < 12; i++) await Promise.resolve(); };
  return {
    now: () => time,
    setTimer(fn, delay) {
      const id = nextId++;
      timers.set(id, { at: time + delay, fn });
      return id;
    },
    clearTimer: id => timers.delete(id),
    async advance(ms) {
      const target = time + ms;
      await flush();
      let turns = 0;
      while (true) {
        const due = [...timers].filter(([, value]) => value.at <= target)
          .sort((a, b) => a[1].at - b[1].at || a[0] - b[0])[0];
        if (!due) break;
        assert.ok(++turns < 1000, 'scheduler must not spin at the same instant');
        time = due[1].at;
        timers.delete(due[0]);
        due[1].fn();
        await flush();
      }
      time = target;
      await flush();
    },
    flush,
    pending: () => timers.size,
  };
}

function deferred() {
  let resolve;
  let reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}

function setup(t, overrides = {}) {
  const clock = fakeClock();
  const calls = [];
  const errors = [];
  const scheduler = createAutoClaimScheduler({
    now: clock.now,
    setTimer: clock.setTimer,
    clearTimer: clock.clearTimer,
    runRound: async () => { calls.push(clock.now()); },
    isBusy: () => false,
    onError: error => errors.push(error),
    intervalMs: 100,
    startDelayMs: 10,
    busyDelayMs: 5,
    ...overrides,
  });
  t.after(() => scheduler.dispose());
  return { clock, calls, errors, scheduler };
}

test('restoring enabled state runs without another toggle and continues periodically', async t => {
  const { clock, calls, scheduler } = setup(t);
  scheduler.setEnabled(true);
  assert.deepEqual(scheduler.getStatus(), { enabled: true, running: false, phase: 'waiting', nextAt: 10 });
  await clock.advance(9);
  assert.equal(calls.length, 0);
  await clock.advance(1);
  assert.deepEqual(calls, [10]);
  await clock.advance(300);
  assert.deepEqual(calls, [10, 110, 210, 310]);
});

test('repeated observation of enabled state cannot postpone the due round', async t => {
  const { clock, calls, scheduler } = setup(t);
  scheduler.setEnabled(true);
  for (let i = 0; i < 10; i++) {
    scheduler.setEnabled(true);
    await clock.advance(1);
  }
  assert.deepEqual(calls, [10]);
  for (let i = 0; i < 10; i++) {
    scheduler.setEnabled(true);
    await clock.advance(10);
  }
  assert.deepEqual(calls, [10, 110]);
});

test('busy across several due windows recovers automatically after work finishes', async t => {
  let busy = true;
  const { clock, calls, scheduler } = setup(t, { isBusy: () => busy });
  scheduler.setEnabled(true);
  await clock.advance(225);
  assert.equal(calls.length, 0);
  assert.equal(scheduler.getStatus().phase, 'busy');
  assert.ok(scheduler.getStatus().nextAt > clock.now());
  busy = false;
  await clock.advance(5);
  assert.equal(calls.length, 1);
  await clock.advance(100);
  assert.equal(calls.length, 2);
});

test('manual refresh completion can request an early round without toggling', async t => {
  let busy = false;
  const { clock, calls, scheduler } = setup(t, { isBusy: () => busy });
  scheduler.setEnabled(true);
  await clock.advance(10);
  busy = true;
  await clock.advance(20);
  busy = false;
  scheduler.request();
  await clock.advance(10);
  assert.equal(calls.length, 2, 'request must not wait for the normal periodic deadline');
});

test('requests during a round coalesce and never overlap the running promise', async t => {
  const first = deferred();
  let calls = 0;
  let active = 0;
  let maximum = 0;
  const { clock, scheduler } = setup(t, {
    runRound: async () => {
      calls++;
      active++;
      maximum = Math.max(maximum, active);
      if (calls === 1) await first.promise;
      active--;
    },
  });
  scheduler.setEnabled(true);
  await clock.advance(10);
  assert.equal(scheduler.getStatus().phase, 'running');
  assert.equal(scheduler.getStatus().running, true);
  for (let i = 0; i < 20; i++) { scheduler.request(); scheduler.setEnabled(true); }
  await clock.advance(300);
  assert.equal(calls, 1);
  first.resolve();
  await clock.flush();
  await clock.advance(10);
  assert.equal(calls, 2);
  assert.equal(maximum, 1);
  await clock.advance(89);
  assert.equal(calls, 2, 'coalesced requests must not leave additional immediate rounds');
});

test('periodic delay begins after completion of a slow round', async t => {
  const first = deferred();
  let calls = 0;
  const { clock, scheduler } = setup(t, { runRound: () => ++calls === 1 ? first.promise : Promise.resolve() });
  scheduler.setEnabled(true);
  await clock.advance(310);
  assert.equal(calls, 1);
  first.resolve();
  await clock.flush();
  await clock.advance(99);
  assert.equal(calls, 1);
  await clock.advance(1);
  assert.equal(calls, 2);
});

test('disable cancels a pending round and requests cannot enable it', async t => {
  const { clock, calls, scheduler } = setup(t);
  scheduler.setEnabled(true);
  await clock.advance(9);
  scheduler.setEnabled(false);
  scheduler.request();
  await clock.advance(500);
  assert.equal(calls.length, 0);
  assert.deepEqual(scheduler.getStatus(), { enabled: false, running: false, phase: 'disabled', nextAt: null });
  assert.equal(clock.pending(), 0);
  scheduler.setEnabled(true);
  await clock.advance(10);
  assert.equal(calls.length, 1);
});

test('disable during a round lets it finish but cancels queued requests', async t => {
  const first = deferred();
  let calls = 0;
  const { clock, scheduler } = setup(t, { runRound: () => { calls++; return first.promise; } });
  scheduler.setEnabled(true);
  await clock.advance(10);
  scheduler.request();
  scheduler.setEnabled(false);
  first.resolve();
  await clock.flush();
  await clock.advance(500);
  assert.equal(calls, 1);
  assert.equal(scheduler.getStatus().running, false);
  assert.equal(clock.pending(), 0);
  scheduler.setEnabled(true);
  await clock.advance(10);
  assert.equal(calls, 2);
});

test('disable and re-enable while running preserves serial execution and resumes', async t => {
  const first = deferred();
  let calls = 0;
  const { clock, scheduler } = setup(t, { runRound: () => ++calls === 1 ? first.promise : Promise.resolve() });
  scheduler.setEnabled(true);
  await clock.advance(10);
  scheduler.setEnabled(false);
  scheduler.setEnabled(true);
  await clock.advance(200);
  assert.equal(calls, 1);
  first.resolve();
  await clock.flush();
  await clock.advance(10);
  assert.equal(calls, 2);
});

test('a rejected round reports its error, releases the lock and retries next interval', async t => {
  const failure = new Error('synthetic request failure');
  let calls = 0;
  const { clock, errors, scheduler } = setup(t, {
    runRound: async () => { if (++calls === 1) throw failure; },
  });
  scheduler.setEnabled(true);
  await clock.advance(10);
  assert.deepEqual(errors, [failure]);
  assert.equal(scheduler.getStatus().running, false);
  await clock.advance(100);
  assert.equal(calls, 2);
});

test('dispose clears pending work and late completion cannot restart timers', async t => {
  const first = deferred();
  let calls = 0;
  const { clock, scheduler } = setup(t, { runRound: () => { calls++; return first.promise; } });
  scheduler.setEnabled(true);
  await clock.advance(10);
  scheduler.request();
  scheduler.dispose();
  first.resolve();
  await clock.flush();
  await clock.advance(500);
  assert.equal(calls, 1);
  assert.equal(clock.pending(), 0);
});
