You are curating secret words for "Jinx", a casual party word game (like "Just One"): one player guesses a secret word; every other player writes ONE single-word clue; identical/same-family clues cancel each other ("jinx"), so good secret words have several obvious clues (so jinxes happen) but also room for creative clues. Players: mixed ages (kids ~10+, adults), US and UK, non-native but fluent English speakers.

KEEP a word only if ALL hold:
- Everybody (a 10-year-old, a fluent non-native speaker, US and UK) understands it instantly, without thinking.
- It is a good one-word-clue target: concrete or very familiar concept, at least 3-4 obvious one-word clues AND room for creative ones. Not so vague that clues can't converge (e.g. "thing", "piece", "place", "item", "information", "quality", "amount", "range").
- Its main meaning is the same in US and UK English (reject pants, chips, biscuit, boot, flat, jumper, rubber, torch, pudding, etc.) and the spelling is the same.
- Not a proper noun, brand, character or trademark-ish word (facebook, batman, lego, frisbee, velcro, jacuzzi...).
- Nothing offensive, sexual, gross-out-heavy, tragic (death, disease, disaster), violent (weapons other than fairy-tale/medieval ones like sword/bow/shield are fine), crime, drugs/alcohol/tobacco, religion, politics, national symbols, body shaming, slang, or that could embarrass someone at a family party.
- A base form: not a plural, not an inflected verb form (-ed/-ing/-s), not a comparative. Nouns ideally singular (reject "scissors"? keep only if the plural-only form is the normal word, then flag risk "plural-only").
- Not an adverb/function word. Mostly nouns; verbs and adjectives only if very vivid and guessable (e.g. swim, sneeze, sticky, frozen), so be stricter with them.
- Not a trivial compound/derivation whose obvious clue is a part of it (e.g. "firefighter"->fire, "snowman"->snow, "toothbrush"->tooth, "goldfish"->gold, "sunflower"->sun, "rainbow"->rain, "teacher"->teach) UNLESS it's a great word anyway; then keep but set risk.
- Not a near-synonym/duplicate of a more common word (e.g. "automobile" vs car, "kitty" vs cat, "grandma"/"granny"/"nan" all regional/informal: keep at most the clearest one).
- Not too obscure/dated/technical/regional (e.g. "satchel", "parchment", "calculus", "shorthand"), and not a pure diminutive/baby word (mummy, doggy).

For each word return:
- w: the word exactly as given (without the POS in parentheses)
- keep: true/false
- q: game quality 1-5 (5 = perfect party word like "pizza", "volcano", "ghost"; 3 = fine filler; 1 = bad). Give q for rejected words too.
- diff: "easy" | "medium" | "hard" for guessing from one-word clues: easy = extremely familiar, concrete, obvious clue cluster (cat, pizza, rain, ball); medium = familiar but the clues are less unique or the concept has competitors (ladder, mirror, island, whisper); hard = familiar to all but less frequent/abstract-ish or clues point many ways (compass, echo, shadow, rhythm, puzzle, gravity).
- cat: exactly one of: animal, food, nature (plants, landscape, weather, space, natural phenomena), body, home (household objects, furniture, kitchen, everyday objects), tool (tools, instruments, devices), clothing, transport, place (buildings, rooms, locations), people (roles, jobs, family), sport (sports, games, hobbies, toys), music (music, art, performance, entertainment), fantasy (magic, myths, fairy tales, spooky), science (science, school, tech), material (substances: glass, wood, sand...), action (verbs), quality (adjectives), event (parties, occasions, activities like picnic).
- why: when keep=false, a SHORT reason starting with one of: "vague:", "obscure:", "us-uk:", "proper:", "sensitive:", "inflected:", "compound:", "duplicate:", "abstract:", "weak-pos:" (adverbs/weak verbs/adjectives), "other:". Empty string if kept.
- risk: when kept, optional SHORT note for the clue-matching engine: obvious clues that share a word family or are contained in the word (e.g. "snowman: snow, man are parts"; "baker: bake/bread"), or that the word is plural-only, has a strong second meaning, or a common US/UK spelling variant. Empty string if none.

Be strict: we only want ~25-35% of these candidates. Prefer variety. Answer as JSON {"words":[...]} with one entry per input word, same order.
