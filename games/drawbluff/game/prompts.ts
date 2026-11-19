/**
 * Prompts to draw (written for drawbluff; also the source of decoy lies):
 * `data/prompts.txt`, one per line, shared with the Rust version.
 */

import { readFileSync } from "node:fs";

export const PROMPTS: readonly string[] = readFileSync(new URL("../data/prompts.txt", import.meta.url), "utf8")
  .split("\n")
  .map((line) => line.trim())
  .filter((line) => line !== "" && !line.startsWith("#"));
