// Minimal @parcel/watcher-compatible shim built on fs.watch. OpenCode uses the
// watcher for file-change hints; correctness is preserved, latency is higher.
const fs = require("fs");
const path = require("path");
const subs = new Map();

function watchDir(dir, cb) {
  try {
    const w = fs.watch(dir, { recursive: true }, (eventType, filename) => {
      if (!filename) return;
      cb(null, [{
        path: path.join(dir, filename.toString()),
        type: eventType === "rename" ? "create" : "update",
      }]);
    });
    subs.set(dir, w);
  } catch (_) {
    // fs.watch recursive is not available on every iOS build; polling is not
    // necessary for the server to function, so fail soft.
  }
}

exports.createWrapper = function createWrapper() {
  return {
    writeSnapshot() { return 0; },
    getEventsSince() { return []; },
    async subscribe(dir, cb) {
      watchDir(dir, cb);
    },
    unsubscribe(dir) {
      const w = subs.get(dir);
      if (w) { try { w.close(); } catch (_) {} subs.delete(dir); }
    },
  };
};