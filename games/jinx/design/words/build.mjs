#!/usr/bin/env node
// Jinx secret-word candidate pipeline. Node >= 18, no npm deps.
//
//   node build.mjs            -> candidates.tsv (+ counts on stderr)
//   node build.mjs --lint     -> also lints words.txt (columns, duplicates,
//                                regional spellings, deny list)
//
// Inputs live in raw/ (not committed, see README.md "Fetching the data").
// The automatic filters only narrow the field; words.txt is the hand-curated
// result (curation.tsv records every decision on the candidates).
import { readFileSync, writeFileSync, readdirSync } from "node:fs";
import { execFileSync } from "node:child_process";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const raw = (p) => join(here, "raw", p);
const log = (...a) => console.error(...a);

// ---- thresholds (documented in README.md) ---------------------------------
const T = {
  minLen: 3,
  maxLen: 12,
  minKnown: 0.9, // Lancaster: share of raters who knew the word (better of the two tasks)
  minKidZipf: 3.0, // CYP-LEX: max Zipf over the 7-9 and 10-12 age bands
  minAdultZipf: 2.5, // SUBTLEX-UK Zipf (as redistributed in CYP-LEX)
  maxProperShare: 0.2, // CYP-LEX: share of NNP/NNPS tags
  minBaseShare: 0.6, // CYP-LEX: share of tags that are base forms (NN, VB/VBP, JJ)
  minNounSense: 2.8, // Lancaster: strongest external sense for nouns (0-5)
  minNounAny: 3.5, // Lancaster: or strongest of all 11 sensorimotor dimensions
  glasgowMinImag: 5.5, // Glasgow imageability (1-7), alternative to concreteness
  minOtherSense: 3.8, // Lancaster: same, for verbs and adjectives
  minVerbAction: 3.8, // Lancaster: strongest action effector for verbs (0-5)
  glasgowMinCnc: 5.0, // Glasgow concreteness (1-7), used when Lancaster is weak
};

// ---- CSV/TSV ---------------------------------------------------------------
function parseCsvLine(line) {
  const out = [];
  let cur = "";
  let q = false;
  for (let i = 0; i < line.length; i++) {
    const c = line[i];
    if (q) {
      if (c === '"' && line[i + 1] === '"') (cur += '"'), i++;
      else if (c === '"') q = false;
      else cur += c;
    } else if (c === '"') q = true;
    else if (c === ",") out.push(cur), (cur = "");
    else cur += c;
  }
  out.push(cur);
  return out;
}
function table(path, sep = ",", headerRow = 0) {
  const lines = readFileSync(path, "utf8").replace(/^\uFEFF/, "").split(/\r?\n/);
  const head = (sep === "," ? parseCsvLine(lines[headerRow]) : lines[headerRow].split(sep)).map((h) =>
    h.trim(),
  );
  const rows = [];
  for (const l of lines.slice(headerRow + 1)) {
    if (!l) continue;
    const cells = sep === "," ? parseCsvLine(l) : l.split(sep);
    rows.push(Object.fromEntries(head.map((h, i) => [h, cells[i]])));
  }
  return rows;
}
function xlsx(path, sheet = 1) {
  const tsv = execFileSync("node", [join(here, "xlsx2tsv.mjs"), path, String(sheet)], {
    maxBuffer: 1 << 30,
  }).toString("utf8");
  return tsv.split("\n").filter(Boolean).map((l) => l.split("\t"));
}

// ---- Lancaster Sensorimotor Norms (CC BY 4.0) ------------------------------
const lanc = new Map();
for (const r of table(raw("Lancaster_sensorimotor_norms_for_39707_words.csv"))) {
  lanc.set(r.Word.trim().toLowerCase(), {
    // the better of the two rating tasks (~18 raters each): words nobody
    // knows score low in both, while one careless "don't know" in a single
    // task would otherwise drop "horse", "egg" or "zoo".
    known: Math.max(+r["Percent_known.perceptual"], +r["Percent_known.action"]),
    sm: +r["Max_strength.sensorimotor"],
    perc: +r["Max_strength.perceptual"],
    act: +r["Max_strength.action"],
    dom: r["Dominant.perceptual"],
    visual: +r["Visual.mean"],
    // strongest of the five external senses (interoception left out: it is
    // what makes "sorrow" or "desire" look perceptual)
    ext: Math.max(+r["Visual.mean"], +r["Haptic.mean"], +r["Auditory.mean"], +r["Gustatory.mean"], +r["Olfactory.mean"]),
  });
}
log(`Lancaster: ${lanc.size} words`);

