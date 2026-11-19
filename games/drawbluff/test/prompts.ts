/**
 * The prompts (`data/prompts.txt`, by structure: `[tag]` sections) for
 * the tests in Node; the host page fetches the same file.
 */

import { readFileSync } from "node:fs";

import { parsePrompts, type Prompt } from "../game/rules.ts";

export const PROMPT_LIST: readonly Prompt[] = parsePrompts(readFileSync(new URL("../data/prompts.txt", import.meta.url), "utf8"));

/** The prompts' texts. */
export const PROMPTS: readonly string[] = PROMPT_LIST.map((p) => p.text);
