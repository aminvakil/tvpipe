# Jinx secret words (English)

`words.txt`: **989 words**, one per line,
`word<TAB>difficulty<TAB>category<TAB>also_right<TAB>banned` (lines starting
with `#` are comments; the header comment names the columns, which is what
`parseWordList` in `../matching/match.ts` reads). Difficulty is `easy` (335),
`medium` (474) or `hard` (180). Categories (18): animal 129, food 111,
home 102, nature 101, people 62, place 57, sport 52, fantasy 47, music 44,
transport 40, body 39, clothing 35, tool 35, event 32, science 32,
material 31, action 30 (verbs), quality 10 (adjectives).

The last two columns are optional, comma-separated (and empty or missing
for most words):

- `also_right` (39 words): guesses accepted as right — short forms and
  other names: bike (bicycle), hippopotamus (hippo), refrigerator (fridge),
  bbq, barbeque (barbecue), e-mail, catsup, uke, ukelele, die (dice), hiccough, mom/mum (mother), dad,
  granny/nan (grandma), grandad (grandpa), cab (taxi), couch (sofa),
  jumper (sweater), lorry (truck), closet (wardrobe), fall (autumn),
  hoover (vacuum), bunny (rabbit), present (gift), cash (money)… The
  matcher also rejects them as clues.
- `banned` (85 words): clues rejected as "too close": obvious Spanish,
  French, German and Italian translations of everyday words (gato, chat,
  katze, gatto for cat; agua, eau, wasser for water), without accents (the
  matcher folds them), and skipping forms that are English words a player
  may fairly use (pan or pain for bread, latte for milk, stern for star).

They live in `extras.tsv` and are merged by `assemble.mjs`.
`../matching/match.test.ts` checks every entry (passes).

`CREDITS.md`: attribution for the game's credits, ready to ship.

`clue-notes.tsv`: notes for the clue-matching engine (see the end).

## Sources and licences

Used to select words (all permit redistribution and commercial use, with
attribution; raw files are not committed, `fetch.sh` downloads them into
`raw/`, which is git-ignored):

