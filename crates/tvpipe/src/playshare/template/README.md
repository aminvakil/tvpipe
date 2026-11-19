# {{title}}

A party game for `tvpipe playshare`, made with `tvpipe bundle --init`:
the host page (`host/`) keeps the rules, the TV page (`tv/`) is cast to
the TVs, phones open the phone page (`phone/`).

    tvpipe playshare .                 # play it (edit, then reload the page)
    tvpipe bundle . --check            # check it
    tvpipe bundle . -o {{name}}.zip    # pack it for others: tvpipe playshare {{name}}.zip

tvpipe's docs/games.md has the whole story.
