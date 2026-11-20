// Build the dictionary the game ships for MatchOptions.known (Node only):
//
//   node build-dict.ts      -> dict.txt (one lowercase a-z word per line, sorted) + dict-LICENSE.txt
//
// Contents: SCOWL 2020.12.07 (english, american and british words, upper-case
// words and proper names, sizes <= 60), lowercased, letters only, plus every
// secret word of ../words/words.txt and its also_right forms (as match.ts keys).
// Input: ../words/raw/scowl-2020.12.07 (fetched by the word-list workstream).
import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { gzipSync } from "node:zlib";

import { clean, parseWordList } from "./match.ts";

const here = dirname(fileURLToPath(import.meta.url));
const scowl = join(here, "..", "words", "raw", "scowl-2020.12.07");
const SIZES = [10, 20, 35, 40, 50, 55, 60];
const KINDS = ["english", "american", "british"].flatMap((v) => [`${v}-words`, `${v}-upper`, `${v}-proper-names`]);

if (!existsSync(join(scowl, "final"))) {
  console.error(`missing ${scowl}/final: run ../words/fetch.sh first`);
  process.exit(1);
}

const words = new Set<string>();
let files = 0;
for (const kind of KINDS) {
  for (const size of SIZES) {
    const p = join(scowl, "final", `${kind}.${size}`);
    if (!existsSync(p)) continue;
    files++;
    for (const line of readFileSync(p, "latin1").split("\n")) {
      const w = line.trim().toLowerCase();
      if (/^[a-z]+$/.test(w)) words.add(w);
    }
  }
}
const fromScowl = words.size;

let secrets = 0;
const list = join(here, "..", "words", "words.txt");
if (existsSync(list)) {
  for (const e of parseWordList(readFileSync(list, "utf8"))) {
    for (const raw of [e.word, ...e.alsoRight]) {
      const c = clean(raw);
      if (c.ok && /^[a-z]+$/.test(c.clue.key) && !words.has(c.clue.key)) {
        words.add(c.clue.key);
        secrets++;
      }
    }
  }
}

const text = [...words].sort().join("\n") + "\n";
writeFileSync(join(here, "dict.txt"), text);
writeFileSync(
  join(here, "dict-LICENSE.txt"),
  "dict.txt is derived from SCOWL (Spell Checker Oriented Word Lists), version 2020.12.07,\n" +
    "http://wordlist.aspell.net/, sizes 10-60 of the English, American and British word,\n" +
    "upper-case and proper-name lists, lowercased; plus the Jinx secret words.\n" +
    "SCOWL's copyright and permission notice follows.\n\n" +
    readFileSync(join(scowl, "Copyright"), "latin1"),
);
const raw = Buffer.byteLength(text);
const gz = gzipSync(text, { level: 9 }).length;
console.log(
  `dict.txt: ${words.size} words (${fromScowl} from ${files} SCOWL files, ${secrets} added from words.txt), ` +
    `${raw} bytes, ${gz} bytes gzipped`,
);
