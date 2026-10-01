# data/gitlab

Embedded data for the GitLab flavor (`include_str!`; see `THIRD_PARTY_NOTICES.md`).

| File | Owner | Content |
|---|---|---|
| `emoji.json` | render-gitlab | GitLab's emoji DB v4 + aliases, compacted for `EmojiFilter` (GL §5.13) |
| `rouge*.json` | highlight | Rouge 5.1.0 lexer registry (GL §8.1) |

## `emoji.json`

```json
{"source": "...",
 "emoji":   [[name, codepoints, description, unicode_version], ...],
 "aliases": [[alias, name], ...]}
```

Generated from `https://gitlab.com/-/emojis/4/emojis.json` (3,782 entries
`{n, c, e, d, u}`; the category `c` is dropped) and gitlab-org/gitlab
`fixtures/emojis/aliases.json` @ `8396d286bea7` (1,723 entries; the two keys ending in a space,
`":) "` and `":( "`, are dropped because they are not alpha codes). Regenerate with:

```python
import json
emoji = json.load(open('emojis.json', encoding='utf-8'))
aliases = json.load(open('aliases.json', encoding='utf-8'))
names = {e['n'] for e in emoji}
rows = [[e['n'], e['e'], e['d'], e['u']] for e in emoji]
al = [[a, n] for a, n in aliases.items() if not a.endswith(' ') and n in names and a not in names]
with open('emoji.json', 'w', encoding='utf-8') as f:
    f.write('{"source":' + json.dumps(SOURCE) + ',\n"emoji":[\n')
    f.write(',\n'.join(json.dumps(r, ensure_ascii=False, separators=(',', ':')) for r in rows))
    f.write('\n],\n"aliases":[\n')
    f.write(',\n'.join(json.dumps(r, ensure_ascii=False, separators=(',', ':')) for r in al))
    f.write('\n]}\n')
```
