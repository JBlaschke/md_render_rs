# Third-party notices

md-render is licensed under the BSD 3-Clause License (see [`LICENSE`](LICENSE)). It contains code
ported from, and data taken from, the projects listed here. Each keeps its own license.

Every section says what md-render uses, where it is in this tree, the upstream license, and the
copyright line exactly as it appears in the upstream license file (checked against the upstream
files on 2026-10-02). The full license texts follow at the end, each given once:
[MIT](#mit-license), [GitLab's MIT Expat license](#gitlab-license), [BSD 2-Clause (Pygments)](#bsd-2-clause-license-pygments),
[BSD 2-Clause (Ruby)](#bsd-2-clause-license-ruby), [TextMate bundles](#textmate-bundle-license),
[Unicode](#unicode-license-v3) and [Apache 2.0](#apache-license-20).

The Rust crates that are compiled into the `md-render` binary are listed separately, in
[`docs/dependency-licenses.md`](docs/dependency-licenses.md) (see
[Rust dependencies](#rust-dependencies)).

| Upstream | Used for | License |
|---|---|---|
| [TarFS.jl](#tarfsjl) | the `tarfs` crate | BSD-3-Clause |
| [grip](#grip) | command-line behaviour and messages | MIT |
| [path-and-address](#path-and-address) | positional PATH/ADDRESS parsing | MIT |
| [Primer Octicons](#primer-octicons) | icons | MIT |
| [GitHub Linguist](#github-linguist) | GitHub language table | MIT |
| [Primer github-syntax-light](#primer-github-syntax-light) | GitHub `pl-*` class map | MIT |
| [language-shellscript](#language-shellscript) | shell-session grammar | MIT |
| [diff.tmbundle](#difftmbundle) | diff grammar | TextMate bundle license |
| [gemoji](#gemoji) | GitHub emoji table | MIT |
| [Rouge](#rouge) | GitLab language registry, lexer ports, word tables | MIT (and BSD-2-Clause for its Pygments-derived parts) |
| [gitlab-glfm-markdown](#gitlab-glfm-markdown) | GitLab's comrak formatter | MIT Expat |
| [GitLab](#gitlab) | GitLab filters, constants, emoji aliases, client behaviour | MIT Expat |
| [tanuki_emoji, Gemojione, Unicode](#tanuki_emoji-gemojione-and-unicode-emoji-data) | GitLab emoji table | MIT, Unicode-3.0 |
| [Sanitize](#sanitize) | GitLab sanitizer rules | MIT |
| [html-pipeline](#html-pipeline) | GitLab sanitizer allowlist | MIT |
| [Addressable](#addressable) | GitLab link sanitizer URI rules | Apache-2.0 |
| [Ruby `uri`](#ruby-uri) | GitLab external-link URI rules | BSD-2-Clause |
| [two-face and bat](#two-face-and-bat-syntax-definitions) | syntax definitions | MIT OR Apache-2.0, plus each syntax's own license |

---------------------------------------------------------------------------------------------------

## Ported code and embedded data

### TarFS.jl

* **Used for:** `crates/tarfs` is a Rust port of [TarFS.jl](https://github.com/JBlaschke/TarFS.jl).
  Its reader and writer logic, the USTAR header layout (`crates/tarfs/src/ustar.rs`), its
  documentation and its test cases were translated from that project, and
  `crates/tarfs/tests/fixtures/tarfs_jl.tar` is TarFS.jl's output.
* **License:** BSD-3-Clause. TarFS.jl has the same author and the same license text as
  md-render, so [`LICENSE`](LICENSE) covers it.
* **Copyright:** `Copyright (c) 2026, Johannes Blaschke`

### grip

* **Used for:** `src/cli/**` reproduces the command-line grammar, option semantics and messages
  of [grip](https://github.com/joeyespo/grip) 4.6.2, and the hash navigation that falls back to
  the `user-content-` prefix (`assets/client/common.js`) follows grip's `scrollToHash`. No Python
  code was copied.
* **License:** [MIT](#mit-license)
* **Copyright:** `Copyright (c) 2014-2022 Joe Esposito <joe@joeyespo.com>`

### path-and-address

* **Used for:** `src/cli/address.rs` ports `resolve`, `split_address`, `valid_hostname` and
  `valid_port` of [path-and-address](https://github.com/joeyespo/path-and-address) 2.0.1
  (`parsing.py`), which grip uses for its positional arguments.
* **License:** [MIT](#mit-license)
* **Copyright:** `Copyright (c) 2012-2016 Joe Esposito <joe@joeyespo.com>`

### Primer Octicons

* **Used for:** SVG path data of [Primer Octicons](https://github.com/primer/octicons), as
  github.com emits them:
  * `src/page/octicons.rs`: the 16 px `book`, `info`, `alert`, `stop`, `file`,
    `file-directory-fill`, `copy` and `check` icons of the page chrome and the code-block
    clipboard template;
  * `src/render/github/octicons.rs`: `link`, `info`, `light-bulb`, `report`, `alert` and `stop`
    in the GitHub markup (heading anchors, alerts), plus the markup of the enrichment loading
    spinner;
  * `src/render/api.rs`: the `link` icon added to `--api` output;
  * `assets/client/github.js` and `assets/client/gitlab.js`: the `copy` and `check` icons drawn
    when no template or icon sprite is available.
* **License:** [MIT](#mit-license)
* **Copyright:** `Copyright (c) 2026 GitHub Inc.`

### GitHub Linguist

* **Used for:** `data/github/linguist.json` holds the language names, aliases, file extensions
  and `tm_scope`s of `lib/linguist/languages.yml` from
  [github-linguist/linguist](https://github.com/github-linguist/linguist) at commit
  `5fbdfcb8133be2bed88bf3ce62b2335f50474525`. The lookup order in
  `src/render/highlight/pl/linguist.rs` follows Linguist's `Language[]`, `find_by_extension` and
  `FileBlob#extensions`. No grammar of Linguist is used.
* **License:** [MIT](#mit-license)
* **Copyright:** `Copyright (c) 2017 GitHub, Inc.`

### Primer github-syntax-light

* **Used for:** `PRETTYLIGHTS_MAP` in `src/render/highlight/pl/classes.rs` (the TextMate scope to
  `pl-*` class table) is `lib/github-light.map.json` of
  [primer/github-syntax-light](https://github.com/primer/github-syntax-light) ("GitHub Light
  v0.5.0", generated by primer/github-syntax-theme-generator).
* **License:** [MIT](#mit-license)
* **Copyright:** `Copyright (c) 2016 GitHub, Inc.` (the repository's `LICENSE`; the header of the
  generated `lib/github-light.css` reads `Copyright (c) 2012 - 2017 GitHub, Inc.`)

### language-shellscript

* **Used for:** `src/render/highlight/pl/session.rs` ports the two rules of the
  `text.shell-session` grammar (`grammars/shell-session.cson`) of
  [atom/language-shellscript](https://github.com/atom/language-shellscript), which Linguist uses
  for `console`, `shellsession` and `sh-session` blocks.
* **License:** [MIT](#mit-license). The package was derived from TextMate's shellscript bundle,
  which is under the [TextMate bundle license](#textmate-bundle-license); its `LICENSE.md`
  reproduces that notice.
* **Copyright:** `Copyright (c) 2014 GitHub Inc.`

### diff.tmbundle

* **Used for:** `src/render/highlight/pl/diff.rs` ports the line rules of the `source.diff`
  grammar of [textmate/diff.tmbundle](https://github.com/textmate/diff.tmbundle), the diff
  grammar Linguist uses.
* **License:** the [TextMate bundle license](#textmate-bundle-license), as stated in the
  bundle's `README.mdown`. The bundle names no copyright holder.

### gemoji

* **Used for:** `data/github/emoji.json` maps every alias of `db/emoji.json` of
  [github/gemoji](https://github.com/github/gemoji) (commit
  `fadaeaf1f1a9be82b321316a6c5502e43138b2f6`) to its emoji string. No emoji images are included:
  the 23 custom GitHub emoji images are downloaded by `md-render vendor`.
* **License:** [MIT](#mit-license)
* **Copyright:** `Copyright (c) 2019 GitHub, Inc.`

### Rouge

* **Used for:** [rouge-ruby/rouge](https://github.com/rouge-ruby/rouge) 5.1.0, the highlighter
  of gitlab.com:
  * `data/gitlab/rouge-registry.json` is `Rouge::Lexer.registry` (lexer tags and aliases) with
    each lexer's title and `*.ext` filename globs;
  * `src/render/highlight/rouge/diff.rs`, `console.rs` and `docker.rs` port the `Diff`,
    `ConsoleLexer` and `Docker` lexers;
  * `src/render/highlight/rouge/words.rs` copies the keyword, builtin and similar word tables
    and word rules of the `Ruby`, `YAML`, `Shell`, `Rust`, `Python`, `Javascript`, `Typescript`,
    `C`, `Cpp`, `SQL`, `Java` and `Go` lexers;
  * `src/render/highlight/rouge/classes.rs` uses Rouge's token short names
    (`lib/rouge/token.rb`) and per-lexer class rules. Its scope-to-class rules are md-render's
    own.
* **License:** [MIT](#mit-license). Rouge's `LICENSE` adds that many of its lexers are adaptations
  of [Pygments](https://pygments.org) lexers and reproduces the Pygments license, given below as
  [BSD 2-Clause (Pygments)](#bsd-2-clause-license-pygments) with the Pygments copyright line
  `Copyright (c) 2006-2012 by the respective authors (see AUTHORS file).` (the AUTHORS list is
  part of Rouge's `LICENSE` file).
* **Copyright:** `Copyright (c) 2012 Jeanine Adkisson.`

### gitlab-glfm-markdown

* **Used for:** `src/render/gitlab/formatter.rs` and `src/render/gitlab/glfm.rs` are ports, kept
  nearly verbatim, of `src/formatter.rs` and `src/glfm.rs` of the
  [gitlab-glfm-markdown](https://gitlab.com/gitlab-org/ruby/gems/gitlab-glfm-markdown) gem
  0.0.43, GitLab's comrak options and HTML formatter. The formatter overrides parts of comrak's
  HTML formatter; comrak itself is a Rust dependency (BSD-2-Clause, see
  [`docs/dependency-licenses.md`](docs/dependency-licenses.md)).
* **License:** [GitLab's MIT Expat license](#gitlab-license) (the gem's `LICENSE` is GitLab's
  license text; the gem is MIT as a whole).
* **Copyright:** `Copyright (c) 2011-present GitLab B.V.`

### GitLab

* **Used for:** code and data from [GitLab](https://gitlab.com/gitlab-org/gitlab) (FOSS edition,
  gitlabhq commit `8396d286bea7ec0d695b57dd93896a15b096642b`), only from outside the `doc/`,
  `ee/` and `jh/` directories:
  * `src/render/gitlab/**` re-implements GitLab's Banzai Markdown pipeline filter by filter
    (`lib/banzai/filter/*.rb`, `lib/banzai/pipeline/*.rb`, `lib/gitlab/front_matter.rb`,
    `lib/banzai/color_parser.rb`) and embeds its constants: sanitizer allowlists, regular
    expressions and message strings;
  * `src/render/gitlab/parse.rs` takes `HTML5_MAX_TREE_DEPTH` and `NESTING_TOO_DEEP_MESSAGE` from
    `config/initializers/html_pipeline_html5_patch.rb`;
  * `src/render/highlight/rouge/mod.rs` follows `Rouge::Formatters::HTMLGitlab`
    (`lib/rouge/formatters/html_gitlab.rb`) for the per-line code markup;
  * `data/gitlab/emoji.json` includes `fixtures/emojis/aliases.json`;
  * `assets/client/gitlab.js` re-implements GitLab's client-side Markdown behaviour
    (`app/assets/javascripts/behaviors/markdown/`: KaTeX rendering with GitLab's options and
    limits, mermaid, the copy-code button, hash navigation).
* **License:** [GitLab's MIT Expat license](#gitlab-license). GitLab's client-side JavaScript is
  MIT Expat in every directory.
* **Copyright:** `Copyright (c) 2011-present GitLab Inc.`

### tanuki_emoji, Gemojione and Unicode emoji data

* **Used for:** `data/gitlab/emoji.json` holds the names, emoji strings, descriptions and Unicode
  versions of GitLab's emoji database (`https://gitlab.com/-/emojis/4/emojis.json`). GitLab
  generates that database with the
  [tanuki_emoji](https://gitlab.com/gitlab-org/ruby/gems/tanuki_emoji) gem. Its names keep
  compatibility with [Gemojione](https://github.com/bonusly/gemojione) 3.3.0, and its emoji and
  descriptions come from the Unicode emoji data files. No emoji images are included.
* **Licenses and copyright:**
  * tanuki_emoji: [MIT](#mit-license), `Copyright (c) 2021 GitLab B.V.`
  * Gemojione: [MIT](#mit-license), `Copyright (c) 2013 Jonathan Wiesel` (its `LICENSE.txt` adds
    "Original work by Steve Klabnik on emoji gem")
  * Unicode emoji data: [Unicode License V3](#unicode-license-v3), `Copyright © 1991-2026 Unicode, Inc.`

### Sanitize

* **Used for:** `src/render/gitlab/sanitize.rs` reproduces the default configuration, the
  traversal (`Sanitize#traverse`), `REGEX_PROTOCOL` and the `CleanElement` and `CleanComment`
  transformers of [Sanitize](https://github.com/rgrove/sanitize) 7.0.0, which GitLab's sanitizer
  runs on.
* **License:** [MIT](#mit-license)
* **Copyright:** `Copyright Ryan Grove <ryan@wonko.com>`

### html-pipeline

* **Used for:** `src/render/gitlab/sanitize.rs` embeds the `ALLOWLIST` of
  `lib/html/pipeline/sanitization_filter.rb` of
  [html-pipeline](https://github.com/gjtorikian/html-pipeline) 2.14.3, the base of GitLab's
  sanitization filter.
* **License:** [MIT](#mit-license)
* **Copyright:** `Copyright (c) 2012 GitHub Inc. and Jerry Cheung`

### Addressable

* **Used for:** GitLab's link sanitizer and external-link filter parse URLs with
  [Addressable](https://github.com/sporkmonger/addressable) (GitLab locks 2.9.0). To give the
  same answers, md-render contains Rust translations of parts of `lib/addressable/uri.rb`:
  * `src/render/gitlab/sanitize.rs`: the `URIREGEX` split and validation, `NORMPATH`,
    `normalize_component` with the `PCHAR` character class, `normalized_path` and the
    `normalize_path` dot-segment loop;
  * `src/render/gitlab/filters.rs`: the `normalized_host` IDNA check, `hostname`, and the
    empty-path rule of `normalized_path`.
* **Changes:** these parts were rewritten in Rust and reduced to the checks GitLab's filters
  need. They are not a complete URI implementation, and their behaviour was checked against
  Addressable 2.8.7.
* **License:** [Apache License 2.0](#apache-license-20). Addressable has no `NOTICE` file.
* **Copyright:** `Copyright (C) Bob Aman` (the header of `lib/addressable/uri.rb`; the
  repository's `LICENSE.txt` is the plain Apache license text).

### Ruby uri

* **Used for:** GitLab's external-link filter calls `URI.parse(url).to_s` from Ruby's
  [uri](https://github.com/ruby/uri) library (GitLab locks 1.1.1). `src/render/gitlab/filters.rs`
  translates the parts that decide whether a URL is valid and how it is printed: the RFC 3986
  parser's validation, the `URI::MailTo` address pattern, validation and `to_s`, the `URI::FTP`
  absolute-path rule, and the removal of an empty userinfo.
* **License:** uri is available under Ruby's license or the BSD 2-Clause license. md-render uses
  it under the [BSD 2-Clause license (Ruby)](#bsd-2-clause-license-ruby).
* **Copyright:** `Copyright (C) 1993-2013 Yukihiro Matsumoto. All rights reserved.` (the
  `uri/mailto.rb` and `uri/ftp.rb` sources name Akira Yamada as their author).

### two-face and bat syntax definitions

* **Used for:** the syntax highlighter (`src/render/highlight/engine.rs`) tokenizes code with
  `two_face::syntax::extra_newlines()`, a compiled set of Sublime Text syntax definitions that
  the [two-face](https://codeberg.org/CosmicHarper/two-face) crate (0.5.2+bat-0.26.1) embeds.
  two-face takes the syntax definitions, and the code that builds the set, from
  [bat](https://github.com/sharkdp/bat).
* **License:** two-face and bat are each available under the MIT license or the Apache License
  2.0, at your option ([MIT](#mit-license), [Apache 2.0](#apache-license-20)). Every embedded
  syntax definition keeps its own license. Their license texts are collected in
  [`docs/licenses/two-face-acknowledgements.md`](docs/licenses/two-face-acknowledgements.md), a
  copy of two-face's `generated/acknowledgements_full.md` for the version md-render uses.
* **Copyright:**
  * two-face: ``Copyright (c) 2023-2025 The `two-face` developers (https://codeberg.org/CosmicHarper/two-face).``
  * bat: `Copyright (c) 2018-2023 bat-developers (https://github.com/sharkdp/bat).`

---------------------------------------------------------------------------------------------------

## md-render's own material

These need no notice beyond [`LICENSE`](LICENSE):

* The fallback stylesheets `assets/fallback/github.css` and `assets/fallback/gitlab.css` are
  hand-written for md-render. They contain no third-party CSS and only reuse the public class
  names and custom-property names of GitHub's and GitLab's markup.
* Markup shapes, allowlists and lists that md-render reproduces from what github.com and
  gitlab.com return (for example GitHub's raw-HTML allowlist, its 26 `<g-emoji>` names and its
  page structure) were observed from the sites' output. They are facts about the output, not
  copied code.
* Several algorithms are re-implemented from their documented or observed behaviour, without
  copying code: github-slugger's heading slugs, Ruby Psych's YAML scalar resolution,
  cmark-gfm's autolink rules and gumbo's tree-depth limit.

## Rust dependencies

The `md-render` binary statically links about 250 Rust crates; a few dozen more run only at
build time. Most are under MIT or Apache-2.0 (often `MIT OR Apache-2.0`) or Unicode-3.0, and a
few under ISC, Zlib, BSD, MPL-2.0 or CDLA-Permissive-2.0. Their licenses ask for the copyright
and license text to come with the binary, so a binary release has to ship them.
[`docs/dependency-licenses.md`](docs/dependency-licenses.md) lists every crate with its version,
license and source. [`docs/dependency-licenses.py`](docs/dependency-licenses.py) generates it from
`cargo metadata`, and the page explains how to regenerate it and how to produce the full
license texts for a release (for example with `cargo about`).

* **MPL-2.0:** `cssparser`, `dtoa-short` and `selectors` (used through `dom_query`) and
  `option-ext` (used through `directories`) are under the Mozilla Public License 2.0, as is the
  build-time macro crate `cssparser-macros`. md-render does not modify them. Their source code
  is available from crates.io and from the repositories that
  [`docs/dependency-licenses.md`](docs/dependency-licenses.md) links.
* **CDLA-Permissive-2.0:** `webpki-roots` and `webpki-root-certs` (used through `ureq` and
  `rustls-platform-verifier`) contain Mozilla's list of root certificates as data under the
  Community Data License Agreement, Permissive 2.0.

## Downloaded at run time, not distributed

md-render does not ship the site stylesheets, fonts, images, icon sprites or JavaScript
libraries that pages use. `md-render vendor` downloads them on the user's machine from
github.com, gitlab.com and the jsDelivr npm mirror (MathJax 3.2.0, KaTeX 0.16.47, mermaid
11.16.1 and 11.17.2). The style bundle's `manifest.json` records every source URL. A standalone
HTML file written by `md-render export` embeds the files the page needs, so their own terms apply
when such a file is passed on.

---------------------------------------------------------------------------------------------------

## License texts

### MIT License

This is the MIT license text of every section marked [MIT](#mit-license), to be read with that
section's copyright line. Upstream files wrap the lines differently, and Sanitize's quotes the
word 'Software' with single quotes; the wording is the same.

```text
Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in
all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
THE SOFTWARE.
```

### GitLab license

The license file of GitLab and of the gitlab-glfm-markdown gem. The gem's copy begins with
`Copyright (c) 2011-present GitLab B.V.`, GitLab's with `Copyright (c) 2011-present GitLab Inc.`
Everything after the first line is identical:

```text
Portions of this software are licensed as follows:

* All content residing under the "doc/" directory of this repository is licensed under "Creative Commons: CC BY-SA 4.0 license".
* All content that resides under the "ee/" directory of this repository, if that directory exists, is licensed under the license defined in "ee/LICENSE".
* All content that resides under the "jh/" directory of this repository, if that directory exists, is licensed under the license defined in "jh/LICENSE".
* All client-side JavaScript (when served directly or after being compiled, arranged, augmented, or combined), is licensed under the "MIT Expat" license.
* All third party components incorporated into the GitLab Software are licensed under the original license provided by the owner of the applicable component.
* Content outside of the above mentioned directories or restrictions above is available under the "MIT Expat" license as defined below.

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

### BSD 2-Clause License (Pygments)

As reproduced in Rouge's `LICENSE`:

```text
Copyright (c) 2006-2012 by the respective authors (see AUTHORS file).
All rights reserved.

Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions are
met:

* Redistributions of source code must retain the above copyright
  notice, this list of conditions and the following disclaimer.

* Redistributions in binary form must reproduce the above copyright
  notice, this list of conditions and the following disclaimer in the
  documentation and/or other materials provided with the distribution.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS
"AS IS" AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT
LIMITED TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR
A PARTICULAR PURPOSE ARE DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT
OWNER OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL,
SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT
LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES; LOSS OF USE,
DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON ANY
THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT
(INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE
OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
```

### BSD 2-Clause License (Ruby)

The `BSDL` file of Ruby's uri library:

```text
Copyright (C) 1993-2013 Yukihiro Matsumoto. All rights reserved.

Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions
are met:
1. Redistributions of source code must retain the above copyright
   notice, this list of conditions and the following disclaimer.
2. Redistributions in binary form must reproduce the above copyright
   notice, this list of conditions and the following disclaimer in the
   documentation and/or other materials provided with the distribution.

THIS SOFTWARE IS PROVIDED BY THE AUTHOR AND CONTRIBUTORS ``AS IS'' AND
ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE
IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE
ARE DISCLAIMED.  IN NO EVENT SHALL THE AUTHOR OR CONTRIBUTORS BE LIABLE
FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL
DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS
OR SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION)
HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT
LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY
OUT OF THE USE OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF
SUCH DAMAGE.
```

### TextMate bundle license

From the `README.mdown` of textmate/diff.tmbundle (and quoted in atom/language-shellscript's
`LICENSE.md`):

```text
If not otherwise specified (see below), files in this repository fall under the following license:

Permission to copy, use, modify, sell and distribute this
software is granted. This software is provided "as is" without
express or implied warranty, and with no claim as to its
suitability for any purpose.

An exception is made for files in readable text which contain their own license information, or files where an accompanying file exists (in the same directory) with a “-license” suffix added to the base-name name of the original file, and an extension of txt, html, or similar. For example “tidy” is accompanied by “tidy-license.txt”.
```

### Unicode License V3

```text
UNICODE LICENSE V3

COPYRIGHT AND PERMISSION NOTICE

Copyright © 1991-2026 Unicode, Inc.

NOTICE TO USER: Carefully read the following legal agreement. BY
DOWNLOADING, INSTALLING, COPYING OR OTHERWISE USING DATA FILES, AND/OR
SOFTWARE, YOU UNEQUIVOCALLY ACCEPT, AND AGREE TO BE BOUND BY, ALL OF THE
TERMS AND CONDITIONS OF THIS AGREEMENT. IF YOU DO NOT AGREE, DO NOT
DOWNLOAD, INSTALL, COPY, DISTRIBUTE OR USE THE DATA FILES OR SOFTWARE.

Permission is hereby granted, free of charge, to any person obtaining a
copy of data files and any associated documentation (the "Data Files") or
software and any associated documentation (the "Software") to deal in the
Data Files or Software without restriction, including without limitation
the rights to use, copy, modify, merge, publish, distribute, and/or sell
copies of the Data Files or Software, and to permit persons to whom the
Data Files or Software are furnished to do so, provided that either (a)
this copyright and permission notice appear with all copies of the Data
Files or Software, or (b) this copyright and permission notice appear in
associated Documentation.

THE DATA FILES AND SOFTWARE ARE PROVIDED "AS IS", WITHOUT WARRANTY OF ANY
KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF
MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT OF
THIRD PARTY RIGHTS.

IN NO EVENT SHALL THE COPYRIGHT HOLDER OR HOLDERS INCLUDED IN THIS NOTICE
BE LIABLE FOR ANY CLAIM, OR ANY SPECIAL INDIRECT OR CONSEQUENTIAL DAMAGES,
OR ANY DAMAGES WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS,
WHETHER IN AN ACTION OF CONTRACT, NEGLIGENCE OR OTHER TORTIOUS ACTION,
ARISING OUT OF OR IN CONNECTION WITH THE USE OR PERFORMANCE OF THE DATA
FILES OR SOFTWARE.

Except as contained in this notice, the name of a copyright holder shall
not be used in advertising or otherwise to promote the sale, use or other
dealings in these Data Files or Software without prior written
authorization of the copyright holder.
```

### Apache License 2.0

The `LICENSE.txt` of Addressable (the standard Apache License 2.0 text):

```text
                              Apache License
                        Version 2.0, January 2004
                     http://www.apache.org/licenses/

TERMS AND CONDITIONS FOR USE, REPRODUCTION, AND DISTRIBUTION

1. Definitions.

   "License" shall mean the terms and conditions for use, reproduction,
   and distribution as defined by Sections 1 through 9 of this document.

   "Licensor" shall mean the copyright owner or entity authorized by
   the copyright owner that is granting the License.

   "Legal Entity" shall mean the union of the acting entity and all
   other entities that control, are controlled by, or are under common
   control with that entity. For the purposes of this definition,
   "control" means (i) the power, direct or indirect, to cause the
   direction or management of such entity, whether by contract or
   otherwise, or (ii) ownership of fifty percent (50%) or more of the
   outstanding shares, or (iii) beneficial ownership of such entity.

   "You" (or "Your") shall mean an individual or Legal Entity
   exercising permissions granted by this License.

   "Source" form shall mean the preferred form for making modifications,
   including but not limited to software source code, documentation
   source, and configuration files.

   "Object" form shall mean any form resulting from mechanical
   transformation or translation of a Source form, including but
   not limited to compiled object code, generated documentation,
   and conversions to other media types.

   "Work" shall mean the work of authorship, whether in Source or
   Object form, made available under the License, as indicated by a
   copyright notice that is included in or attached to the work
   (an example is provided in the Appendix below).

   "Derivative Works" shall mean any work, whether in Source or Object
   form, that is based on (or derived from) the Work and for which the
   editorial revisions, annotations, elaborations, or other modifications
   represent, as a whole, an original work of authorship. For the purposes
   of this License, Derivative Works shall not include works that remain
   separable from, or merely link (or bind by name) to the interfaces of,
   the Work and Derivative Works thereof.

   "Contribution" shall mean any work of authorship, including
   the original version of the Work and any modifications or additions
   to that Work or Derivative Works thereof, that is intentionally
   submitted to Licensor for inclusion in the Work by the copyright owner
   or by an individual or Legal Entity authorized to submit on behalf of
   the copyright owner. For the purposes of this definition, "submitted"
   means any form of electronic, verbal, or written communication sent
   to the Licensor or its representatives, including but not limited to
   communication on electronic mailing lists, source code control systems,
   and issue tracking systems that are managed by, or on behalf of, the
   Licensor for the purpose of discussing and improving the Work, but
   excluding communication that is conspicuously marked or otherwise
   designated in writing by the copyright owner as "Not a Contribution."

   "Contributor" shall mean Licensor and any individual or Legal Entity
   on behalf of whom a Contribution has been received by Licensor and
   subsequently incorporated within the Work.

2. Grant of Copyright License. Subject to the terms and conditions of
   this License, each Contributor hereby grants to You a perpetual,
   worldwide, non-exclusive, no-charge, royalty-free, irrevocable
   copyright license to reproduce, prepare Derivative Works of,
   publicly display, publicly perform, sublicense, and distribute the
   Work and such Derivative Works in Source or Object form.

3. Grant of Patent License. Subject to the terms and conditions of
   this License, each Contributor hereby grants to You a perpetual,
   worldwide, non-exclusive, no-charge, royalty-free, irrevocable
   (except as stated in this section) patent license to make, have made,
   use, offer to sell, sell, import, and otherwise transfer the Work,
   where such license applies only to those patent claims licensable
   by such Contributor that are necessarily infringed by their
   Contribution(s) alone or by combination of their Contribution(s)
   with the Work to which such Contribution(s) was submitted. If You
   institute patent litigation against any entity (including a
   cross-claim or counterclaim in a lawsuit) alleging that the Work
   or a Contribution incorporated within the Work constitutes direct
   or contributory patent infringement, then any patent licenses
   granted to You under this License for that Work shall terminate
   as of the date such litigation is filed.

4. Redistribution. You may reproduce and distribute copies of the
   Work or Derivative Works thereof in any medium, with or without
   modifications, and in Source or Object form, provided that You
   meet the following conditions:

   (a) You must give any other recipients of the Work or
       Derivative Works a copy of this License; and

   (b) You must cause any modified files to carry prominent notices
       stating that You changed the files; and

   (c) You must retain, in the Source form of any Derivative Works
       that You distribute, all copyright, patent, trademark, and
       attribution notices from the Source form of the Work,
       excluding those notices that do not pertain to any part of
       the Derivative Works; and

   (d) If the Work includes a "NOTICE" text file as part of its
       distribution, then any Derivative Works that You distribute must
       include a readable copy of the attribution notices contained
       within such NOTICE file, excluding those notices that do not
       pertain to any part of the Derivative Works, in at least one
       of the following places: within a NOTICE text file distributed
       as part of the Derivative Works; within the Source form or
       documentation, if provided along with the Derivative Works; or,
       within a display generated by the Derivative Works, if and
       wherever such third-party notices normally appear. The contents
       of the NOTICE file are for informational purposes only and
       do not modify the License. You may add Your own attribution
       notices within Derivative Works that You distribute, alongside
       or as an addendum to the NOTICE text from the Work, provided
       that such additional attribution notices cannot be construed
       as modifying the License.

   You may add Your own copyright statement to Your modifications and
   may provide additional or different license terms and conditions
   for use, reproduction, or distribution of Your modifications, or
   for any such Derivative Works as a whole, provided Your use,
   reproduction, and distribution of the Work otherwise complies with
   the conditions stated in this License.

5. Submission of Contributions. Unless You explicitly state otherwise,
   any Contribution intentionally submitted for inclusion in the Work
   by You to the Licensor shall be under the terms and conditions of
   this License, without any additional terms or conditions.
   Notwithstanding the above, nothing herein shall supersede or modify
   the terms of any separate license agreement you may have executed
   with Licensor regarding such Contributions.

6. Trademarks. This License does not grant permission to use the trade
   names, trademarks, service marks, or product names of the Licensor,
   except as required for reasonable and customary use in describing the
   origin of the Work and reproducing the content of the NOTICE file.

7. Disclaimer of Warranty. Unless required by applicable law or
   agreed to in writing, Licensor provides the Work (and each
   Contributor provides its Contributions) on an "AS IS" BASIS,
   WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or
   implied, including, without limitation, any warranties or conditions
   of TITLE, NON-INFRINGEMENT, MERCHANTABILITY, or FITNESS FOR A
   PARTICULAR PURPOSE. You are solely responsible for determining the
   appropriateness of using or redistributing the Work and assume any
   risks associated with Your exercise of permissions under this License.

8. Limitation of Liability. In no event and under no legal theory,
   whether in tort (including negligence), contract, or otherwise,
   unless required by applicable law (such as deliberate and grossly
   negligent acts) or agreed to in writing, shall any Contributor be
   liable to You for damages, including any direct, indirect, special,
   incidental, or consequential damages of any character arising as a
   result of this License or out of the use or inability to use the
   Work (including but not limited to damages for loss of goodwill,
   work stoppage, computer failure or malfunction, or any and all
   other commercial damages or losses), even if such Contributor
   has been advised of the possibility of such damages.

9. Accepting Warranty or Additional Liability. While redistributing
   the Work or Derivative Works thereof, You may choose to offer,
   and charge a fee for, acceptance of support, warranty, indemnity,
   or other liability obligations and/or rights consistent with this
   License. However, in accepting such obligations, You may act only
   on Your own behalf and on Your sole responsibility, not on behalf
   of any other Contributor, and only if You agree to indemnify,
   defend, and hold each Contributor harmless for any liability
   incurred by, or claims asserted against, such Contributor by reason
   of your accepting any such warranty or additional liability.

END OF TERMS AND CONDITIONS

APPENDIX: How to apply the Apache License to your work.

   To apply the Apache License to your work, attach the following
   boilerplate notice, with the fields enclosed by brackets "[]"
   replaced with your own identifying information. (Don't include
   the brackets!)  The text should be enclosed in the appropriate
   comment syntax for the file format. We also recommend that a
   file or class name and description of purpose be included on the
   same "printed page" as the copyright notice for easier
   identification within third-party archives.

Copyright [yyyy] [name of copyright owner]

Licensed under the Apache License, Version 2.0 (the "License");
you may not use this file except in compliance with the License.
You may obtain a copy of the License at

    http://www.apache.org/licenses/LICENSE-2.0

Unless required by applicable law or agreed to in writing, software
distributed under the License is distributed on an "AS IS" BASIS,
WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
See the License for the specific language governing permissions and
limitations under the License.
```
