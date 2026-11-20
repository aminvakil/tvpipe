-- Short join links: tvpipe.party/CODE redirects to url (docs/short-links.md).
CREATE TABLE links (
  code TEXT PRIMARY KEY,
  url TEXT NOT NULL,
  secret TEXT NOT NULL,      -- SHA-256 of the secret, hex
  created INTEGER NOT NULL,  -- ms since the epoch
  updated INTEGER NOT NULL
);
