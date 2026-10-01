# Third-party notices

md-render is BSD-3-Clause licensed (see `LICENSE`). It ports or embeds the following third-party
material, each under its own license. Each owner appends one section per source.

Styles, fonts and scripts downloaded by `md-render vendor` come from github.com / gitlab.com /
jsDelivr at run time on the user's machine; they are not distributed with md-render.

## cli: grip, path-and-address, Octicons

* `src/cli/address.rs` ports the positional path/address rules (`resolve`, `split_address`,
  `valid_hostname`, `valid_port`) of **path-and-address 2.0.1** (Copyright (c) Joe Esposito,
  MIT License, <https://github.com/joeyespo/path-and-address>).
* `src/cli/**` reproduces command-line semantics and messages of **grip 4.6.2**
  (Copyright (c) 2014-2022 Joe Esposito, MIT License, <https://github.com/joeyespo/grip>).
* `src/render/api.rs` embeds the `link` icon of **GitHub Octicons** (Copyright (c) GitHub Inc.,
  MIT License, <https://github.com/primer/octicons>).

MIT License text (applies to each of the above with its copyright line):

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
