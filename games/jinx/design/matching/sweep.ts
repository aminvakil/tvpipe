// Sweep the matcher over real data (Node only, not part of the game):
//
//   node sweep.ts            -> sweep-report.md, sweep-invalid.tsv, sweep-typos.tsv
//
// Inputs (from the word-list workstream, not committed): ../words/words.txt,
// ../words/raw/CYPLEX_all_age_bands.csv (word frequencies + lemmas) and
// ../words/raw/scowl-2020.12.07/final (SCOWL word lists, as an optional dictionary).
import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import {
  checkSubmit,
  clean,
  dictionary,
  forms,
  invalidClue,
  judgeGuess,
  parseWordList,
  sameClue,
  spellNorm,
  type Clue,
  type MatchOptions,
  type WordEntry,
} from "./match.ts";

const here = dirname(fileURLToPath(import.meta.url));
const words = join(here, "..", "words");
const out: string[] = [];
const say = (s = "") => out.push(s);

// ---- data -------------------------------------------------------------------

interface Word {
  w: string;
  zipf: number;
  lemma: string;
}
const lex = new Map<string, Word>();
{
  const lines = readFileSync(join(words, "raw", "CYPLEX_all_age_bands.csv"), "utf8").replace(/^\uFEFF/, "").split(/\r?\n/);
  for (const l of lines.slice(1)) {
    const f = l.split(",");
    const w = f[0]?.toLowerCase() ?? "";
    const zipf = Number(f[2]);
    const lemma = (f[3] ?? "").toLowerCase();
    if (!/^[a-z]+$/.test(w) || !(zipf >= 3)) continue;
    const old = lex.get(w);
    if (!old || old.zipf < zipf) lex.set(w, { w, zipf, lemma: /^[a-z]+$/.test(lemma) ? lemma : w });
  }
}
const common = [...lex.values()].sort((a, b) => b.zipf - a.zipf);

// The shipped dictionary: run `node build-dict.ts` first.
if (!existsSync(join(here, "dict.txt"))) {
  console.error("missing dict.txt: run node build-dict.ts");
  process.exit(1);
}
const dictText = readFileSync(join(here, "dict.txt"), "utf8");
const known = dictionary(dictText);
const dictSize = dictText.split("\n").filter(Boolean).length;

const entries: WordEntry[] = existsSync(join(words, "words.txt")) ? parseWordList(readFileSync(join(words, "words.txt"), "utf8")) : [];
const entryOf = new Map(entries.map((e) => [e.word, e]));
const secrets = entries.map((e) => e.word);

const C = new Map<string, Clue>();
const cl = (w: string): Clue => {
  let c = C.get(w);
  if (!c) {
    const r = clean(w);
    if (!r.ok) throw new Error(w);
    c = r.clue;
    C.set(w, c);
  }
  return c;
};
const z = (w: string) => lex.get(w)?.zipf.toFixed(1) ?? "-";
const lemmaOf = (w: string) => lex.get(w)?.lemma ?? w;

say("# Matching sweep");
say();
say(`Data: ${common.length} CYP-LEX words with Zipf >= 3 (lemmas from CYP-LEX), ${dictSize} words of dict.txt (build-dict.ts) as the optional dictionary, ${secrets.length} secret words from words.txt.`);
say();

// ---- A. inflection recall -------------------------------------------------------

{
  const misses: Word[] = [];
  let total = 0;
  for (const x of common) {
    if (x.lemma === x.w || !lex.has(x.lemma)) continue;
    total++;
    if (!sameClue(cl(x.w), cl(x.lemma))) misses.push(x);
  }
  say("## A. Inflected word vs its CYP-LEX lemma (should jinx)");
  say();
  say(`${total - misses.length} of ${total} match (${((100 * (total - misses.length)) / total).toFixed(1)} %). Misses, most frequent first:`);
  say();
  say(misses.slice(0, 120).map((m) => `${m.w}/${m.lemma}`).join(", "));
  say();
}

// ---- B. different lemmas that the matcher joins ------------------------------------

const famIndex = new Map<string, string[]>();
for (const x of common) {
  for (const f of forms(x.w).fam) {
    const b = famIndex.get(f) ?? [];
    b.push(x.w);
    famIndex.set(f, b);
  }
}

