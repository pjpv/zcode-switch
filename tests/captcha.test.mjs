import test from "node:test";
import assert from "node:assert/strict";
import { harness, turn } from "./ui-harness.mjs";

test("config failure offers retry and reports setup failure, never an interactive challenge or claim", async () => {
  const calls = [], events = [];
  let reloaded = 0;
  const h = harness("captcha.js", async (cmd) => {
    calls.push(cmd);
    if (cmd === "get_state") return {};
    throw new Error("connection timed out");
  }, { emit: async (event, payload) => events.push({ event, payload }), location: { search: "?claimId=session-a", reload() { reloaded++; } } });
  await turn();
  assert.equal(h.nodes.get("cap-retry").hidden, false);
  assert.deepEqual(calls, ["get_state", "claim_captcha_config"]);
  assert.equal(events[0].event, "captcha://failed");
  assert.equal(events[0].payload.claimId, "session-a");
  h.nodes.get("cap-retry").click();
  assert.equal(reloaded, 1);
});

test("SDK sees configuration on load; duplicate success submits only once with the same claim ID", async () => {
  let h, options;
  const calls = [];
  const fakeWindow = {};
  h = harness("captcha.js", async (cmd, args) => {
    calls.push({ cmd, args });
    return cmd === "get_state" ? {} : { enabled: true, scene_id: "scene", region: "sgp", prefix: "prefix" };
  }, { window: fakeWindow });
  h.context.document.head.appendChild = (script) => {
    assert.equal(fakeWindow.AliyunCaptchaConfig.region, "sgp");
    fakeWindow.initAliyunCaptcha = (value) => { options = value; value.getInstance({ startTracelessVerification() {} }); };
    script.onload();
  };
  await turn();
  options.success("verified-param"); options.success("duplicate");
  await turn();
  const submissions = calls.filter(({ cmd }) => cmd === "claim_captcha_submit");
  assert.equal(submissions.length, 1);
  assert.equal(submissions[0].args.claimId, "test-claim");
  assert.equal(h.timers.size, 0);
});

test("SDK load hang terminates with a retry instead of hanging the batch", async () => {
  const events = [];
  const h = harness("captcha.js", async (cmd) => cmd === "get_state" ? {} : { enabled: true, scene_id: "s" },
    { emit: async (name) => events.push(name) });
  await turn();
  const timer = [...h.timers.values()].find(({ ms }) => ms === 12000);
  assert.ok(timer);
  timer.fn();
  await turn();
  assert.equal(h.nodes.get("cap-retry").hidden, false);
  assert.ok(events.includes("captcha://failed"));
});

test("initialization timeout ignores late SDK callbacks", async () => {
  let options;
  const calls = [], events = [];
  const h = harness("captcha.js", async (cmd) => {
    calls.push(cmd);
    return cmd === "get_state" ? {} : { enabled: true, scene_id: "s" };
  }, { window: { initAliyunCaptcha(value) { options = value; } }, emit: async (name) => events.push(name) });
  await turn();
  [...h.timers.values()].find(({ ms }) => ms === 12000).fn();
  options.success("late-result");
  assert.equal(calls.includes("claim_captcha_submit"), false);
  assert.equal(h.nodes.get("cap-retry").hidden, false);
  assert.deepEqual(events, ["captcha://failed"]);
});
