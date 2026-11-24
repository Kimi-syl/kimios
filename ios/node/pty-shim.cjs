// OpenCode imports node-pty at startup. iOS has no PTY and no process spawn,
// so the native module cannot load. The server path never calls spawn; only the
// terminal tool does, and that tool reports the sandbox limitation.
exports.spawn = function spawn() {
  throw new Error("PTY unavailable in the iOS app sandbox");
};