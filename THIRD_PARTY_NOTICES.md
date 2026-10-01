# Third-party notices

md-render is BSD-3-Clause licensed (see `LICENSE`). It ports or embeds the following third-party
material, each under its own license. Each owner appends one section per source.

Styles, fonts and scripts downloaded by `md-render vendor` come from github.com / gitlab.com /
jsDelivr at run time on the user's machine; they are not distributed with md-render.

---------------------------------------------------------------------------------------------------

## render-github: gemoji emoji data (`data/github/emoji.json`)

The alias → emoji table is derived from `db/emoji.json` of
[github/gemoji](https://github.com/github/gemoji) at commit
`fadaeaf1f1a9be82b321316a6c5502e43138b2f6` (only aliases and emoji strings are kept). The lists of
`<g-emoji>`-wrapped and custom (image) emoji names were observed on github.com and are facts, not
copied code. No emoji images are distributed; the 23 custom emoji PNGs are downloaded by
`md-render vendor` at run time.

SPDX-License-Identifier: MIT — Copyright (c) 2019 GitHub, Inc.

## render-github: Primer Octicons (`src/render/github/octicons.rs`)

SVG path data of the `link`, `info`, `light-bulb`, `report`, `alert` and `stop` icons, plus the
markup of the enrichment loading spinner, as emitted by github.com, from
[primer/octicons](https://github.com/primer/octicons).

SPDX-License-Identifier: MIT — Copyright (c) 2026 GitHub Inc.

MIT License text (applies to both sections above):

    Permission is hereby granted, free of charge, to any person obtaining a copy of this
    software and associated documentation files (the "Software"), to deal in the Software
    without restriction, including without limitation the rights to use, copy, modify, merge,
    publish, distribute, sublicense, and/or sell copies of the Software, and to permit persons
    to whom the Software is furnished to do so, subject to the following conditions:

    The above copyright notice and this permission notice shall be included in all copies or
    substantial portions of the Software.

    THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED,
    INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR
    PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE
    FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR
    OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
    DEALINGS IN THE SOFTWARE.