{
  const seen = new Set<string>();
  const byReason = new Map<string, [string, string, number][]>();
  for (const bucket of famIndex.values()) {
    if (bucket.length > 200) continue;
    for (let i = 0; i < bucket.length; i++) {
      for (let j = i + 1; j < bucket.length; j++) {
        const a = bucket[i]!;
        const b = bucket[j]!;
        const k = a < b ? a + " " + b : b + " " + a;
        if (seen.has(k)) continue;
        seen.add(k);
        if (lemmaOf(a) === lemmaOf(b) || lemmaOf(a) === b || lemmaOf(b) === a) continue;
        const r = sameClue(cl(a), cl(b));
        if (!r) continue;
        const list = byReason.get(r) ?? [];
        list.push([a, b, Math.min(lex.get(a)!.zipf, lex.get(b)!.zipf)]);
        byReason.set(r, list);
      }
    }
  }
  say("## B. Common words with different lemmas that jinx (review: most are intended families)");
  say();
  for (const [r, list] of byReason) {
    list.sort((x, y) => y[2] - x[2]);
    say(`### ${r}: ${list.length} pairs`);
    say();
    say(list.slice(0, r === "same family" ? 400 : 150).map(([a, b]) => `${a}/${b}`).join(", "));
    say();
  }
}

// ---- C. typos between real words ------------------------------------------------------

function histDist(a: string, b: string): number {
  const h = new Array<number>(26).fill(0);
  for (const c of a) h[c.charCodeAt(0) - 97]!++;
  for (const c of b) h[c.charCodeAt(0) - 97]!--;
  return h.reduce((s, v) => s + Math.abs(v), 0);
}

{
  const pool = common.filter((x) => x.w.length >= 5 && x.zipf >= 3.0);
  const byFirst = new Map<string, Word[]>();
  for (const x of pool) {
    const b = byFirst.get(x.w[0]!) ?? [];
    b.push(x);
    byFirst.set(x.w[0]!, b);
  }
  const pairs: [string, string, number][] = [];
  let withDict = 0;
  const dictPairs: string[] = [];
  for (const bucket of byFirst.values()) {
    for (let i = 0; i < bucket.length; i++) {
      for (let j = i + 1; j < bucket.length; j++) {
        const a = bucket[i]!.w;
        const b = bucket[j]!.w;
        if (Math.abs(a.length - b.length) > 2 || histDist(a, b) > 6) continue;
        if (sameClue(cl(a), cl(b)) === "typo") pairs.push([a, b, Math.min(bucket[i]!.zipf, bucket[j]!.zipf)]);
        if (sameClue(cl(a), cl(b), { known }) === "typo") {
          withDict++;
          if (dictPairs.length < 80) dictPairs.push(`${a}/${b}`);
        }
      }
    }
  }
  pairs.sort((x, y) => y[2] - x[2]);
  writeFileSync(join(here, "sweep-typos.tsv"), "a\tb\tmin_zipf\n" + pairs.map((p) => p.join("\t")).join("\n") + "\n");
  say("## C. Real-word pairs judged \"typo\" between clues, without a dictionary");
  say();
  say(`${pairs.length} pairs among ${pool.length} words of 5+ letters with Zipf >= 3 (same first letter only). With the dictionary option all of them are blocked (both words are known). Most frequent:`);
  say();
  say(pairs.slice(0, 150).map(([a, b]) => `${a}/${b}`).join(", "));
  say();
  say(`With the dictionary: ${withDict} pairs (one of the two is not in it): ${dictPairs.join(", ")}`);
  say();
}

// ---- D. secret words -------------------------------------------------------------------------

