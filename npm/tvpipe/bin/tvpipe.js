#!/usr/bin/env node
// `npx tvpipe …`: runs the platform binary with this process's stdio.
import { spawn } from "node:child_process";

import { resolveBinary } from "../dist/binary.js";

let binary;
try {
  binary = resolveBinary();
} catch (err) {
  console.error(`tvpipe: ${err.message}`);
  process.exit(1);
}
const child = spawn(binary, process.argv.slice(2), { stdio: "inherit" });
// Ctrl-C reaches tvpipe too (same process group / console); it tears the
// receivers down and exits, then so do we.
for (const sig of ["SIGINT", "SIGTERM", "SIGHUP"]) {
  process.on(sig, () => {
    if (sig !== "SIGINT") child.kill(sig);
  });
}
child.on("error", (err) => {
  console.error(`tvpipe: cannot run ${binary}: ${err.message}`);
  process.exit(1);
});
child.on("exit", (code, signal) => {
  if (signal) process.kill(process.pid, signal);
  else process.exit(code ?? 1);
});
