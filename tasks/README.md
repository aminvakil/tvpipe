# Task board

One file per workstream. The owner updates the `Status` line and the
checklist as it lands pieces on `main`.

| Card | Owner | Branch / worktree | Status |
|---|---|---|---|
| [W1 crypto](W1-crypto.md) | subagent `w1-crypto` | `w1-crypto` / `../streamlib-w1` | in progress |
| [W2 codecs](W2-codecs.md) | subagent `w2-codecs` | `w2-codecs` / `../streamlib-w2` | done |
| [W3 net](W3-net.md) | subagent `w3-net` | `w3-net` / `../streamlib-w3` | in progress |
| [W4 media](W4-media.md) | subagent `w4-media` | `w4-media` / `../streamlib-w4` | in progress |
| W5 AirPlay pairing | — | — | waits for W1+W2+W3 |
| W6 FairPlay (clean) | — | — | waits for spec S1 |
| W7 AirPlay mirror | — | — | waits for W4+W5+W6+S2 |
| W8 Cast | — | — | waits for W1 AES + W2 json/protobuf + W3 + W4 |
| S1/S2 specs (dirty) | — | — | not started; needs human go-ahead |