// ---- Glasgow Norms (CC BY 4.0) ---------------------------------------------
const glas = new Map();
{
  const rows = xlsx(raw("GlasgowNorms.xlsx"));
  // row 0: scale names, row 1: M/SD/N; columns: word, length, then 9 x (M,SD,N)
  const scales = ["AROU", "VAL", "DOM", "CNC", "IMAG", "FAM", "AOA", "SIZE", "GEND"];
  for (const r of rows.slice(2)) {
    const g = {};
    scales.forEach((s, i) => (g[s] = +r[2 + i * 3]));
    glas.set(r[0].trim().toLowerCase(), g);
  }
}
log(`Glasgow: ${glas.size} words`);

// ---- CYP-LEX (CC BY 4.0) ---------------------------------------------------
const cyp = new Map();
for (const r of table(raw("CYPLEX_all_age_bands.csv"))) {
  const tags = (r.AllPoS || "").split(".");
  const freqs = (r.AllPoSFreq || "").split(".").map(Number);
  // AllPoSFreq is "." separated but the numbers can have decimals; when the
  // counts do not line up, fall back to the whole count on the first tag.
  let pos = {};
  if (freqs.length === tags.length) tags.forEach((t, i) => (pos[t] = (pos[t] || 0) + freqs[i]));
  else pos = { [tags[0]]: +r.Count };
  cyp.set(r.Word, {
    zipf: +r.Zipf_freq,
    lemma: r.Lemma,
    pos,
    kid: Math.max(+r.CYPLEX79_log || 0, +r.CYPLEX1012_log || 0),
    k79: +r.CYPLEX79_log || 0,
    adult: +r.SubtlexUK_log || 0,
    cbeebies: +r.CBeebies_log || 0,
  });
}
log(`CYP-LEX: ${cyp.size} word forms`);

// ---- Open English WordNet 2025 (CC BY 4.0) ---------------------------------
const lexnames = `adj.all adj.pert adv.all noun.Tops noun.act noun.animal noun.artifact noun.attribute noun.body noun.cognition noun.communication noun.event noun.feeling noun.food noun.group noun.location noun.motive noun.object noun.person noun.phenomenon noun.plant noun.possession noun.process noun.quantity noun.relation noun.shape noun.state noun.substance noun.time verb.body verb.change verb.cognition verb.communication verb.competition verb.consumption verb.contact verb.creation verb.emotion verb.motion verb.perception verb.possession verb.social verb.stative verb.weather adj.ppl`.split(
  " ",
);
const wn = new Map(); // lemma -> {n: [lexname...], v: [...], a: [...]}
for (const [pos, file] of [
  ["n", "noun"],
  ["v", "verb"],
  ["a", "adj"],
]) {
  const lexOf = new Map();
  for (const l of readFileSync(raw(`oewn2025/data.${file}`), "utf8").split("\n")) {
    if (!l || l.startsWith(" ")) continue;
    const [off, lf] = l.split(" ");
    lexOf.set(off, lexnames[+lf]);
  }
  for (const l of readFileSync(raw(`oewn2025/index.${file}`), "utf8").split("\n")) {
    if (!l || l.startsWith(" ")) continue;
    const f = l.trim().split(" ");
    const lemma = f[0];
    const nSyn = +f[2];
    const offs = f.slice(f.length - nSyn);
    const e = wn.get(lemma) || {};
    e[pos] = offs.map((o) => lexOf.get(o));
    wn.set(lemma, e);
  }
}
log(`OEWN: ${wn.size} lemmas`);

// ---- SCOWL (permissive): regional spellings --------------------------------
const regional = new Set();
{
  const dir = raw("scowl-2020.12.07/final");
  for (const f of readdirSync(dir)) {
    const m = /^(.+)-words\.(\d+)$/.exec(f);
    if (!m || m[1] === "english" || +m[2] > 70) continue;
    for (const w of readFileSync(join(dir, f), "latin1").split("\n")) if (w) regional.add(w);
  }
}
log(`SCOWL regional/variant spellings (size <= 70): ${regional.size}`);

