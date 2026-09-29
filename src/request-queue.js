// One shared budget for quota, preview and eligibility requests. Duplicate clicks
// share the pending promise; a rejected request releases its slot as usual.
export function createRequestQueue(limit = 3) {
  const pending = new Map();
  const queue = [];
  let active = 0;
  function drain() {
    while (active < limit && queue.length) {
      const { key, task, resolve, reject } = queue.shift();
      active++;
      const finish = (settle, value) => {
        active--;
        pending.delete(key);
        settle(value);
        drain();
      };
      Promise.resolve().then(task).then((value) => finish(resolve, value), (error) => finish(reject, error));
    }
  }
  return (key, task, priority = false) => {
    if (pending.has(key)) {
      if (priority) {
        const index = queue.findIndex((job) => job.key === key);
        if (index >= 0) { queue[index].priority = true; queue.sort((a, b) => Number(b.priority) - Number(a.priority)); }
      }
      return pending.get(key);
    }
    const promise = new Promise((resolve, reject) => {
      queue.push({ key, task, resolve, reject, priority });
      queue.sort((a, b) => Number(b.priority) - Number(a.priority));
    });
    pending.set(key, promise);
    drain();
    return promise;
  };
}
