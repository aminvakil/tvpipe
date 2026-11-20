#!/usr/bin/env node
// Assemble words.txt from the LLM curation pass plus manual overrides.
//
//   node assemble.mjs   -> words.txt, clue-notes.tsv (+ summary on stderr)
//
// Inputs: candidates.tsv (from build.mjs), llm-curation.tsv (LLM pass, see
// curation-prompt.md), overrides.tsv (manual decisions), and, for the
// compound/family notes, raw/CYPLEX_all_age_bands.csv (CC BY 4.0).
import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const tsv = (f) =>
  readFileSync(join(here, f), "utf8")
    .split("\n")
    .filter((l) => l && !l.startsWith("#"))
    .map((l) => l.split("\t"));

const cand = new Map(tsv("candidates.tsv").slice(1).map((r) => [r[0], { ease: +r[1], pos: r[2] }]));
const llm = new Map(
  tsv("llm-curation.tsv")
    .slice(1)
    .map((r) => [r[0], { keep: r[1] === "keep", q: +r[2], diff: r[3], cat: r[4], why: r[5], risk: r[6] || "" }]),
);

const final = new Map(); // word -> {cat, forced}
for (const [w, j] of llm) if (j.keep) final.set(w, { cat: j.cat });
const forced = new Map();
for (const r of tsv("overrides.tsv")) {
  const op = r[0][0];
  const w = r[0].slice(1);
  if (op === "-") {
    if (!final.delete(w)) console.error(`override: ${w} was not kept`);
  } else if (op === "+") {
    if (final.has(w)) console.error(`override: ${w} already kept`);
    final.set(w, { cat: r[1] });
  } else if (op === "=") {
    if (!final.has(w)) console.error(`override: ${w} not in list`);
    if (r[2] && r[2] !== "-") final.get(w).cat = r[2];
    if (r[1] && r[1] !== "-") forced.set(w, r[1]);
  }
}

// ---- extra columns -----------------------------------------------------------
const extras = new Map();
for (const r of tsv("extras.tsv")) {
  if (!final.has(r[0])) console.error(`extras: ${r[0]} not in list`);
  extras.set(r[0], { alsoRight: r[1] || "", banned: r[2] || "" });
}
for (const [w, e] of extras)
  for (const x of `${e.alsoRight},${e.banned}`.split(",").filter(Boolean)) {
    if (final.has(x)) console.error(`extras: ${w}: ${x} is itself a secret word`);
  }

// ---- difficulty ------------------------------------------------------------
// Combined score: 50% data (rank of the "ease" score from build.mjs: kid
// frequency, CBeebies frequency, Glasgow age of acquisition, Lancaster
// knowledge), 35% the LLM's guessability call, 15% the LLM's quality call.
// The top 35% are easy, the bottom 20% hard, the rest medium; overrides win.
const words = [...final.keys()];
const easeSorted = words.map((w) => cand.get(w)?.ease ?? -Infinity).sort((a, b) => a - b);
const pct = (w) => {
  const e = cand.get(w)?.ease;
  if (e === undefined) return 0.3;
  return easeSorted.indexOf(e) / (easeSorted.length - 1);
};
const DIFF = { easy: 1, medium: 0.5, hard: 0 };
const score = new Map(
  words.map((w) => {
    const j = llm.get(w);
    return [w, 0.5 * pct(w) + 0.35 * (DIFF[j?.diff] ?? 0.5) + 0.15 * (((j?.q ?? 3) - 1) / 4)];
  }),
);
const ranked = [...words].sort((a, b) => score.get(b) - score.get(a));
const diff = new Map();
ranked.forEach((w, i) => {
  const r = i / ranked.length;
  diff.set(w, forced.get(w) || (r < 0.35 ? "easy" : r < 0.8 ? "medium" : "hard"));
});