// ---- manual deny list --------------------------------------------------------
const deny = new Map();
for (const l of readFileSync(join(here, "deny.tsv"), "utf8").split("\n")) {
  if (!l || l.startsWith("#")) continue;
  const [w, reason] = l.split("\t");
  deny.set(w.trim(), reason || "deny");
}

// ---- pipeline ----------------------------------------------------------------
const steps = [];
const rejected = new Map(); // step -> [words]
function step(name, words, pred) {
  const kept = [];
  const rej = [];
  for (const w of words) (pred(w) ? kept : rej).push(w);
  steps.push([name, kept.length, rej.length]);
  rejected.set(name, rej);
  return kept;
}

const BASE = new Set(["NN", "VB", "VBP", "JJ"]);
const posShare = (w) => {
  const c = cyp.get(w);
  const tot = Object.values(c.pos).reduce((a, b) => a + b, 0) || 1;
  const share = (t) => (c.pos[t] || 0) / tot;
  return {
    proper: share("NNP") + share("NNPS"),
    base: [...BASE].reduce((a, t) => a + share(t), 0),
    noun: share("NN"),
    verb: share("VB") + share("VBP"),
    adj: share("JJ"),
  };
};

let words = [...new Set([...lanc.keys(), ...glas.keys()])];
steps.push(["universe (Lancaster ∪ Glasgow)", words.length, 0]);
words = step("single lowercase word, 3-12 letters", words, (w) =>
  new RegExp(`^[a-z]{${T.minLen},${T.maxLen}}$`).test(w),
);
words = step(`known by >= ${T.minKnown * 100}% of raters`, words, (w) => {
  const l = lanc.get(w);
  if (l) return l.known >= T.minKnown;
  const g = glas.get(w);
  return g && g.FAM >= 5.0; // Glasgow familiarity (1-7)
});
words = step(`in children's books (CYP-LEX 7-12 Zipf >= ${T.minKidZipf})`, words, (w) => {
  const c = cyp.get(w);
  return c && c.kid >= T.minKidZipf;
});
words = step(`in adult subtitles (SUBTLEX-UK Zipf >= ${T.minAdultZipf})`, words, (w) => {
  return cyp.get(w).adult >= T.minAdultZipf;
});
words = step("lemma form (not plural / inflected)", words, (w) => cyp.get(w).lemma === w);
words = step(`not a proper noun (NNP share <= ${T.maxProperShare})`, words, (w) => posShare(w).proper <= T.maxProperShare);
words = step(`mostly base-form noun/verb/adjective (>= ${T.minBaseShare})`, words, (w) => posShare(w).base >= T.minBaseShare);
words = step("in WordNet as noun/verb/adjective (or a clearly sensory noun)", words, (w) => {
  const e = wn.get(w);
  const l = lanc.get(w);
  return (e && (e.n || e.v || e.a)) || (l && l.ext >= 4 && posShare(w).noun >= 0.6);
});
words = step("sensory/concrete enough", words, (w) => {
  const l = lanc.get(w);
  const g = glas.get(w);
  const p = posShare(w);
  const main = p.noun >= Math.max(p.verb, p.adj) ? "n" : p.verb >= p.adj ? "v" : "a";
  if (main === "n" && g && (g.CNC >= T.glasgowMinCnc || g.IMAG >= T.glasgowMinImag)) return true;
  if (!l) return false;
  if (main === "n") return l.ext >= T.minNounSense || l.sm >= T.minNounAny;
  if (main === "v") return l.ext >= T.minOtherSense || l.act >= T.minVerbAction;
  return l.ext >= T.minOtherSense;
});
words = step("no US/UK spelling split (SCOWL)", words, (w) => !regional.has(w));
words = step("not on deny list (deny.tsv)", words, (w) => !deny.has(w));

