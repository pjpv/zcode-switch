import test from "node:test";
import assert from "node:assert/strict";
import { harness, turn } from "./ui-harness.mjs";

test("eligibility completes out of order, counts settlements and preserves failed accounts", async () => {
  const pending = new Map();
  const h = harness("main.js", (cmd, { id }) => {
    assert.equal(cmd, "claim_refresh");
    return new Promise((resolve, reject) => pending.set(id, { resolve, reject }));
  });
  h.run("state.accounts = Array.from({length: 20}, (_, i) => ({id: String(i), name: String(i)})); claimable['0'] = {plans:[{plan_id:'saved'}]};");
  const work = h.run("actions.refreshClaim()");
  await turn();
  assert.equal(pending.size, 3);
  assert.equal(h.run("refreshClaim.done"), 0);
  pending.get("1").resolve({ plans: [{ plan_id: "new" }] }); pending.delete("1");
  await turn();
  assert.equal(h.run("refreshClaim.done"), 1);
  pending.get("0").reject("connection timed out"); pending.delete("0");
  while (h.run("refreshClaim.done") < 20) {
    await turn();
    for (const [id, p] of pending) { p.resolve({ plans: [] }); pending.delete(id); }
  }
  await work;
  assert.equal(h.run("claimable['0'].plans[0].plan_id"), "saved");
  assert.match(h.run("claimStripHtml('0')"), /connection timed out/);
  assert.match(h.run("claimStripHtml('0')"), /actions.refreshClaim\('0'\)/);
  assert.equal(h.run("refreshClaim.running"), false);
  assert.match(h.toasts.at(-1)[0], /"failed":1/);
});

test("all due quotas drain in one sweep with a shared three-slot budget and no duplicate clicks", async () => {
  let active = 0, peak = 0, calls = 0;
  const pending = [];
  const h = harness("main.js", async (cmd, { id }) => {
    calls++; peak = Math.max(peak, ++active);
    await new Promise((r) => pending.push(r)); active--;
    if (id === "1") throw new Error("offline");
    return { plans: [] };
  });
  h.run("state.accounts = Array.from({length:20}, (_,i) => ({id:String(i)}));");
  const work = h.run("sweepTick()");
  const duplicate = h.run("loadAcctQuota('0')");
  await turn(); assert.equal(calls, 3);
  while (calls < 20 || active) { pending.splice(0).forEach((r) => r()); await turn(); }
  await work; await duplicate;
  assert.equal(calls, 20); assert.equal(peak, 3);
  assert.match(h.run("acctQuota['1'].err"), /offline/);
  assert.equal(h.run("Object.values(quotaDue).every(at => at > Date.now())"), true);
});

test("a preview already in flight cannot overwrite a newer eligibility result", async () => {
  let resolvePreview;
  const calls = [];
  const h = harness("main.js", (cmd) => {
    calls.push(cmd);
    if (cmd === "claim_preview") return new Promise((r) => { resolvePreview = r; });
    return Promise.resolve({ plans: [{ plan_id: "fresh" }] });
  });
  const preview = h.run("loadClaimPreview('a')"); await turn();
  const refresh = h.run("refreshEligibility('a')"); await turn();
  assert.equal(calls.length, 1);
  resolvePreview([{ plan_id: "old" }]);
  await preview; await refresh;
  assert.equal(h.run("claimable.a.plans[0].plan_id"), "fresh");
});

test("batch setup failure before claim_start returns cancels promptly and ignores stale events", async () => {
  let h, claimId;
  const calls = [];
  h = harness("main.js", async (cmd, args) => {
    calls.push(cmd);
    if (cmd === "claim_start") {
      claimId = args.claimId;
      await h.listeners.get("captcha://failed")({ payload: { claimId, message: "config timeout" } });
    }
  });
  h.run("claimAllRunning = true;");
  const result = await h.run("performClaim('a', {plan_id:'p'})");
  assert.equal(result.code, "setup");
  assert.deepEqual(calls, ["claim_start", "claim_cancel"]);
  assert.equal(h.run("claimFailures.a"), "config timeout");
  h.run("waitForClaimResult('b', 'new-session')");
  await h.listeners.get("captcha://failed")({ payload: { claimId, message: "late error" } });
  assert.equal(h.run("claimWaiter.accountId"), "b");
  assert.equal(calls.length, 2);
  h.run("claimWaiter.finish(null)");
});

test("manual setup failure keeps the claim pending for in-dialog retry", async () => {
  const h = harness("main.js", async () => {});
  const result = h.run("waitForClaimResult('a', 'manual')");
  await h.listeners.get("captcha://failed")({ payload: { claimId: "manual", message: "retry config" } });
  assert.equal(h.run("claimWaiter.claimId"), "manual");
  h.listeners.get("claim://cancelled")({ payload: { claimId: "manual", accountId: "a" } });
  assert.equal((await result).code, "cancelled");
});

test("individual failed-account retry works during the bulk cooldown and only refreshes that account", async () => {
  const calls = [];
  const h = harness("main.js", async (_, { id }) => { calls.push(id); return { plans: [] }; });
  h.run("state.accounts = [{id:'a'}, {id:'b'}]; refreshClaim.cooldownUntil = Date.now() + 60000;");
  await h.run("actions.refreshClaim('b')");
  assert.deepEqual(calls, ["b"]);
  await h.run("actions.refreshClaim()");
  assert.deepEqual(calls, ["b"]);
});

test("terminal submission error finishes a manual attempt so the account can be retried", async () => {
  const calls = [];
  const h = harness("main.js", async (cmd) => calls.push(cmd));
  const result = h.run("waitForClaimResult('a', 'manual')");
  await h.listeners.get("captcha://failed")({ payload: { claimId: "manual", message: "submit failed", terminal: true } });
  assert.equal((await result).code, "setup");
  assert.deepEqual(calls, ["claim_cancel"]);
  assert.equal(h.run("claimFailures.a"), "submit failed");
});
