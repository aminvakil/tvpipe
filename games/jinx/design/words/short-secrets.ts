// Short secret words vs. accidental containment (Node >= 23, no deps):
//
//   node short-secrets.ts  -> short-secrets.tsv, plausible-rejected.tsv
//
// For every secret word of 3-5 letters, runs the game's matcher
// (../matching/match.ts `invalidClue`, with a dictionary) over common words
// (CYP-LEX Zipf >= 3, known to SCOWL size <= 60) and lists the clues it would
// reject because they contain the secret ("ant" in "important", "ice" in
// "police"). Plurals, verb forms and word family are left out: those are
// meant. The list is for a human to judge which hits are accidents; the
// decisions are in README.md (3b).
import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { checkSubmit, clean, dictionary, invalidClue, type Clue } from "../matching/match.ts";

const here = dirname(fileURLToPath(import.meta.url));

const dict = new Set<string>();
{
  const dir = join(here, "raw", "scowl-2020.12.07", "final");
  for (const kind of ["english", "american", "british"].map((v) => `${v}-words`))
    for (const size of [10, 20, 35, 40, 50, 55, 60]) {
      const p = join(dir, `${kind}.${size}`);
      if (existsSync(p)) for (const l of readFileSync(p, "latin1").split("\n")) if (/^[a-z]+$/.test(l)) dict.add(l);
    }
}
const zipf = new Map<string, number>();
for (const l of readFileSync(join(here, "raw", "CYPLEX_all_age_bands.csv"), "utf8").split("\n").slice(1)) {
  const f = l.split(",");
  const w = (f[0] ?? "").toLowerCase();
  const z = Number(f[2]);
  if (/^[a-z]+$/.test(w) && z >= 3 && dict.has(w)) zipf.set(w, Math.max(z, zipf.get(w) ?? 0));
}
const secrets = readFileSync(join(here, "words.txt"), "utf8")
  .split("\n")
  .filter((l) => l && !l.startsWith("#"))
  .map((l) => l.split("\t")[0]!)
  .filter((w) => w.length <= 5);
secrets.sort();
for (const s of secrets) dict.add(s);

const clue = (w: string): Clue => {
  const r = clean(w);
  if (!r.ok) throw new Error(w);
  return r.clue;
};
const opt = { known: (w: string) => dict.has(w) };
const rows = ["secret\thits\tweight_per_million\tclues (zipf)"];
const summary: [string, number, number, string][] = [];
for (const s of secrets) {
  const sc = clue(s);
  const hits: [string, number][] = [];
  for (const [w, z] of zipf) {
    if (w === s) continue;
    const r = invalidClue(clue(w), sc, opt);
    if (r === "contains the word") hits.push([w, z]);
  }
  hits.sort((a, b) => b[1] - a[1]);
  // weight: occurrences per million of the containing words
  const weight = hits.reduce((a, [, z]) => a + 10 ** (z - 3), 0);
  const list = hits.map(([w, z]) => `${w} ${z.toFixed(1)}`).join(", ");
  summary.push([s, hits.length, weight, list]);
}
summary.sort((a, b) => b[2] - a[2]);
for (const [s, n, wt, list] of summary) if (n) rows.push(`${s}\t${n}\t${wt.toFixed(0)}\t${list}`);
writeFileSync(join(here, "short-secrets.tsv"), rows.join("\n") + "\n");
console.error(`${secrets.length} secrets of <= 5 letters, ${rows.length - 1} with containing clues`);

// ---- step 2: plausible clues ------------------------------------------------
// Rejection happens on the phone at submit, so an accidental hit only matters
// when it is a clue someone would plausibly write. plausible-clues.tsv lists
// natural clues for the flagged secrets; report every one checkSubmit rejects,
// using the game's dictionary (../matching/dict.txt) when present.
{
  const dictPath = join(here, "..", "matching", "dict.txt");
  const known = existsSync(dictPath) ? dictionary(readFileSync(dictPath, "utf8")) : opt.known;
  const lines = ["secret\tclue\treason"];
  for (const l of readFileSync(join(here, "plausible-clues.tsv"), "utf8").split("\n")) {
    if (!l || l.startsWith("#")) continue;
    const [w, list] = l.split("\t") as [string, string];
    for (const c of list.split(",").filter(Boolean)) {
      const r = checkSubmit(c, clue(w), { known });
      if (!r.ok) lines.push(`${w}\t${c}\t${"reason" in r ? r.reason : r.code}`);
    }
  }
  writeFileSync(join(here, "plausible-rejected.tsv"), lines.join("\n") + "\n");
  console.error(`plausible-rejected.tsv: ${lines.length - 1} plausible clues rejected`);
}