// ---- score + hints ---------------------------------------------------------
const CAT_OF_LEX = {
  "noun.animal": "animal",
  "noun.food": "food",
  "noun.plant": "nature",
  "noun.object": "nature",
  "noun.phenomenon": "nature",
  "noun.substance": "stuff",
  "noun.body": "body",
  "noun.artifact": "thing",
  "noun.location": "place",
  "noun.person": "people",
  "noun.act": "activity",
  "noun.event": "event",
  "noun.time": "time",
  "noun.feeling": "feeling",
  "noun.communication": "language",
  "noun.shape": "shape",
};
function hint(w) {
  const e = wn.get(w) || {};
  const p = posShare(w);
  const main = p.noun >= Math.max(p.verb, p.adj) ? "n" : p.verb >= p.adj ? "v" : "a";
  if (main === "v") return { pos: "verb", cat: "action", lex: (e.v || []).join(",") };
  if (main === "a") return { pos: "adj", cat: "quality", lex: (e.a || []).join(",") };
  const counts = {};
  for (const l of e.n || []) counts[l] = (counts[l] || 0) + 1;
  const pri = Object.keys(CAT_OF_LEX);
  const best = pri.find((l) => counts[l]) || Object.keys(counts)[0] || "";
  return { pos: "noun", cat: CAT_OF_LEX[best] || best, lex: (e.n || []).join(",") };
}
// "ease" score: higher = easier to know (kid exposure, AoA, knowledge).
function ease(w) {
  const c = cyp.get(w);
  const g = glas.get(w);
  const l = lanc.get(w);
  let s = c.k79 + 0.5 * c.cbeebies;
  if (g) s += (7 - g.AOA) * 0.3;
  if (l) s += (l.known - 0.9) * 5;
  return s;
}

const out = ["word\tease\tpos\tcat_hint\tkid_zipf\tadult_zipf\tknown\tperceptual\tglasgow_cnc\tglasgow_aoa\twordnet"];
words.sort((a, b) => ease(b) - ease(a));
for (const w of words) {
  const c = cyp.get(w);
  const l = lanc.get(w);
  const g = glas.get(w);
  const h = hint(w);
  out.push(
    [
      w,
      ease(w).toFixed(2),
      h.pos,
      h.cat,
      c.kid.toFixed(2),
      c.adult.toFixed(2),
      l ? l.known.toFixed(2) : "",
      l ? l.perc.toFixed(2) : "",
      g ? g.CNC.toFixed(1) : "",
      g ? g.AOA.toFixed(1) : "",
      h.lex,
    ].join("\t"),
  );
}
writeFileSync(join(here, "candidates.tsv"), out.join("\n") + "\n");

log("\nstep\tkept\trejected");
for (const [n, k, r] of steps) log(`${n}\t${k}\t${r}`);
log(`\nwrote candidates.tsv (${words.length} words)`);
const sample = (name, n = 25) =>
  log(`  rejected at "${name}": ${rejected.get(name).filter((w, i, a) => i % Math.max(1, Math.floor(a.length / n)) === 0).slice(0, n).join(" ")}`);
if (process.argv.includes("--samples")) for (const [n] of steps.slice(1)) sample(n);
const whyIdx = process.argv.indexOf("--why");
if (whyIdx > 0)
  for (const w of process.argv.slice(whyIdx + 1)) {
    const at = [...rejected].find(([, ws]) => ws.includes(w));
    log(`${w}: ${at ? "rejected at " + at[0] : words.includes(w) ? "candidate" : "not in universe"}`, lanc.get(w), glas.get(w)?.CNC, cyp.get(w) && { ...cyp.get(w), share: posShare(w) });
  }

// ---- --lint: check the curated list ----------------------------------------
if (process.argv.includes("--lint")) {
  const list = readFileSync(join(here, "words.txt"), "utf8")
    .split("\n")
    .filter((l) => l && !l.startsWith("#"))
    .map((l) => l.split("\t"));
  const cand = new Set(words);
  const issues = [];
  const seen = new Set();
  for (const [w, diff, cat] of list) {
    if (seen.has(w)) issues.push(`${w}: duplicate`);
    seen.add(w);
    if (!["easy", "medium", "hard"].includes(diff)) issues.push(`${w}: bad difficulty ${diff}`);
    if (!cat) issues.push(`${w}: no category`);
    if (!cand.has(w)) issues.push(`${w}: not in candidates (manual addition)`);
    if (regional.has(w)) issues.push(`${w}: regional spelling`);
    if (deny.has(w)) issues.push(`${w}: on deny list`);
  }
  const cats = {};
  const diffs = {};
  for (const [, d, c] of list) (cats[c] = (cats[c] || 0) + 1), (diffs[d] = (diffs[d] || 0) + 1);
  log(`\nwords.txt: ${list.length} words`, diffs, cats);
  log(issues.length ? issues.join("\n") : "no issues");
}