| Dataset | What we use | Licence |
| --- | --- | --- |
| Lancaster Sensorimotor Norms (Lynott, Connell, Brysbaert, Brand & Carney 2020, *BRM* 52; [osf.io/rwhs6](https://osf.io/rwhs6)) | 39,707 words: share of raters who knew the word; perceptual (visual, haptic, auditory, taste, smell) and action strength 0–5 | CC BY 4.0 |
| Glasgow Norms (Scott, Keitel, Becirspahic, Yao & Sereno 2019, *BRM* 51; [osf.io/42s65](https://osf.io/42s65)) | 5,553 words: concreteness, imageability, familiarity, age of acquisition | CC BY 4.0 |
| CYP-LEX (Korochkina, Marelli, Brysbaert & Rastle 2024, *QJEP*; [osf.io/tnu8k](https://osf.io/tnu8k)) | Frequency in 1,200 UK children's/teen books by age band (7–9, 10–12), lemma, part-of-speech tags in context; also carries SUBTLEX-UK and CBeebies Zipf values | CC BY 4.0 |
| Open English WordNet 2025 ([en-word.net](https://en-word.net)) | Part of speech, noun "lexicographer files" (animal, food, artifact…) as category hints | CC BY 4.0 |
| SCOWL 2020.12.07 (Kevin Atkinson, [wordlist.aspell.net](http://wordlist.aspell.net)) | US/UK/CA/AU-only spellings and variants (color/colour, gray/grey, mom) | permissive (MIT-like, see `Copyright` in the tarball) |

Looked at, not used (nothing in the pipeline reads them):

- Word prevalence norms for 62,000 English lemmas (Brysbaert, Mandera, McCormick & Keuleers 2019, [osf.io/g4xrt](https://osf.io/g4xrt)): CC BY-NC-SA 4.0, so it may not feed anything.
- SUBTLEX-US frequencies (Brysbaert & New 2009, [osf.io/djpqz](https://osf.io/djpqz)): CC BY-NC-SA 4.0.
- Kuperman et al. 2012 age of acquisition (30k words) and Brysbaert, Warriner & Kuperman 2014 concreteness (40k): the Ghent download pages state no licence; the OSF copy ([osf.io/kj76e](https://osf.io/kj76e)) has none either. Glasgow (AoA, concreteness) and Lancaster (sensory strength, on the same 40k lemmas) cover the need under CC BY.
- wordfreq (Robyn Speer): data CC BY-SA 4.0 (share-alike); not needed given CYP-LEX.

No dataset values are shipped: `words.txt` is a hand-curated list of common
English words. `candidates.tsv` (committable) does carry a few numbers from
the CC BY sets; credit them as above if it is published.

## Method

Reproduce (Node ≥ 18, no npm packages, needs `curl`, `unzip`, `tar`):

```sh
./fetch.sh                 # raw data into raw/
node build.mjs             # filters -> candidates.tsv, counts on stderr
node build.mjs --samples   # plus samples of what each filter rejected
node assemble.mjs          # llm-curation.tsv + overrides.tsv -> words.txt, clue-notes.tsv
node build.mjs --lint      # lint words.txt (columns, duplicates, regional spellings, deny list)
node build.mjs --why zoo   # which filter dropped a word
node short-secrets.ts      # short secrets: containment sweep + plausible-clue check (Node >= 23)
```

`xlsx2tsv.mjs` is a tiny .xlsx reader (shells out to `unzip`).
`short-secrets.ts` imports the game's matcher from `../matching/match.ts`.

### 1. Automatic filters (`build.mjs`)

| Step | Threshold | Kept | Dropped |
| --- | --- | ---: | ---: |
| Universe: Lancaster ∪ Glasgow | | 40,808 | |
| Single lowercase word | 3–12 letters, a–z only | 33,490 | 7,318 |
| Known | ≥ 90% of Lancaster raters (better of its two tasks; one careless rater otherwise drops "zoo") or Glasgow familiarity ≥ 5/7 | 28,554 | 4,936 |
| In children's books | CYP-LEX Zipf ≥ 3.0 (≥ 1 per million) in the 7–9 or 10–12 band | 13,256 | 15,298 |
| In adult speech | SUBTLEX-UK Zipf ≥ 2.5 | 12,139 | 1,117 |
| Lemma form | CYP-LEX lemma = word (drops plurals, -ed/-ing forms) | 10,156 | 1,983 |
| Not a proper noun | ≤ 20% of CYP-LEX tokens tagged NNP | 10,081 | 75 |
| Content word in base form | ≥ 60% of tokens tagged NN, VB/VBP or JJ | 9,038 | 1,043 |
| In WordNet as n/v/adj | (or a noun with sensory strength ≥ 4) | 8,944 | 94 |
| Sensory / concrete | nouns: strongest external sense ≥ 2.8 or any sensorimotor dimension ≥ 3.5 or Glasgow concreteness ≥ 5 / imageability ≥ 5.5; verbs: sense or action ≥ 3.8; adjectives: sense ≥ 3.8 | 5,804 | 3,140 |
| No US/UK spelling split | not in any SCOWL regional/variant list up to size 70 | 5,746 | 58 |
| Not on `deny.tsv` | 127 hand-listed words: US/UK meaning splits, sensitive topics, currency/units | **5,619** | 127 |

The thresholds are deliberately loose: the filters only take out what no
person needs to look at. Interoception is left out of the sensory score on
purpose (it made "sorrow" and "desire" look concrete).

Samples of what each step dropped (from `--samples`):
- known: aardvark, covenant, dregs, marlin, strongbox
- children's books: abacus, laziness, nightstick, retweet, spokesperson
- adult speech: chalkboard, dumpster, rainstorm, sailboat, jogger
- lemma form: running, younger, sessions, splashing
- proper noun: december, easter, friday, nintendo, venus (also fang, jack, locust: a known cost)
- sensory: annoy, debt, inclusion, sudden, unfair
- spelling split: airplane, catalogue, gray, harbor, mom, meow, cheque
- deny list: pants, chips, biscuit, jumper, torch, rubber, pavement, lift; beer, gun, coffin, church; dollar, pound, county

### 2. LLM curation pass (`curation-prompt.md` → `llm-curation.tsv`)

Every candidate went to an LLM (Claude, batches of ~160 in stratified
order, 36 batches) with the prompt in `curation-prompt.md`: keep/reject with
a reason class, quality 1–5, difficulty, category, clue risks.
Result: **960 kept, 4,661 rejected**. Rejection reasons: abstract 825,
obscure 680, sensitive 663, weak part of speech 564, vague 515, compound
482, duplicate 284, other 266, us-uk 217, inflected 92, proper 71.

Examples: *vague* — crowd, head, plant, meat, station; *abstract* — fashion,
rescue, silence; *obscure* — dusk, radiator, sardine, orchard; *sensitive*
— explosion, poison, thief, president, tattoo, lottery; *us-uk* — elevator,
gravy, stove, movie/film, candy, jam, sled, cricket, icing/frosting;
*compound* — policeman, drummer; *duplicate* — bunny (rabbit), lawn
(grass), hippopotamus (hippo), telephone (phone); *proper* — lego, python,
ruby, halloween, turkey; *inflected* — painting, shopping, skiing.

The LLM was too strict on compounds and near-duplicates (it dropped
sunflower, toothbrush, goldfish, kitten), and a little loose elsewhere,
hence step 3.

### 3. Manual pass (`overrides.tsv`)

I read the whole list. `overrides.tsv` records every decision with a
reason: **90 dropped** (weak targets such as lid, poster, twig; near
duplicates such as hamburger/burger, bike/bicycle; spelling variants such as
doughnut/donut, make-up; too many compounds of one family such as
moonlight, raincoat, thunderstorm; obscure or niche: lasso, tuba, fin,
warthog, yeti, dam, sumo, piranha, puffin),
**119 added** back from the LLM's rejects (prince, kitten, sunflower,
toothbrush, bowl, plate, knife, match, spring…; 2 of them, slide and
catapult, the filters had dropped because their dominant tag is a verb),
and **144 difficulty fixes**.

960 − 90 + 119 = **989**.

### 3b. Short secrets (`short-secrets.ts`)

Invalid clues are rejected on the phone at submit and the giver retypes, so
an accidental hit (police contains *ice*, carpet contains *car*) only
matters if it is a clue someone would plausibly write for that word.

Rule: keep a short secret unless a natural, fair clue for it is rejected
by accident.

Method: `short-secrets.ts` first runs the game's `invalidClue` over common
words (CYP-LEX Zipf ≥ 3, SCOWL dictionary) for all 435 secrets of ≤ 5
letters and lists the "contains the word" hits (`short-secrets.tsv`,
205 secrets with hits). For the 25 secrets with frequent hits (elf, ice,
ant, pen, car, pig, sea, ear, key, bow, ham, pea, leg, corn, ship, tent,
tail, sand, lion, hand, pier, cape, nest, disco, fence),
`plausible-clues.tsv` lists ~20 natural clues each (Christmas, shelf for
*elf*; elephant, giant, plant for *ant*; sandal, hourglass for *sand*; arrow,
ribbon, violin for *bow*), and the script runs `checkSubmit` on every one
with the game dictionary (`../matching/dict.txt`). Rejected:
`plausible-rejected.tsv`.

Result: **none dropped**. All 29 rejections are the rules working, not
accidents: compounds and relatives (iceberg, anthill, piglet, seashell,
earring, keyhole, crossbow, popcorn, shipwreck, ponytail, sandcastle,
handshake), plurals and verb forms (peas, nesting, fencing) and family
(piggy, corny, sandy, lioness). Two are borderline and were kept:
rainbow for *bow* and peanut for *pea* (both visibly contain the word).
Accidental words like shelf (*elf*), elephant/giant/plant (*ant*) and
sandal (*sand*) are not rejected once the dictionary is on.

### 4. Difficulty and category

Difficulty means "how hard to guess from one-word clues", not just how rare
the word is. `assemble.mjs` ranks the words by
`0.5 × data rank + 0.35 × LLM difficulty + 0.15 × LLM quality`, where the
data rank is an ease score from `build.mjs` (children's-book frequency
7–9, CBeebies frequency, Glasgow age of acquisition, Lancaster knowledge).
Top 35% → easy, bottom 20% → hard, then the manual fixes in
`overrides.tsv` (e.g. dream, story, surprise → hard: clues scatter;
popcorn, igloo, panda → easy; bark, match, spring, fan, spell → hard:
two strong meanings). Category comes from the LLM (WordNet lexicographer
files are in `candidates.tsv` as a hint), with manual corrections.

Compounds: since an invalid clue is now rejected at submit and the player
just retypes, compounds stay. Their tags were reviewed by asking what is
left once the part-clues are gone: snowman (carrot, winter), goldfish
(bowl, pet), sunflower, firefighter, waterfall, spaceship, lighthouse,
superhero, cupcake, homework and necklace went from easy to medium;
toothbrush, eyebrow, doorbell, teapot, starfish, seahorse, treehouse,
bedroom and newspaper (both parts are the obvious clues) to hard. Compounds
whose parts are not the natural clues (butterfly, pineapple, breakfast,
cowboy, popcorn, rainbow, laptop, birthday) kept their tag.

Easy: dog, moon, pizza, giraffe, volcano, school, pilot, jellyfish.
Medium: lantern, plumber, museum, waffle, suitcase, captain, barbecue.
Hard: hourglass, glacier, pitchfork, echo, horizon, gravity, auction, tutu.

## Notes for clue matching (`clue-notes.tsv`)

Columns: `word`, `compound_parts` (the word splits into two common words:
`snowman` → `snow+man`), `family` (derived from a common word:
`baker` → `bake`, `dancer` → `dance`, `sticker` → `stick`),
`contains_list_words` (other secret words inside it), `llm_note` (free
text). Parts and families are found automatically from CYP-LEX words with
Zipf ≥ 4 plus a stop list of accidental splits (off+ice, kit+ten); treat
them as a starting point.

- **Compounds** (91 words, e.g. snowman, toothbrush, firefighter,
  goldfish, sunflower, rainbow, cupcake, pancake, keyboard, lighthouse,
  butterfly, pineapple): the most natural clue is a part of the word; the
  matcher rejects it at submit ("part of the word") and the player retypes.
- **Families** (actor/act, baker/bake, teacher/teach, sailor/sail,
  explorer/explore, farmer/farm, printer/print, toaster/toast,
  freezer/freeze, builder/build, dancer/dance, bowling/bowl, archery/arrow):
  needs the same-stem check.
- **Do not deal related words in one game**: `contains_list_words` lists
  them (snow/snowman/snowflake, fire/fireplace/firework/firefly/firefighter,
  sun/sunset/sunrise/sunflower, seahorse/seashell/seaweed, bath/bathroom,
  tooth/toothbrush/toothpaste). Also near-synonym pairs with no shared
  letters: boat/yacht, puppy/dog,
  kitten/cat, lamb/sheep, pony/horse, sofa/armchair, cup/mug,
  prince/princess, king/queen, magic/wizard/witch, tortoise/turtle.
- **Strong second meanings** (good hard words, but expect scattered clues):
  bat, bark, match, fan, spring, seal, palm, ring, nail, bank, horn,
  stamp, web, wave, tie, bug, cone, mint, gum, chain, lock, sink, slide,
  swing, trap, spell, judge, mouse. *story* (US/UK "storey"), *party*
  (political).
- **Secret words are base forms**, but some are usually said in the plural,
  so the guess check must accept the regular plural: dice (already plural of
  die), noodle, firework, slipper, hiccup, earring, glove, sock, shoe,
  shoelace, confetti/popcorn (mass nouns).
- **Spelling variants and other names** to accept in guesses: in the
  `also_right` column. Words whose standard spelling differs between US
  and UK are not on the list (doughnut/donut, make-up, colour, grey, axe,
  mum/mom as secrets).
- **Hard to spell** (typo tolerance matters for the typed guess):
  mayonnaise, croissant, chandelier, kaleidoscope, accordion, tambourine,
  saxophone, somersault, broccoli, cauliflower, cinnamon, stethoscope,
  thermometer, leprechaun, porcupine, chameleon, pharaoh, hyena, yacht,
  sphinx, restaurant, dictionary.
- **Short secrets**: see 3b; `short-secrets.tsv` lists, for every secret of
  ≤ 5 letters, the common words that contain it; `plausible-rejected.tsv`
  the natural clues the matcher rejects.

## Decisions (settled)

1. The CC BY-NC-SA prevalence data feeds nothing; the 9 words it had
   flagged are plain judgement overrides.
2. Credits: `CREDITS.md`.
3. Sensitivity line as is: knife, sword, skull, skeleton, zombie, vampire,
   police, hospital, medicine, judge are in; alcohol, guns, death,
   religion, kiss, underwear are out.
4. Compounds stay; part-clues are rejected at submit (tags reviewed, 3/4).
5. Short secrets stay unless a natural clue is rejected by accident (3b):
   none is.

## Open questions

1. Category set: 18 categories is generous; the game probably only needs
   them for variety (no two from the same category in a row).
