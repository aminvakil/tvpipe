#!/usr/bin/env node
// Stages the npm packages: `@tvpipe/sdk` (the SDK) and `@tvpipe/<os>-<cpu>`
// (one binary each), all at the crate's version.
//
//   node npm/scripts/build-packages.mjs --artifacts DIR   # release archives (CI)
//   node npm/scripts/build-packages.mjs --local PATH      # this platform's binary only
//
// Options: --version X (default: Cargo.toml's), --out DIR (default npm/dist).
// The SDK must be built first (`npm run build` in npm/tvpipe). Writes
// OUT/packages.json: the package directories in publish order (the
// platform packages first, `@tvpipe/sdk` last).

import { execFileSync } from "node:child_process";
import { chmodSync, cpSync, existsSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { parseArgs } from "node:util";

const repo = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..");
const sdk = join(repo, "npm", "tvpipe");

/** Rust target → npm platform (`process.platform-process.arch`). */
export const TARGETS = {
  "aarch64-apple-darwin": "darwin-arm64",
  "x86_64-apple-darwin": "darwin-x64",
  "aarch64-unknown-linux-musl": "linux-arm64",
  "x86_64-unknown-linux-musl": "linux-x64",
  "aarch64-pc-windows-msvc": "win32-arm64",
  "x86_64-pc-windows-msvc": "win32-x64",
};

const { values } = parseArgs({
  options: {
    version: { type: "string" },
    artifacts: { type: "string" },
    local: { type: "string" },
    out: { type: "string" },
  },
});
const version = values.version ?? cargoVersion();
const out = resolve(values.out ?? join(repo, "npm", "dist"));
if (!values.artifacts && !values.local) fail("pass --artifacts DIR (release archives) or --local PATH (a binary)");

function fail(msg) {
  console.error(`build-packages: ${msg}`);
  process.exit(1);
}

function cargoVersion() {
  const m = readFileSync(join(repo, "Cargo.toml"), "utf8").match(/^version = "([^"]+)"/m);
  if (!m) fail("no version in Cargo.toml");
  return m[1];
}

const repository = { type: "git", url: "git+https://github.com/mitsuhiko/tvpipe.git" };
const json = (path, v) => writeFileSync(path, `${JSON.stringify(v, null, 2)}\n`);

rmSync(out, { recursive: true, force: true });
mkdirSync(out, { recursive: true });

// The platform packages.
const platforms = [];
function platformPackage(platform, binary) {
  const [os, cpu] = platform.split("-");
  const name = `@tvpipe/${platform}`;
  const dir = join(out, `tvpipe-${platform}`);
  const exe = os === "win32" ? "tvpipe.exe" : "tvpipe";
  mkdirSync(join(dir, "bin"), { recursive: true });
  cpSync(binary, join(dir, "bin", exe));
  if (os !== "win32") chmodSync(join(dir, "bin", exe), 0o755);
  cpSync(join(repo, "LICENSE"), join(dir, "LICENSE"));
  cpSync(join(repo, "LICENSES"), join(dir, "LICENSES"), { recursive: true });
  json(join(dir, "package.json"), {
    name,
    version,
    description: `The tvpipe binary for ${platform} (used by @tvpipe/sdk)`,
    homepage: "https://tvpipe.party/",
    repository,
    // The binary includes the LGPL FairPlay module (see LICENSES/).
    license: "Apache-2.0 AND LGPL-3.0-or-later",
    os: [os],
    cpu: [cpu],
    files: ["bin", "LICENSE", "LICENSES"],
    preferUnplugged: true,
  });
  writeFileSync(
    join(dir, "README.md"),
    `# ${name}\n\nThe [tvpipe](https://github.com/mitsuhiko/tvpipe) binary for ${platform}. ` +
      `Install [\`@tvpipe/sdk\`](https://www.npmjs.com/package/@tvpipe/sdk) instead; it pulls in this ` +
      `package on ${platform}.\n`,
  );
  platforms.push({ name, dir, platform });
}

if (values.local) {
  platformPackage(`${process.platform}-${process.arch}`, resolve(values.local));
} else {
  const artifacts = resolve(values.artifacts);
  const files = readdirSync(artifacts);
  for (const [target, platform] of Object.entries(TARGETS)) {
    const base = `tvpipe-${version}-${target}`;
    const archive = files.find((f) => f === `${base}.tar.gz` || f === `${base}.zip`);
    if (!archive) fail(`${base}.tar.gz/.zip missing in ${artifacts}`);
    const tmp = mkdtempSync(join(tmpdir(), "tvpipe-npm-"));
    if (archive.endsWith(".zip")) execFileSync("unzip", ["-q", join(artifacts, archive), "-d", tmp]);
    else execFileSync("tar", ["-xzf", join(artifacts, archive), "-C", tmp]);
    const exe = target.includes("windows") ? "tvpipe.exe" : "tvpipe";
    const binary = join(tmp, base, exe);
    if (!existsSync(binary)) fail(`${archive} has no ${base}/${exe}`);
    platformPackage(platform, binary);
    rmSync(tmp, { recursive: true, force: true });
  }
}

// The SDK.
if (!existsSync(join(sdk, "dist", "index.js"))) fail("npm/tvpipe/dist is missing: run `npm run build` in npm/tvpipe");
const main = join(out, "sdk");
mkdirSync(main);
for (const f of ["dist", "src", "bin", "README.md"]) cpSync(join(sdk, f), join(main, f), { recursive: true });
cpSync(join(repo, "LICENSE"), join(main, "LICENSE"));
const pkg = JSON.parse(readFileSync(join(sdk, "package.json"), "utf8"));
pkg.version = version;
delete pkg.scripts;
delete pkg.devDependencies;
// Every platform package, pinned: npm installs the one matching os/cpu.
pkg.optionalDependencies = Object.fromEntries(Object.values(TARGETS).map((p) => [`@tvpipe/${p}`, version]));
json(join(main, "package.json"), pkg);

const order = [...platforms.map((p) => ({ name: p.name, dir: p.dir })), { name: pkg.name, dir: main }];
json(join(out, "packages.json"), order);
for (const p of order) console.log(`${p.name}@${version}  ${p.dir}`);
