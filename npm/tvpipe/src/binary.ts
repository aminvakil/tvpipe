/**
 * Finds the tvpipe binary: the `binary` option, `$TVPIPE_BINARY`, or the
 * platform package (`@tvpipe/<os>-<cpu>`) npm installed next to this one.
 */

import { accessSync, chmodSync, constants, existsSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname, join } from "node:path";

import { TvpipeError } from "./errors.ts";

/** The platforms with a published binary: `${process.platform}-${process.arch}`. */
export const PLATFORMS = [
  "darwin-arm64",
  "darwin-x64",
  "linux-arm64",
  "linux-x64",
  "win32-arm64",
  "win32-x64",
] as const;

export type Platform = (typeof PLATFORMS)[number];

/** The npm package with the binary for a platform. */
export function platformPackage(platform: string = `${process.platform}-${process.arch}`): string {
  return `@tvpipe/${platform}`;
}

export interface BinaryOptions {
  /** Path of a tvpipe binary to use instead of the installed one. */
  binary?: string | undefined;
}

/** The environment variable that names a binary to use. */
export const BINARY_ENV = "TVPIPE_BINARY";

/**
 * The tvpipe binary to run. Throws a {@link TvpipeError} (`binary-missing`
 * or `unsupported-platform`) that says how to fix it.
 */
export function resolveBinary(options: BinaryOptions = {}): string {
  const explicit = options.binary ?? process.env[BINARY_ENV];
  if (explicit) {
    if (!existsSync(explicit)) {
      throw new TvpipeError(
        "binary-missing",
        `tvpipe binary not found at ${explicit} (from ${options.binary ? "the binary option" : `$${BINARY_ENV}`})`,
      );
    }
    return explicit;
  }
  const platform = `${process.platform}-${process.arch}`;
  if (!(PLATFORMS as readonly string[]).includes(platform)) {
    throw new TvpipeError(
      "unsupported-platform",
      `tvpipe has no binary for ${platform} (supported: ${PLATFORMS.join(", ")}); ` +
        `build it from https://github.com/mitsuhiko/tvpipe and set $${BINARY_ENV} to its path`,
    );
  }
  const pkg = platformPackage(platform);
  const exe = process.platform === "win32" ? "tvpipe.exe" : "tvpipe";
  let path: string;
  try {
    const require = createRequire(import.meta.url);
    path = join(dirname(require.resolve(`${pkg}/package.json`)), "bin", exe);
  } catch (cause) {
    throw new TvpipeError(
      "binary-missing",
      `the tvpipe binary for ${platform} is not installed: npm did not install the optional ` +
        `dependency ${pkg} (installed with --omit=optional or --no-optional, or the lockfile ` +
        `was made on another platform?). Reinstall tvpipe with optional dependencies, ` +
        `install ${pkg} explicitly, or set $${BINARY_ENV} to a tvpipe binary`,
      { cause },
    );
  }
  if (!existsSync(path)) {
    throw new TvpipeError("binary-missing", `${pkg} is installed but has no ${join("bin", exe)}`);
  }
  if (process.platform !== "win32") {
    try {
      accessSync(path, constants.X_OK);
    } catch {
      // Some package managers drop the executable bit.
      try {
        chmodSync(path, 0o755);
      } catch {
        // Spawning reports the problem.
      }
    }
  }
  return path;
}
