import assert from 'node:assert/strict';
import test from 'node:test';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';
import { createClaimResultWaiter } from '../src/claim-result.js';

function fakeClock() {
  let now = 0;
  let id = 0;
  const timers = new Map();
  return {
    setTimer(fn, ms) { const handle = ++id; timers.set(handle, { at: now + ms, fn }); return handle; },
    clearTimer: handle => timers.delete(handle),
    advance(ms) {
      now += ms;
      for (const [handle, timer] of [...timers]) {
        if (timer.at <= now) { timers.delete(handle); timer.fn(); }
      }
    },
    pending: () => timers.size,
  };
}

function fixture(claimId = 'claim-a', clock = fakeClock()) {
  const waiter = createClaimResultWaiter({ accountId: 'account-a', claimId, timeoutMs: 50, ...clock });
  return { waiter, clock };
}

const payload = (claimId = 'claim-a', accountId = 'account-a') => ({ accountId, claimId, ok: true });

test('a result received immediately is retained until its promise is awaited', async () => {
  const { waiter, clock } = fixture();
  const result = payload();
  assert.equal(waiter.accept(result), true);
  assert.equal(await waiter.promise, result);
  assert.equal(clock.pending(), 0);
});

test('another claim for the same account cannot consume the receipt', async () => {
  const { waiter, clock } = fixture();
  assert.equal(waiter.accept(payload('older-claim')), false);
  assert.equal(clock.pending(), 1);
  const result = payload();
  assert.equal(waiter.accept(result), true);
  assert.equal(await waiter.promise, result);
});

test('wrong account and malformed payloads are ignored', async () => {
  const { waiter, clock } = fixture();
  for (const value of [payload('claim-a', 'account-b'), null, undefined, {}, { accountId: 'account-a' }]) {
    assert.equal(waiter.accept(value), false);
  }
  assert.equal(clock.pending(), 1);
  waiter.finish(null);
  assert.equal(await waiter.promise, null);
});

test('duplicate results and a later finish cannot change the first outcome', async () => {
  const { waiter } = fixture();
  const first = payload();
  assert.equal(waiter.accept(first), true);
  assert.equal(waiter.accept({ ...payload(), ok: false }), false);
  assert.equal(waiter.finish(null), false);
  assert.equal(await waiter.promise, first);
});

test('timeout returns null and late results cannot resolve the next account receipt', async () => {
  const { waiter: first, clock } = fixture();
  clock.advance(49);
  assert.equal(clock.pending(), 1);
  clock.advance(1);
  assert.equal(await first.promise, null);
  const { waiter: next } = fixture('claim-b', clock);
  assert.equal(first.accept(payload()), false);
  assert.equal(next.accept(payload()), false);
  assert.equal(clock.pending(), 1);
  const result = payload('claim-b');
  next.accept(result);
  assert.equal(await next.promise, result);
  assert.equal(clock.pending(), 0);
});

test('setup failure or cancellation can finish and release the timer exactly once', async () => {
  const { waiter, clock } = fixture();
  assert.equal(waiter.finish(null), true);
  assert.equal(waiter.finish(null), false);
  assert.equal(clock.pending(), 0);
  clock.advance(500);
  assert.equal(await waiter.promise, null);
});

// Execute the actual production caller, without loading DOM/Tauri dependencies.
// The bounded extraction intentionally fails if this code moves, instead of
// silently testing a duplicated implementation of the request ordering.
const main = readFileSync(new URL('../src/main.js', import.meta.url), 'utf8');
const start = main.indexOf('let claimWaiter = null;');
const end = main.indexOf('async function awaitClaimPreviewFresh', start);
assert.ok(start >= 0 && end > start, 'locate production claim waiter and performClaim');
const productionCaller = main.slice(start, end);

function callerFixture(invokeImpl) {
  const clock = fakeClock();
  const calls = [];
  let sequence = 0;
  let api;
  const context = vm.createContext({
    crypto: { randomUUID: () => `request-${++sequence}` },
    createClaimResultWaiter: options => createClaimResultWaiter({ ...options, ...clock }),
    invoke: (command, args) => { calls.push({ command, args }); return invokeImpl(command, args, api); },
  });
  api = vm.runInContext(`${productionCaller}\n({ performClaim, accept: p => claimWaiter?.accept(p), current: () => claimWaiter })`, context);
  return { api, clock, calls };
}

test('production performClaim registers receipt before invoking a synchronously completing claim', async () => {
  let releaseStart;
  const startDone = new Promise(resolve => { releaseStart = resolve; });
  let delivered;
  const { api, clock, calls } = callerFixture((command, args, caller) => {
    assert.equal(command, 'claim_start');
    delivered = { accountId: args.id, claimId: args.claimId, ok: true };
    assert.equal(caller.accept(delivered), true);
    return startDone;
  });
  const outcome = api.performClaim('account-a', { plan_id: 'plan-a' }, true, 50);
  assert.equal(clock.pending(), 0);
  releaseStart();
  assert.equal(await outcome, delivered);
  assert.equal(api.current(), null);
  assert.equal(calls.length, 1);
  assert.equal(calls[0].args.planId, 'plan-a');
  assert.equal(calls[0].args.auto, true);
});

test('production invoke setup failure releases the waiter and propagates its error', async () => {
  const failure = new Error('synthetic setup failure');
  const { api, clock } = callerFixture(() => { throw failure; });
  await assert.rejects(api.performClaim('account-a', { plan_id: 'plan-a' }), error => error === failure);
  assert.equal(api.current(), null);
  assert.equal(clock.pending(), 0);
});

test('production timeout cancels only its request ID and rejects stale receipts for the next claim', async () => {
  const { api, clock, calls } = callerFixture(() => Promise.resolve());
  const first = api.performClaim('account-a', { plan_id: 'plan-a' }, true, 50);
  await Promise.resolve();
  clock.advance(50);
  assert.equal(await first, null);
  assert.equal(api.current(), null);
  const cancel = calls.find(call => call.command === 'claim_cancel');
  assert.ok(cancel);
  assert.equal(cancel.args.claimId, 'request-1');
  const second = api.performClaim('account-a', { plan_id: 'plan-a' }, true, 50);
  assert.equal(api.accept(payload('request-1')), false);
  const result = payload('request-2');
  assert.equal(api.accept(result), true);
  assert.equal(await second, result);
  assert.equal(clock.pending(), 0);
});
