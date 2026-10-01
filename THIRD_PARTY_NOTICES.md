# Third-party notices

md-render is BSD-3-Clause licensed (see `LICENSE`). It ports or embeds the following third-party
material, each under its own license. Each owner appends one section per source.

Styles, fonts and scripts downloaded by `md-render vendor` come from github.com / gitlab.com /
jsDelivr at run time on the user's machine; they are not distributed with md-render.

## GitLab: gitlab-glfm-markdown formatter, Banzai filters, emoji data (render-gitlab)

Used in: `src/render/gitlab/**`, `data/gitlab/emoji.json`.

* `src/render/gitlab/formatter.rs` and `src/render/gitlab/glfm.rs` are ports (kept nearly
  verbatim) of `src/formatter.rs` and `src/glfm.rs` of the
  [gitlab-glfm-markdown](https://gitlab.com/gitlab-org/ruby/gems/gitlab-glfm-markdown) gem,
  version 0.0.43.
* The other files of `src/render/gitlab/` re-implement, filter by filter, GitLab's Banzai
  pipeline (`lib/banzai/filter/*.rb`, `lib/gitlab/front_matter.rb`, `lib/banzai/color_parser.rb`,
  `lib/rouge/formatters/html_gitlab.rb`, gitlab-org/gitlab @ `8396d286bea7`, content outside
  `doc/`, `ee/` and `jh/`, i.e. MIT Expat) and embed its constants (sanitizer allowlists, regular
  expressions, message strings).
* `data/gitlab/emoji.json` is a compacted copy of GitLab's emoji database
  (`https://gitlab.com/-/emojis/4/emojis.json`, generated from the
  [tanuki_emoji](https://gitlab.com/gitlab-org/ruby/gems/tanuki_emoji) gem, MIT,
  Copyright (c) 2021 GitLab B.V.; its emoji names and descriptions derive from gemojione (MIT,
  Copyright (c) 2013 Jonathan Wiesel), emojione-alpha-codes (MIT) and Unicode's emoji data
  files) and of `fixtures/emojis/aliases.json` of gitlab-org/gitlab (MIT Expat).

```
Copyright (c) 2011-present GitLab B.V.

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

The sanitizer in `src/render/gitlab/sanitize.rs` also reproduces the default configuration and
the element-cleaning rules of [Sanitize](https://github.com/rgrove/sanitize) 7.0.0 (MIT,
Copyright (c) Ryan Grove) and the `ALLOWLIST` of
[html-pipeline](https://github.com/gjtorikian/html-pipeline) 2.14.3 (MIT, Copyright (c) 2012
GitHub Inc. and Jerry Cheung); both are under the MIT license text above (with their own
copyright lines).
