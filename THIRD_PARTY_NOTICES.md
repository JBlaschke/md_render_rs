# Third-party notices

md-render is BSD-3-Clause licensed (see `LICENSE`). It ports or embeds the following third-party
material, each under its own license. Each owner appends one section per source.

Styles, fonts and scripts downloaded by `md-render vendor` come from github.com / gitlab.com /
jsDelivr at run time on the user's machine; they are not distributed with md-render.

## Primer Octicons (page)

Used in: `src/page/octicons.rs` (page chrome icons and the code-block clipboard template),
`assets/client/github.js` and `assets/client/gitlab.js` (copy / check icons drawn when no
template or icon sprite is available). Source: <https://github.com/primer/octicons>, path data
as emitted by github.com (16 px `book`, `info`, `alert`, `stop`, `file`, `file-directory-fill`,
`copy`, `check`).

```
MIT License

Copyright (c) 2026 GitHub Inc.

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

The fallback stylesheets `assets/fallback/github.css` and `assets/fallback/gitlab.css` are
hand-written for md-render (BSD-3-Clause) and contain no third-party CSS; they only reuse the
public class and custom-property names of GitHub's and GitLab's markup.
