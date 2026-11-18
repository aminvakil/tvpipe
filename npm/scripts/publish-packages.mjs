#!/usr/bin/env node
// Publishes the packages staged by build-packages.mjs, in order (the
// platform packages first, `tvpipe` last). Versions already on the
// registry are skipped, so a rerun after a partial failure completes.
//
//   node npm/scripts/publish-packages.mjs [--out DIR] [--dry-run] [--provenance] [--tag TAG]
//
// --dry-run: `npm publish --dry-run` (shows the tarballs, publishes nothing).
// --tag: the dist-tag (default: `latest`, or `next` for prereleases).
// Authentication is npm's: trusted publishing (OIDC) in GitHub Actions,
// else the user's npm login.

import { execFileSync, spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { parseArgs } from "node:util";

const repo = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..");
const { values } = parseArgs({
  options: {
    out: { type: "string" },
    "dry-run": { type: "boolean" },
    provenance: { type: "boolean" },
    tag: { type: "string" },
  },
});
const out = resolve(values.out ?? join(repo, "npm", "dist"));
const packages = JSON.parse(readFileSync(join(out, "packages.json"), "utf8"));
const npm = process.platform === "win32" ? "npm.cmd" : "npm";

function published(name, version) {
  const r = spawnSync(npm, ["view", `${name}@${version}`, "version"], { encoding: "utf8", shell: process.platform === "win32" });
  return r.status === 0 && r.stdout.trim() === version;
}

for (const { name, dir } of packages) {
  const { version } = JSON.parse(readFileSync(join(dir, "package.json"), "utf8"));
  const tag = values.tag ?? (version.includes("-") ? "next" : "latest");
  if (!values["dry-run"] && published(name, version)) {
    console.log(`${name}@${version}: already published, skipped`);
    continue;
  }
  const args = ["publish", "--access", "public", "--tag", tag];
  if (values["dry-run"]) args.push("--dry-run");
  if (values.provenance) args.push("--provenance");
  console.log(`${name}@${version}: npm ${args.join(" ")}`);
  execFileSync(npm, args, { cwd: dir, stdio: "inherit", shell: process.platform === "win32" });
}
