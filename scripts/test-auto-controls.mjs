import assert from 'node:assert/strict';
import test from 'node:test';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';

const main = readFileSync(new URL('../src/main.js', import.meta.url), 'utf8');
const predicate = main.match(/isBusy: \(\) => ([^\r\n]+),/)[1];
const start = main.indexOf('  async toggleAutoClaim() {');
const end = main.indexOf('\n};', start);
assert.ok(start >= 0 && end > start);
const method = main.slice(start, end);

test('production scheduler blocks every operation lock, including switch and toggle', () => {
  for (const lock of ['busy', 'autoToggleBusy', 'claimActive', 'claimAllRunning', 'autoClaimRunning', 'refreshClaim']) {
    const state = { busy: false, autoToggleBusy: false, claimActive: false, claimAllRunning: false, autoClaimRunning: false, refreshClaim: { running: false } };
    if (lock === 'refreshClaim') state.refreshClaim.running = true;
    else state[lock] = true;
    assert.equal(vm.runInNewContext(predicate, state), true, lock);
  }
});

function fixture() {
  let resolve, reject;
  const saved = new Promise((ok, fail) => { resolve = ok; reject = fail; });
  const enabled = [];
  const context = {
    state: { auto_claim: true }, autoToggleBusy: false, autoAbortRequested: false,
    autoClaimRunning: false, claimActive: false, claimAllRunning: false, refreshClaim: { running: false },
    autoClaimScheduler: { setEnabled: value => enabled.push(value), request() {} },
    invoke: () => saved, refresh: async () => { context.state.auto_claim = false; },
    render() {}, toast() {}, t: x => x, stripErr: String, lastAutoRound: null,
  };
  const actions = vm.runInNewContext(`({${method}})`, context);
  return { actions, context, enabled, resolve, reject };
}

test('closing auto claim stops scheduling before its persistence RPC resolves', async () => {
  const f = fixture();
  const pending = f.actions.toggleAutoClaim();
  assert.deepEqual(f.enabled, [false]);
  assert.equal(f.context.autoAbortRequested, true);
  assert.equal(f.context.autoToggleBusy, true);
  f.resolve();
  await pending;
  assert.equal(f.context.state.auto_claim, false);
  assert.equal(f.context.autoToggleBusy, false);
});

test('a failed close restores the known enabled state and releases its lock', async () => {
  const f = fixture();
  const pending = f.actions.toggleAutoClaim();
  f.reject(new Error('synthetic persistence failure'));
  await pending;
  assert.deepEqual(f.enabled, [false, true]);
  assert.equal(f.context.autoToggleBusy, false);
});
