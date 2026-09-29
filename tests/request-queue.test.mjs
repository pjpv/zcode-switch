import test from "node:test";
import assert from "node:assert/strict";
import { createRequestQueue } from "../src/request-queue.js";

const turn = () => new Promise(setImmediate);

test("twenty requests use at most three slots and a failure does not stall the queue", async () => {
  const request = createRequestQueue(3);
  let active = 0, peak = 0;
  const started = [], releases = [];
  const jobs = Array.from({ length: 20 }, (_, id) => request(String(id), async () => {
    started.push(id);
    peak = Math.max(peak, ++active);
    await new Promise((resolve) => releases.push(resolve));
    active--;
    if (id === 1) throw new Error("offline");
    return id;
  }));
  const all = Promise.allSettled(jobs);
  await turn();
  assert.equal(started.length, 3);
  while (started.length < 20 || active) {
    releases.splice(0).forEach((release) => release());
    await turn();
  }
  const results = await all;
  assert.equal(peak, 3);
  assert.equal(results.filter((r) => r.status === "fulfilled").length, 19);
  assert.equal(results[1].reason.message, "offline");
});

test("pending duplicates share work, but a completed or failed key can be retried", async () => {
  const request = createRequestQueue(3);
  let calls = 0, release;
  const first = request("account", async () => { calls++; await new Promise((r) => { release = r; }); return 1; });
  const second = request("account", () => { throw new Error("duplicate"); });
  assert.equal(first, second);
  await turn(); release();
  assert.equal(await first, 1);
  assert.equal(await request("account", () => ++calls), 2);
  await assert.rejects(request("account", () => { throw new Error("failed"); }));
  assert.equal(await request("account", () => 3), 3);
});

test("interactive work and promoted duplicate clicks pass queued background work", async () => {
  const request = createRequestQueue(1), started = [];
  let release;
  const running = request("running", () => new Promise((r) => { release = r; }));
  const background = request("background", () => started.push("background"));
  const promoted = request("promoted", () => started.push("promoted"));
  request("promoted", () => assert.fail("duplicate work"), true);
  const interactive = request("interactive", () => started.push("interactive"), true);
  await turn(); release();
  await Promise.all([running, background, promoted, interactive]);
  assert.deepEqual(started, ["promoted", "interactive", "background"]);
});