for (const [mode, opt] of [["without dictionary", {}], ["with dictionary", { known }]] as [string, MatchOptions][]) {
  if (secrets.length === 0) break;
  const rows: string[] = ["secret\tclue\treason\tclue_zipf\tclue_lemma\tsuspicious"];
  const ownMiss: string[] = [];
  const suspicious = new Map<string, string[]>();
  const counts = new Map<string, number>();
  const selfFail: string[] = [];
  for (const s of secrets) {
    const sc = clean(s);
    if (!sc.ok) {
      selfFail.push(`${s} (${sc.error})`);
      continue;
    }
    const secret = sc.clue;
    if (invalidClue(secret, secret) !== "same word") selfFail.push(s);
    const sf = forms(secret.key);
    const cand = new Set<string>();
    for (const f of sf.fam) for (const w of famIndex.get(f) ?? []) cand.add(w);
    const sk = secret.key;
    const sp = spellNorm(sk);
    for (const x of common) {
      const w = x.w;
      if (w.includes(sk) || sk.includes(w) || w.includes(sp)) cand.add(w);
      else if (Math.abs(w.length - sk.length) <= 3 && histDist(w, sk) <= 6) cand.add(w);
    }
    for (const x of common) if (x.lemma === sk) cand.add(x.w);
    for (const w of cand) {
      if (w === sk) continue;
      const e = entryOf.get(s);
      const r = invalidClue(cl(w), secret, opt, [...(e?.banned ?? []), ...(e?.alsoRight ?? [])]);
      const own = lemmaOf(w) === sk;
      if (own && !r) ownMiss.push(`${w}/${sk}`);
      if (!r) continue;
      counts.set(r, (counts.get(r) ?? 0) + 1);
      // Suspicious: not the secret's own inflection, and the reason is a guess about meaning.
      const sus = !own && ["typo", "same family", "sounds the same", "part of the word", "contains the word", "plural", "verb form"].includes(r);
      rows.push([sk, w, r, z(w), lemmaOf(w), sus ? "?" : ""].join("\t"));
      if (sus && (lex.get(w)?.zipf ?? 0) >= 3.5) {
        const list = suspicious.get(r) ?? [];
        list.push(`${sk}: ${w}`);
        suspicious.set(r, list);
      }
    }
  }
  writeFileSync(join(here, mode === "with dictionary" ? "sweep-invalid-dict.tsv" : "sweep-invalid.tsv"), rows.join("\n") + "\n");
  say(`## D. Secret words (words.txt) vs common words, ${mode}`);
  say();
  say(`Every secret word cleans and is invalid against itself: ${selfFail.length === 0 ? "yes" : "NO: " + selfFail.join(", ")}.`);
  say();
  say(`Invalid clues found, by reason: ${[...counts].map(([r, n]) => `${r} ${n}`).join(", ")}. Full list: sweep-invalid${mode === "with dictionary" ? "-dict" : ""}.tsv.`);
  say();
  say(`The secret's own inflections (CYP-LEX lemma = secret) that are NOT invalid: ${ownMiss.length === 0 ? "none" : ownMiss.join(", ")}.`);
  say();
  say("Struck clues whose lemma is not the secret, clue Zipf >= 3.5 (review these):");
  say();
  for (const [r, list] of suspicious) {
    say(`### ${r}: ${list.length}`);
    say();
    say(list.join(", "));
    say();
  }
}

// ---- E. word-list columns ----------------------------------------------------------------------

{
  const problems: string[] = [];
  let right = 0;
  let banned = 0;
  for (const e of entries) {
    const sc = clean(e.word);
    if (!sc.ok) continue;
    for (const a of e.alsoRight) {
      right++;
      const g = clean(a);
      if (!g.ok) problems.push(`${e.word}: also_right "${a}" does not clean (${g.error})`);
      else if (!judgeGuess(g.clue, sc.clue, { known }, e.alsoRight)) problems.push(`${e.word}: also_right "${a}" is not judged right`);
      else if (checkSubmit(a, sc.clue, { known, banned: e.banned, alsoRight: e.alsoRight }).ok) problems.push(`${e.word}: also_right "${a}" is accepted as a clue`);
    }
    for (const b of e.banned) {
      banned++;
      if (checkSubmit(b, sc.clue, { known, banned: e.banned, alsoRight: e.alsoRight }).ok) problems.push(`${e.word}: banned "${b}" is accepted as a clue`);
    }
  }
  say("## E. Word-list columns (also_right, banned)");
  say();
  say(`${right} also_right and ${banned} banned forms checked. ${problems.length === 0 ? "No problems." : "Problems:"}`);
  say();
  for (const p of problems) say(`- ${p}`);
  say();
}

writeFileSync(join(here, "sweep-report.md"), out.join("\n"));
console.log(out.slice(0, 12).join("\n"));
console.log("... wrote sweep-report.md");