// ---- clue notes for the matching engine -------------------------------------
// Parts: common words (CYP-LEX Zipf >= 4, 3+ letters) that the secret word
// starts or ends with, e.g. snowman -> snow, man. Players will reach for them.
const common = new Set();
{
  const lines = readFileSync(join(here, "raw/CYPLEX_all_age_bands.csv"), "utf8").split("\n");
  for (const l of lines.slice(1)) {
    const f = l.split(",");
    if (+f[2] >= 4 && /^[a-z]{3,}$/.test(f[0])) common.add(f[0]);
  }
}
// Split points that are spelling accidents, not meaning (off+ice, kit+ten).
const ACCIDENTAL = new Set(
  "off her kin ten don kit fat ham lad dee but butt pie cure pup pet mock pump yer ice fair flow crate bat dump ling ton lap ear pine pan kin nap nick pad pin pop post cart pitch tick lips bow fold brief draw pass card nick land per hell seas tad ward were store show".split(" "),
);
const NOT_SECOND = new Set(["per", "hell", "key", "pole", "robe", "yer"]);
const parts = (w) => {
  const out = [];
  for (let i = 3; i <= w.length - 3; i++) {
    const a = w.slice(0, i);
    const b = w.slice(i);
    // keep the split unless both halves are on the accidental list or the
    // second half is a fragment that only looks like a word (sea+shell is
    // reported by the LLM note instead of "seas+hell")
    if (!common.has(a) || !common.has(b) || NOT_SECOND.has(b)) continue;
    if (ACCIDENTAL.has(a) && ACCIDENTAL.has(b)) continue;
    out.push(`${a}+${b}`);
  }
  // derivations of a common word: baker -> bake, dancer -> dance, sticker -> stick
  const deriv = [];
  for (const suf of ["er", "r", "ing", "ist", "or", "ery", "ry", "y"]) {
    if (w.endsWith(suf)) {
      const base = w.slice(0, -suf.length);
      for (const b of [base, base + "e", base.replace(/(.)\1$/, "$1")])
        if (b.length >= 4 && b !== w && (common.has(b) || final.has(b)) && !ACCIDENTAL.has(b)) deriv.push(b);
    }
  }
  return { compound: out, family: [...new Set(deriv)] };
};

// Other list words inside this one (snow in snowman, bath in bathroom): the
// game should not deal both in one evening.
const inList = new Set(words);
const related = (w, p) =>
  [...new Set([...p.compound.flatMap((c) => c.split("+")), ...p.family])].filter((o) => inList.has(o));
const notes = ["word\tcompound_parts\tfamily\tcontains_list_words\tllm_note"];
const lines = [];
const sorted = [...words].sort();
for (const w of sorted) {
  const e = extras.get(w);
  lines.push(`${w}\t${diff.get(w)}\t${final.get(w).cat}` + (e ? `\t${e.alsoRight}\t${e.banned}`.replace(/\t+$/, "") : ""));
  const p = parts(w);
  const risk = llm.get(w)?.keep ? llm.get(w).risk : "";
  const rel = related(w, p);
  if (p.compound.length || p.family.length || rel.length || risk)
    notes.push(`${w}\t${p.compound.join(" ")}\t${p.family.join(" ")}\t${rel.join(" ")}\t${risk.replace(/\t/g, " ")}`);
}
writeFileSync(
  join(here, "words.txt"),
  "# Jinx secret words (English). word<TAB>difficulty<TAB>category<TAB>also_right<TAB>banned\n" +
    "# also_right (accepted guesses) and banned (clues rejected as too close) are optional, comma-separated. See README.md.\n" + lines.join("\n") + "\n",
);
writeFileSync(join(here, "clue-notes.tsv"), notes.join("\n") + "\n");

const count = (f) => {
  const c = {};
  for (const w of words) c[f(w)] = (c[f(w)] || 0) + 1;
  return c;
};
console.error(`words.txt: ${words.length} words`);
console.error(count((w) => diff.get(w)));
console.error(count((w) => final.get(w).cat));
console.error(`clue-notes.tsv: ${notes.length - 1} words with notes`);
