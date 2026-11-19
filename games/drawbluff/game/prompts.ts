/**
 * Prompts to draw (written for drawbluff; also the source of decoy lies):
 * `data/prompts.txt`, by structure (`[tag]` sections), shared with the
 * Rust version.
 */

import { readFileSync } from "node:fs";

import { parsePrompts, type Prompt } from "./rules.ts";

export const PROMPT_LIST: readonly Prompt[] = parsePrompts(readFileSync(new URL("../data/prompts.txt", import.meta.url), "utf8"));

/** The prompts' texts. */
export const PROMPTS: readonly string[] = PROMPT_LIST.map((p) => p.text);
