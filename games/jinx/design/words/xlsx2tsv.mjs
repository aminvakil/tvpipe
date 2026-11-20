#!/usr/bin/env node
// Minimal .xlsx -> .tsv converter (no npm deps; needs the `unzip` CLI).
// Usage: node xlsx2tsv.mjs file.xlsx [sheetNumber=1] > out.tsv
// Handles shared strings, inline strings and numbers; good enough for the
// flat tables the word-norm datasets ship as. Not a general xlsx reader.
import { execFileSync } from "node:child_process";

const [file, sheet = "1"] = process.argv.slice(2);
if (!file) {
  console.error("usage: node xlsx2tsv.mjs file.xlsx [sheet]");
  process.exit(2);
}
const read = (name) =>
  execFileSync("unzip", ["-p", file, name], { maxBuffer: 1 << 30 }).toString("utf8");

const unescape = (s) =>
  s
    .replace(/&lt;/g, "<")
    .replace(/&gt;/g, ">")
    .replace(/&quot;/g, '"')
    .replace(/&apos;/g, "'")
    .replace(/&#(\d+);/g, (_, n) => String.fromCodePoint(+n))
    .replace(/&amp;/g, "&");

let shared = [];
try {
  const xml = read("xl/sharedStrings.xml");
  shared = [...xml.matchAll(/<si>([\s\S]*?)<\/si>/g)].map((m) =>
    unescape([...m[1].matchAll(/<t[^>]*>([\s\S]*?)<\/t>/g)].map((t) => t[1]).join("")),
  );
} catch {
  // no shared strings
}

const colIndex = (ref) => {
  let n = 0;
  for (const ch of ref.replace(/\d+/g, "")) n = n * 26 + (ch.charCodeAt(0) - 64);
  return n - 1;
};

const xml = read(`xl/worksheets/sheet${sheet}.xml`);
const out = [];
for (const row of xml.matchAll(/<row[^>]*>([\s\S]*?)<\/row>/g)) {
  const cells = [];
  for (const c of row[1].matchAll(/<c ([^>]*?)(?:\/>|>([\s\S]*?)<\/c>)/g)) {
    const attrs = c[1];
    const body = c[2] || "";
    const ref = /r="([A-Z]+)\d+"/.exec(attrs)?.[1];
    const type = /t="(\w+)"/.exec(attrs)?.[1];
    let v = /<v>([\s\S]*?)<\/v>/.exec(body)?.[1] ?? "";
    if (type === "s") v = shared[+v] ?? "";
    else if (type === "inlineStr") v = [...body.matchAll(/<t[^>]*>([\s\S]*?)<\/t>/g)].map((t) => t[1]).join("");
    v = unescape(v).replace(/[\t\r\n]+/g, " ");
    const i = ref ? colIndex(ref) : cells.length;
    while (cells.length < i) cells.push("");
    cells[i] = v;
  }
  out.push(cells.join("\t"));
}
process.stdout.write(out.join("\n") + "\n");
