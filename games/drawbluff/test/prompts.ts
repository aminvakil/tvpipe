/**
 * The prompts (`data/prompts.txt`, `data/prompts.de.txt`, by structure:
 * `[tag]` sections) for the tests in Node; the host page fetches the
 * same files.
 */

import { readFileSync } from "node:fs";

import { parsePrompts, type Lang, type Prompt } from "../game/rules.ts";

/** A language's prompts file. */
export const PROMPT_FILE: Record<Lang, URL> = {
  en: new URL("../data/prompts.txt", import.meta.url),
  de: new URL("../data/prompts.de.txt", import.meta.url),
};

/** The prompts per language. */
export const PROMPTS_BY_LANG: Record<Lang, readonly Prompt[]> = {
  en: parsePrompts(readFileSync(PROMPT_FILE.en, "utf8")),
  de: parsePrompts(readFileSync(PROMPT_FILE.de, "utf8")),
};

/** A language's adult prompts file (the room's "adult" setting). */
export const ADULT_FILE: Record<Lang, URL> = {
  en: new URL("../data/prompts.adult.txt", import.meta.url),
  de: new URL("../data/prompts.adult.de.txt", import.meta.url),
};

/** The adult prompts per language. */
export const ADULT_BY_LANG: Record<Lang, readonly Prompt[]> = {
  en: parsePrompts(readFileSync(ADULT_FILE.en, "utf8")),
  de: parsePrompts(readFileSync(ADULT_FILE.de, "utf8")),
};

/** The English prompts. */
export const PROMPT_LIST: readonly Prompt[] = PROMPTS_BY_LANG.en;

/** The English prompts' texts. */
export const PROMPTS: readonly string[] = PROMPT_LIST.map((p) => p.text);
