# `emoji.json` (GitHub flavor emoji data)

Embedded by `src/render/github/emoji.rs` (`include_str!`, parsed once on first use). Format:

```json
{
  "_source": "provenance",
  "emoji":   { "<alias>": "<emoji string exactly as in gemoji, VS16 included>", ... },
  "wrapped": ["<alias>", ...],
  "custom":  ["<name>", ...]
}
```

* `emoji` — every alias of gemoji `db/emoji.json` @ `fadaeaf1f1a9be82b321316a6c5502e43138b2f6`
  (1913 aliases, MIT, see `THIRD_PARTY_NOTICES.md`).
* `wrapped` — the 26 names github.com renders as `<g-emoji class="g-emoji" alias="NAME">`
  (from shortcodes and from the same raw Unicode string), observed with `POST /markdown`
  on 2026-10-01 (`research/github/emoji/probe_summary.json`, GH §8.5).
* `custom` — the 23 non-Unicode names (`:octocat:`, `:shipit:` …) rendered as
  `<img class="emoji">`; the images are served from the style bundle
  (`github/emoji/<name>.png`, downloaded by `md-render vendor`).

Regenerate (from the research directory) with:

```sh
python3 - <<'PY'
import json
d = json.load(open('github/emoji/gemoji-emoji.json'))
s = json.load(open('github/emoji/probe_summary.json'))
m = {a: e['emoji'] for e in d for a in e['aliases']}
out = {'_source': 'github/gemoji db/emoji.json @ fadaeaf1f1a9be82b321316a6c5502e43138b2f6 (MIT); '
                  'wrapped/custom lists from GitHub API probes 2026-10-01',
       'emoji': dict(sorted(m.items())),
       'wrapped': s['wrapped_shortcodes'],
       'custom': s['image_shortcodes']}
open('emoji.json', 'w').write(json.dumps(out, ensure_ascii=False, separators=(',', ':')))
PY
```
