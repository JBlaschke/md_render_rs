# md-render

md-render shows Markdown files in your browser the way GitHub or GitLab would show them, and
updates the page while you edit. It works like [grip](https://github.com/joeyespo/grip) and
accepts grip's command line, with three differences:

* **Offline by default.** md-render renders on your machine and sends nothing to GitHub or
  GitLab. It reproduces each site's HTML instead of asking the site's API for it. The API is
  still available with `--api`.
* **Two flavours.** `--flavor github` (the default) reproduces github.com's file view, and
  `--flavor gitlab` reproduces gitlab.com's. Each flavour uses that site's own stylesheets.
* **Live updates keep your place.** Only the blocks you changed are replaced. The scroll
  position, rendered math and diagrams, open `<details>` and loaded images stay as they were.

What else it does:

* GitHub: alerts (`> [!NOTE]`), math (`$…$`, `$$…$$`, ```` ```math ````), mermaid, footnotes,
  task lists, emoji shortcodes, heading anchors, front matter shown as a table, and syntax
  highlighting with GitHub's `pl-*` classes.
* GitLab: `::include{file=…}`, front matter, KaTeX math, mermaid, `<gl-emoji>`, wiki links,
  task lists, and Rouge-style highlighting with per-line spans.
* The site styles are downloaded once with `md-render vendor` into a single `.tar.gz`, and
  running servers read that file in place.
* Directories show their README or a file listing. Other files are served raw.
* `md-render export` writes one self-contained HTML file, or an HTML file plus an assets
  directory.

## Contents

* [Install](#install)
* [Quick start](#quick-start)
* [Command line](#command-line)
* [Flavours, modes, themes and layout](#flavours-modes-themes-and-layout)
* [The style bundle](#the-style-bundle)
* [Live reload](#live-reload)
* [Directories, raw files and security](#directories-raw-files-and-security)
* [Export](#export)
* [API mode](#api-mode)
* [CLI reference](#cli-reference)
* [Coming from grip](#coming-from-grip)
* [Differences from github.com and gitlab.com](#differences-from-githubcom-and-gitlabcom)
* [Development](#development)
* [License](#license)

## Install

You need Rust 1.88 or newer. From a checkout of this repository:

```sh
cargo install --path .          # installs `md-render` into ~/.cargo/bin
# or
cargo build --release           # binary at target/release/md-render
```

md-render needs no OpenSSL or other system libraries. HTTPS (used only by `vendor` and `--api`)
goes through rustls with the operating system's certificate store. rustls's `ring` backend
compiles a little C and assembly, so the build needs a C compiler, which most Rust setups already
have. md-render is not published on crates.io.

md-render is developed on macOS, and CI runs the test suite on Linux and macOS. File watching
uses each platform's native backend (FSEvents, inotify, ReadDirectoryChangesW) through the
`notify` crate. Windows has not been tested.

## Quick start

```sh
md-render vendor                 # once: download the GitHub and GitLab styles (about 6 MB)
md-render                        # serve ./ (its README, or a listing)
md-render README.md              # serve one file
md-render -b docs/guide.md       # serve a file and open the browser
md-render --flavor gitlab .      # GitLab flavour
md-render export README.md       # write README.html (self-contained)
```

Then open <http://localhost:6419/>. Every save in your editor updates the page.

Without `md-render vendor`, pages still render, but with a small built-in stylesheet. md-render
then prints the exact `md-render vendor …` command that fixes this. `--auto-vendor` downloads the
missing styles when the server starts.

## Command line

```
md-render [OPTIONS] [PATH] [ADDRESS]     serve PATH (default .) on ADDRESS (default localhost:6419)
md-render export [OPTIONS] [PATH] [OUT]  write a standalone HTML file
md-render vendor [OPTIONS]               download the site styles into the style bundle
md-render cache path | cache clear       show or delete the cached style bundle
```

**PATH** is a Markdown file, a directory, or `-` for stdin. **ADDRESS** is `HOST`, `HOST:PORT`,
`PORT`, `:PORT`, `[IPv6]` or `[IPv6]:PORT`. With a single positional argument, md-render follows
grip's rule: the argument is the ADDRESS only if it contains a port, and otherwise it is the
PATH.

| Command | Serves | Listens on |
|---|---|---|
| `md-render` | `./` | `localhost:6419` |
| `md-render 8080` | `./` | `localhost:8080` |
| `md-render docs/` | `docs/` | `localhost:6419` |
| `md-render localhost` | the file or directory `./localhost` | `localhost:6419` |
| `md-render README.md 0.0.0.0:6419` | `README.md` | every IPv4 interface (`[::]:6419`: every IPv6 one) |
| `md-render . [::1]:8080` | `./` | IPv6 loopback, port 8080 |
| `md-render ./8080` | the file `./8080` | `localhost:6419` |
| `md-render ./vendor` | the file `./vendor` (not the subcommand) | `localhost:6419` |
| `cat notes.md \| md-render -` | stdin, rendered once (no live updates) | `localhost:6419` |

Port `0` picks a free port. `localhost` binds both `127.0.0.1` and `::1`. If another program
already uses the port on either address, md-render stops with an error. It does not fall back
to serving only one of the two.

Exit codes: `0` success, `1` runtime error (file not found, port in use, export failed), `2`
usage error (bad option or address). Pressing Ctrl-C a second time during shutdown exits with
`130`.

Output: the server prints its URL and one line per change, plus warnings and failed requests
(404s for missing images, for example). `-q` prints only errors. `-v` adds debug messages and an
access line for every request, `-vv` traces md-render itself, and `-vvv` traces every library.
`RUST_LOG` overrides all of these.

Environment variables:

| Variable | Meaning |
|---|---|
| `MD_RENDER_FLAVOR` | default for `--flavor` (`github` or `gitlab`; `vendor` ignores it) |
| `MD_RENDER_STYLES` | default for `--styles` (the style bundle file) |
| `GITHUB_TOKEN`, `GH_TOKEN` | GitHub token for `--api` (github.com and `*.ghe.com`) |
| `GH_ENTERPRISE_TOKEN`, `GITHUB_ENTERPRISE_TOKEN` | token for a GitHub Enterprise Server `--api-url` |
| `GITLAB_TOKEN` | GitLab token for `--api` |
| `RUST_LOG` | log filter (overrides `-q`/`-v`) |

The [CLI reference](#cli-reference) below lists every option. `md-render --help`,
`md-render export --help` and `md-render vendor --help` show the same information.

## Flavours, modes, themes and layout

**Flavour** (`-f`/`--flavor github|gitlab`, or `MD_RENDER_FLAVOR`) chooses the site whose
rendering and styles md-render reproduces. GitLab also renders `.rmd` files as Markdown. Both
render `.md`, `.markdown`, `.mdown`, `.mkdn`, `.mkd` and `.mdwn`.

**Mode** (`--mode readme|comment`) chooses the context:

* `readme` (default) is the repository file view.
* `comment` is an issue or pull-request comment. `--user-content` is grip's name for it. On
  GitHub, task lists, alerts and math look the same in both modes, but some markup differs: for
  example, in comments the YAML front matter is not a table and soft line breaks become `<br>`.
  On GitLab, comments use the same renderer without `::include`, inside a note box.

**GitLab `::include`.** In readme mode, `::include{file=path}` pastes another file of the
repository, as on gitlab.com. md-render resolves the path against the including file (a leading
`/` means the served root). It reads only files inside the served root and never reads from
`.git/`. Letter case must match exactly, as on gitlab.com, even on case-insensitive file systems.
md-render allows at most 32 directives per document and 10 MiB of text in total. `http(s)://`
includes become links, as on gitlab.com. md-render also watches the included files, so editing
one updates the page.

**Theme** (`--theme`):

* `auto` (default) follows the operating system's light or dark setting.
* `light` and `dark` force one colour mode.
* For GitHub, `--theme` also accepts one of GitHub's 14 theme ids, which then sets the colour
  mode too: `light`, `light_high_contrast`, `light_colorblind`,
  `light_colorblind_high_contrast`, `light_tritanopia`, `light_tritanopia_high_contrast`,
  `dark`, `dark_high_contrast`, `dark_colorblind`, `dark_colorblind_high_contrast`,
  `dark_tritanopia`, `dark_tritanopia_high_contrast`, `dark_dimmed` and
  `dark_dimmed_high_contrast`.
* `--light-theme ID` and `--dark-theme ID` choose which GitHub themes `auto` switches between.
* By default only `light` and `dark` are vendored. Run `md-render vendor --flavor github
  --themes all` for the other 12. md-render warns when a requested theme is not in the bundle.
* GitLab has only `light` and `dark`.

**Layout** (`--layout readme|bare`). `readme` (default) draws the site's file box with the file
name in its header. `bare` shows only the rendered content. `--title TEXT` sets the page title
and replaces the file name in the box header, as grip does (the default is the file name).

**Extra CSS** (`--css URL|FILE`, repeatable) adds a stylesheet after the site CSS. A local file is
served by md-render and watched, so editing it restyles open pages. In exports, a local file is
embedded in the page.

## The style bundle

`md-render vendor` downloads the stylesheets of the real sites into one gzip-compressed tarball,
the *style bundle*.

* **Where it lives.** By default the bundle is `<cache dir>/md-render/styles.tar.gz`, which is
  `~/Library/Caches/md-render/styles.tar.gz` on macOS and `~/.cache/md-render/styles.tar.gz` on
  Linux. `md-render cache path` prints the path in use. `--styles FILE` or
  `MD_RENDER_STYLES=FILE` selects another bundle, for example one checked into a project.
  `vendor -o FILE` writes a bundle somewhere else.
* **What it downloads.** For GitHub, md-render scrapes the stylesheet links of
  `https://github.com/joeyespo/grip`. It keeps the light and dark theme CSS (or all 14 themes
  with `--themes all`) and the primer-primitives, primer, global, github, repository and code
  stylesheets.
  It also downloads the fonts and images those stylesheets reference, the 23 custom emoji
  images, MathJax 3.2.0 and mermaid 11.17.2. For GitLab, it scrapes
  `https://gitlab.com/gitlab-org/gitlab-foss`. It keeps the application CSS (light and dark),
  Tailwind, fonts, the white and dark highlight themes and the icon sprite, and downloads
  KaTeX 0.16.47 with its fonts and mermaid 11.16.1. The libraries come from the jsDelivr npm
  mirror at those exact versions. `--no-libs` skips the libraries and emoji images (math and
  mermaid then stay as source). `--github-source URL` and `--gitlab-source URL` scrape another
  page, for example a GitHub Enterprise or self-managed GitLab instance. The manifest inside the
  bundle records every URL, size, SHA-256 and failed download.
* **Updating.** Re-run `md-render vendor` whenever you want the current site styles. The sites
  rename their hashed CSS files on every deploy, so md-render never hard-codes them.
  `--flavor github` or `--flavor gitlab` re-vendors one flavour and keeps the other. md-render
  refuses to overwrite a file that is not an md-render bundle, so a typo in `-o` cannot destroy
  unrelated data.
* **Re-vendoring while servers run.** The bundle is replaced atomically: md-render writes a
  temporary file and renames it over the old one. Running servers (unless started with
  `--norefresh`) watch the bundle file, load the new one off the request path and swap the page
  stylesheets without a reload. A corrupt
  new bundle is ignored: the server keeps the old one and shows an error banner. If you start a
  server first and vendor later, the server picks up the new bundle within a few seconds.
* **Read in place.** Servers never unpack the bundle. The [`tarfs`](crates/tarfs/README.md)
  sub-crate, a Rust port of [TarFS.jl](https://github.com/JBlaschke/TarFS.jl), decompresses it
  into memory once and serves every file as a zero-copy slice. Decompression stops at 512 MiB,
  so a damaged or hostile bundle cannot exhaust memory. If one flavour in the bundle is
  damaged, md-render drops only that flavour and names the command that repairs it.
* **Fallback.** When the bundle is missing, or lacks the flavour being served, pages use a small
  hand-written stylesheet for that flavour, and md-render prints the `md-render vendor` command
  to run. md-render never downloads anything on its own unless you pass `--auto-vendor`.
* **Clearing.** `md-render cache clear` (grip: `--clear`) deletes md-render's cache directory. A
  bundle given with `--styles` that lives outside that directory is never deleted.

## Live reload

The page connects back to the server with Server-Sent Events. When a file changes:

1. The server's single file watcher reports it. md-render watches the directory that contains
   each file, not the file itself, so editors that save by writing a temporary file and renaming
   it (vim, JetBrains IDEs, VS Code's safe write), and deleting and re-creating a file, are
   handled. Events are debounced for 50 ms.
2. The server re-reads the file and renders it only if its SHA-256 changed, so `touch` and
   duplicate events cost nothing. All tabs that show the same document share one render. A file
   that suddenly reads as empty (some editors truncate before writing) is given 250 ms to
   receive its new content before the empty page is shown.
3. The page replaces only the top-level blocks whose content changed. It keeps the block at the
   top of the window at the same position, using the source line the block came from. Pages
   scrolled to the bottom stay at the bottom.

What else is watched:

* local images the document uses (they are reloaded in place),
* GitLab `::include` files,
* `--css` files and the style bundle,
* directory listings: new and deleted files show up, and the page switches to the README when
  one appears.

Errors show as a dismissible banner above the last good content. While an error is active, the
tab title starts with `(Error) `. A deleted file shows a notice and comes back when the file
is re-created. When the server restarts, open pages reload themselves and keep their scroll
position.

`--poll[=MS]` polls the files every MS milliseconds (default 300) instead of using operating
system notifications. Use it on network file systems, in containers and in VMs with shared
folders, where change events often do not arrive. md-render falls back to polling every second
on its own when the native watcher cannot be started. Write `--poll=500`, or put the address
before the option (`md-render . 8080 --poll`): `--poll 500` with a space is a usage error,
because the number would otherwise be read as the PATH or ADDRESS. `--norefresh` (alias
`--no-reload`) turns live updates off. Pages rendered from stdin are never updated.

**Many tabs.** Browsers open only about six HTTP/1.1 connections to one server (`host:port`), and
every open preview holds one for its event stream. Hidden tabs therefore close their stream after
5 seconds and reconnect, catching up on changes, when they are shown again. A tab opened in the
background does not connect until you look at it. With more than about five *visible* previews of
the same server at once (several windows side by side), further pages and images can still stall.
In that case, start a second server on another port.

## Directories, raw files and security

**Directories.** When PATH is a directory, `/` shows its README, or a file listing if it has
none. Every subdirectory works the same way (`/dir` redirects to `/dir/` so that relative links
resolve as on the sites). md-render finds a README case-insensitively with any Markdown
extension, preferring an exact `README.md`. For the served root, the GitHub flavour uses
GitHub's order: `.github/README.md`, then the root, then `docs/`. The GitLab flavour looks only
in the root. Listings show directories first, hide `.git` and symlinks that lead outside the
root, and update live.

**Raw files.** Markdown files are rendered. Every other file is served raw with its
`Content-Type`. Text types get `charset=utf-8`, so source files display in the browser instead of
being downloaded, and files without an extension are checked for binary content. Raw responses
support `Range` and `If-Modified-Since`. Append `?raw=1` to a Markdown URL to see its source.

**Security model.** md-render is meant for files you trust on your own machine. It still guards
against the attacks a local web server invites:

* **Network exposure.** The default address `localhost` binds only the loopback interfaces.
  Binding `0.0.0.0` or `[::]` makes every file under the served root readable from your network.
  md-render says so in its startup lines and prints the address other machines would use.
* **DNS rebinding.** On loopback, md-render answers only requests whose `Host` header (and
  absolute request URI) names `localhost`, `127.0.0.1`, `[::1]`, `*.localhost` or the host you
  bound, with the right port. Any other name gets `403`, so a web page cannot use a DNS name that
  resolves to 127.0.0.1 to read your files. Non-loopback binds check the `Host` header as well.
  When you reach the server through a name of its own (a reverse proxy, a tunnel, a Codespaces
  URL), allow that name with `--allow-host NAME`.
* **Served root.** Only files below the served root are served: the directory you named, or the
  directory of the file you named. `--root DIR` widens the root, for example to the repository
  top level, so that links like `/docs/x.md` work. md-render rejects request paths with `..`,
  backslashes or NUL bytes, and checks the resolved path against the canonical root. Symlinks
  that lead outside the root get `403` unless you pass `--follow-symlinks`. The one exception is
  the file you named on the command line: it is served even if it is a symlink to somewhere else.
  FIFOs, sockets and devices are never opened.
* **`.git` is never served** (in any letter case) and never listed, and GitLab `::include` cannot
  read from it.
* **Raw HTML, SVG and XML files** are sent with `Content-Security-Policy: sandbox allow-scripts
  allow-popups allow-forms`. Their scripts still run, but in an opaque origin, so an SVG or HTML
  file from an untrusted repository cannot read your other files through the preview server.
  Embedding such a file with `<img src="x.svg">` is not affected. The sandbox also blocks
  same-origin requests, CORS web fonts and module scripts in local HTML pages.
* **Rendered Markdown is sanitized** the way each site sanitizes it. GitHub's raw-HTML allowlist
  and GitLab's Sanitize configuration and link sanitizer remove `<script>`, event-handler
  attributes, `javascript:` links and the like.
* **Network access.** md-render itself contacts the network only for `md-render vendor`,
  `--auto-vendor` and `--api`. Your browser still loads any external images or links the
  document references.

## Export

```sh
md-render export README.md                  # README.html next to it, self-contained
md-render export docs/ site/index.html      # a directory exports its README
md-render export --no-inline README.md      # README.html + README_files/
md-render export README.md -                # HTML on stdout
cat notes.md | md-render export - -         # stdin to stdout
md-render --export README.md out.html       # grip's spelling
```

By default the export **inlines** everything the page needs: the site CSS in cascade order, the
fonts and images that CSS references (as `data:` URIs), md-render's scripts, and only the client
libraries the document uses (MathJax, KaTeX, mermaid). With the GitHub styles, a plain page is
about 1.2 MB, and a page with math and a mermaid diagram about 6 MB. Large images that the site
CSS references (rasters over 16 KiB, SVGs over 64 KiB, mostly page decoration) are left out.

`--no-inline` copies those files into a `<stem>_files/` directory next to the output and links to
them. It needs an output file, not stdout.

Images and links of the document itself are not embedded. They stay relative and are rewritten
when OUT is in a different directory than the source. Exports contain no live-update script and
no `data-sourcepos` attributes. The file is written atomically. An existing OUT keeps its
permissions, and if OUT is a symlink, its target is replaced. Any rendering or `--api` error
makes the export fail with exit code 1, so scripts can rely on it.

## API mode

`--api` sends the document to the site's Markdown API and shows the HTML it returns, adjusted
for the file view. Use it to compare md-render with the site, or to have references such as
`#123` linked (with `--context owner/repo`). Without `--api`, nothing leaves your machine.

| | GitHub | GitLab |
|---|---|---|
| Request | `POST https://api.github.com/markdown` with `{"text", "mode": "gfm", "context"}` | `POST https://gitlab.com/api/v4/markdown` with `{"text", "gfm": true, "project"}` |
| Other servers | `--api-url https://HOST/api/v3` (GitHub Enterprise Server) | `--api-url https://HOST` (self-managed) |
| Token | `--token`, else `GITHUB_TOKEN`, `GH_TOKEN`, then `gh auth token --hostname HOST` | `--token`, else `GITLAB_TOKEN`, then `glab config get token --host HOST` |
| Enterprise token | `GH_ENTERPRISE_TOKEN`, `GITHUB_ENTERPRISE_TOKEN`, then `gh auth token --hostname HOST` | (same as above) |
| Without a token | works, with a low rate limit (60 requests per hour) | gitlab.com answers `401` |

How md-render treats the request:

* The document text and `--context` go to the API host, together with `User-Agent:
  md-render/<version>` and the token. GitHub gets the token as `Authorization: Bearer`, GitLab as
  `PRIVATE-TOKEN`. In readme mode, GitHub-flavoured YAML front matter is rendered locally and not
  sent.
* A github.com token is never sent to a GitHub Enterprise Server. Redirects are never followed,
  so a token cannot leak to another host. Tokens are never printed or logged, and
  `--api-url https://user:token@host` is refused. Prefer the environment variables over `--token`,
  because other users can see command-line arguments.
* Responses are cached in memory by a hash of the text, so saving unchanged content or opening
  more tabs does not repeat a request. GitHub rejects documents over 400 KiB, and md-render does
  not send them.
* **Fallback.** When the API fails while serving (rate limit, bad token, network error, a
  redirect), md-render renders offline instead and shows the reason in a banner. It then stops
  calling the API for a while: for 15 s after a network error, until the reset time after a rate
  limit, and for the rest of the session after a rejected token. `md-render export --api` fails
  instead of falling back.
* `--context`, `--api-url` and `--token` have no effect without `--api`, and md-render warns when
  you pass them alone.

## CLI reference

These are the options of md-render 0.1.0, collected from its `--help` output. Global options
work before or after a subcommand.

**Rendering** (all commands)

| Option | Meaning |
|---|---|
| `-f`, `--flavor github\|gitlab` | site to reproduce (default `$MD_RENDER_FLAVOR`, else `github`; `vendor` also takes `all`, its default) |
| `--mode readme\|comment` | file view (default) or comment body |
| `--user-content` | same as `--mode comment` (grip) |
| `--context REPO` | `owner/repo` or `group/project` sent with `--api` |
| `--title TITLE` | page title (default: the file name) |
| `--theme THEME` | `auto` (default), `light`, `dark`, or a GitHub theme id |
| `--light-theme ID`, `--dark-theme ID` | GitHub themes used in light and dark mode (default `light`, `dark`) |
| `--layout readme\|bare` | site-like file box (default) or content only |

**Styles and API** (all commands)

| Option | Meaning |
|---|---|
| `--styles FILE` | style bundle (default `$MD_RENDER_STYLES`, else `<cache dir>/md-render/styles.tar.gz`) |
| `--css URL\|FILE` | extra stylesheet after the site CSS; repeatable |
| `--api` | render through the site's Markdown API |
| `--api-url URL` | API base URL (GitHub Enterprise: `https://HOST/api/v3`; GitLab: `https://HOST`) |
| `--token TOKEN` | API token (prefer the environment variables) |

**Output** (all commands)

| Option | Meaning |
|---|---|
| `-q`, `--quiet` | print only errors |
| `-v`, `--verbose` | more output; repeat for more (`-vv`, `-vvv`) |
| `-h`, `--help` / `-V`, `--version` | help / version |

**Serving** (`md-render [OPTIONS] [PATH] [ADDRESS]`)

| Option | Meaning |
|---|---|
| `-b`, `--browser` | open the page once the server listens |
| `--norefresh` (alias `--no-reload`) | no live updates |
| `--poll[=MS]` | poll for changes every MS ms (default 300) instead of OS notifications |
| `--root DIR` | serve DIR (it must contain PATH), e.g. the repository root |
| `--follow-symlinks` | serve files whose symlinks point outside the root |
| `--allow-host NAME` | also answer to the host name NAME (non-loopback binds behind a proxy or tunnel) |
| `--auto-vendor` | download the flavour's styles at startup when the bundle lacks them |

**grip compatibility**

| Option | Meaning |
|---|---|
| `--export [PATH] [OUT]` | same as `md-render export` (OUT `-` = stdout) |
| `--no-inline` | with `--export`: link the styles from a directory instead of inlining them |
| `--clear` | same as `md-render cache clear` |

**`md-render export [OPTIONS] [PATH] [OUT]`**: PATH defaults to `.` (a directory exports its
README), OUT to PATH with the extension `.html`, and `-` means stdin or stdout. `--no-inline`
writes `<stem>_files/` instead of inlining.

**`md-render vendor [OPTIONS]`**

| Option | Meaning |
|---|---|
| `-f`, `--flavor github\|gitlab\|all` | flavours to vendor (default `all`; `MD_RENDER_FLAVOR` does not apply) |
| `-o`, `--output FILE` | bundle to write (default `--styles`, `$MD_RENDER_STYLES`, or the cache bundle) |
| `--themes default\|all` | GitHub light and dark only (default), or all 14 GitHub themes |
| `--no-libs` | skip MathJax, KaTeX, mermaid and the emoji images |
| `--github-source URL` | page scraped for GitHub stylesheets (default `https://github.com/joeyespo/grip`) |
| `--gitlab-source URL` | page scraped for GitLab stylesheets (default `https://gitlab.com/gitlab-org/gitlab-foss`) |

**`md-render cache path`** prints the bundle in use. **`md-render cache clear`** deletes the cache
directory.

A subcommand combined with serve-only options or positionals is a usage error with a hint, for
example `'--browser' cannot be used with the 'export' subcommand`. `vendor` and `cache` warn about
rendering and API options they ignore.

## Coming from grip

md-render keeps grip's command-line grammar, so most grip commands work unchanged.

| grip | md-render | |
|---|---|---|
| `grip [<path>] [<address>]`, path-and-address rules | same, plus IPv6 (`[::1]:8080`) | kept |
| default `localhost:6419`, port `0` = random | same; `localhost` binds IPv4 and IPv6 | kept |
| `-` reads stdin | same; "Reading Markdown from stdin" hint on a terminal | kept |
| `-b`/`--browser`, `--title`, `--norefresh`, `--quiet`, `-V`, `-h` | same (`--no-reload` alias; `--quiet` is fully quiet) | kept |
| `--user-content`, `--context` | same (`--mode comment`); `--context` only matters with `--api` | kept |
| `--export [<path>] [<out>]`, `--no-inline` | same, plus `md-render export`; fonts and images inlined too, atomic write, exit 1 on any error | kept |
| `--clear` | same as `md-render cache clear` | kept |
| `--api-url` | same, for GitHub Enterprise and self-managed GitLab | kept |
| renders through the GitHub API | renders offline; `--api` opts in to the API | changed |
| `--user`/`--pass` (basic auth) | usage error pointing to `--token` and `GITHUB_TOKEN` (GitHub dropped password authentication) | changed |
| `-a`/`-p`: hint to use the positional form, exit 2 | same | kept |
| invalid address: warning, then serve anyway | usage error, exit 2 | changed |
| `--theme light\|dark` | `auto` (default), `light`, `dark` and 14 GitHub theme ids | changed |
| styles scraped from github.com on first request, cached per version | `md-render vendor` writes a bundle; the server never downloads styles itself | changed |
| directory without a README: error | file listing | changed |
| `README`/`Readme`/`readme` with `.md`/`.markdown` | case-insensitive, six extensions (plus `.rmd` for GitLab), GitHub's `.github/` and `docs/` fallbacks | changed |
| other text files rendered as Markdown | served raw with their own content type | changed |
| polls each file every 0.3 s per tab | one OS file watcher; `--poll[=MS]` for network file systems | changed |
| page replaced on every change | only changed blocks replaced; scroll position kept | changed |
| page title `<file> - Grip` | `<file>` | changed |
| `--wide` | accepted, warns that it has no effect (it was a no-op in grip as well); see `--layout` | dropped |
| `~/.grip/settings.py`, `GRIPHOME`, `GRIPURL` | no configuration file; `MD_RENDER_FLAVOR`, `MD_RENDER_STYLES` | dropped |
| Python API (`grip.serve`, `grip.export`) | the `md-render` library crate (`md_render::render`, `export`, `server`) | changed |

## Differences from github.com and gitlab.com

md-render's offline output is checked against real site HTML. Its golden tests compare 43 GitHub
files, 10 GitLab repository files and 16 GitLab issues with a normalizing DOM diff. All GitHub
files and all GitLab repository files match, as do 12 of the 16 GitLab issues. Of the other four,
three are pages that gitlab.com rendered with an older GitLab version and still serves from its
cache, and one uses PlantUML and custom emoji, which need GitLab's servers. These are the known
differences.

**Both flavours**

* **References and mentions are not linked offline.** GitLab links `@user`, `#123`, `!45`,
  `~label`, `%milestone`, `&epic`, `$snippet` and commit SHAs by looking them up in its database,
  and GitHub does the same in comments. md-render leaves them as text. GitHub's file view does
  not link them either. `--api --context owner/repo` gets the site's links.
* **External images are not proxied.** github.com serves them through its camo proxy and
  gitlab.com through its asset proxy. md-render keeps the original URL, so your browser loads
  them from the original host.
* **Syntax highlighting is an approximation.** github.com uses Linguist's TextMate grammars
  (tree-sitter for some languages), and gitlab.com uses Rouge 5.1.0. md-render tokenizes with the
  Sublime syntaxes bundled in `two-face` and maps the scopes to each site's classes. `diff`,
  `console`/shell sessions and (on GitLab) `Dockerfile` are exact ports. Measured per character
  on real site output: about 92% of GitHub code gets github.com's exact class nesting, and 92%
  to 95% gets its colour. About 86% of GitLab code gets Rouge's exact class, and 92% a class of
  the same family. A language without a bundled grammar (PowerShell, for example) is not
  highlighted.
* **Styles are whatever the sites served the day you vendored.** Hashed file names and rules
  change with every deploy. Re-run `md-render vendor` to refresh them. Page chrome is limited to
  the file box. There is no site navigation.
* **Math and diagrams render in the browser** with MathJax 3.2.0 (GitHub) or KaTeX 0.16.47
  (GitLab), and with mermaid. These libraries come from the bundle, so `vendor --no-libs`, or no
  bundle at all, leaves them as source.
* **Pathological nesting.** Deeply nested input would make rendering quadratic. GitLab stops at
  a nesting depth of 4096 and shows gitlab.com's "Rendering aborted" message. Its parser places
  a few elements differently, so the cut-off can come a few levels earlier or later than on
  gitlab.com. GitHub output above a fixed nesting budget is shown as escaped source with a
  warning. Deeply nested `>` quotes are still slow inside comrak, the Markdown parser, for
  inputs with tens of thousands of levels.
* md-render adds `data-sourcepos` and `data-mdr-key` attributes, which the live updates need.
  Exports drop `data-sourcepos`.

**GitHub flavour**

* Mermaid renders inline (github.com uses an iframe viewer). GeoJSON, TopoJSON and STL blocks
  are shown as code.
* Some file-view behaviours cannot be checked without committing files to github.com, so they are
  inferred from the API's output: multi-line inline math, invalid YAML or TOML front matter, and
  date-only YAML values.
* Which 26 emoji github.com wraps in `<g-emoji>` was observed, not taken from GitHub's source. The
  list is complete for gemoji as of 2026-09-28.
* With `--api`, GitHub has no endpoint for the file view, so md-render calls `mode=gfm` and
  rewrites the result. Soft line breaks then stay `<br>` in readme mode, where the file view
  joins the lines.

**GitLab flavour**

* PlantUML blocks (rendered by gitlab.com's PlantUML server) and Kroki diagrams (off on
  gitlab.com, possibly on for self-managed instances) are shown as code. So are ```` ```json:table ````
  and GLQL blocks, which GitLab's JavaScript turns into tables.
* `::include` reads the working tree, so it also includes files that are not committed yet,
  which gitlab.com would not find.
* Comment mode renders like gitlab.com's comments but draws only a simple note box around them
  (approximate).
* The link sanitizer models Ruby's Addressable and URI libraries as gitlab.com runs them. It does
  not model host-normalisation failures, the LDAP/WS/WSS/File URI classes, or the 4 KiB limit on
  normalized paths.
* Matching of text-presentation emoji (without U+FE0F) is approximated.
* Self-managed instances can differ from gitlab.com (Kroki and PlantUML settings, themes, a
  relative URL root). `vendor --gitlab-source URL` takes the styles from your instance.

## Development

```sh
cargo build
cargo test --workspace                          # unit and integration tests, no network
cargo clippy --workspace --all-targets          # must stay free of warnings
cargo fmt --all --check
cargo test -p tarfs --doc                        # includes the tarfs README examples
```

* **Golden tests** compare the offline output with real github.com and gitlab.com HTML
  (`tests/github_golden.rs`, `tests/gitlab_golden.rs`, `tests/api_golden.rs`,
  `tests/highlight_golden.rs`). The fixtures are public files from other projects, several
  under GPL, AGPL or CC-BY-SA licenses or under no license at all, so they are not committed.
  The tests read them from `MD_RENDER_GOLDEN_DIR` (default `<repo>/.dev/research`, laid out as
  `github/{ctx,pages,comments,probes}` and `gitlab/{samples,issue_samples}`) and print a loud
  `SKIPPED (set MD_RENDER_GOLDEN_DIR)` without it:

  ```sh
  MD_RENDER_GOLDEN_DIR=/path/to/research cargo test --test github_golden --test gitlab_golden -- --nocapture
  ```

  `GOLDEN_STRICT_HIGHLIGHT=1` also compares the token classes inside code blocks. Small
  hand-written inputs with expected output are committed under `tests/fixtures/<flavor>/`.
* **Network tests** are `#[ignore]`d: `cargo test --test api_live -- --ignored` (the live APIs;
  GitLab needs `GITLAB_TOKEN`) and the live vendoring test in `tests/styles_vendor.rs`.
  `cargo test -p tarfs --test julia -- --ignored` checks byte-identical output against TarFS.jl
  (needs `julia`).
* **Browser tests** (`tests/server_browser.rs`) drive headless Chrome through Node, and
  `tests/page_client_js.rs` runs the page scripts under Node. Both print `SKIPPED` without `node`
  (or without Chrome; `MD_RENDER_TEST_CHROME` points at a Chrome binary).
* `MD_RENDER_PROP_CORPUS=DIR` runs the sanitizer property test over a directory of `.md` files.
* CI (`.github/workflows/ci.yml`) checks formatting, runs clippy and rustdoc with warnings as
  errors, runs the test suite on Linux and macOS, and checks that the crate builds with the
  minimum supported Rust version (1.88). The golden tests print `SKIPPED` there.

[`docs/DESIGN.md`](docs/DESIGN.md) describes the architecture, the decisions behind it and the
internal interfaces. The style bundle reader is the separate [`tarfs`](crates/tarfs/README.md)
crate.

## License

md-render is licensed under the BSD 3-Clause License (see [`LICENSE`](LICENSE)).

It ports code and embeds data from other projects (GitLab's Markdown formatter and filters,
Linguist and Rouge language data, gemoji and GitLab emoji data, Primer Octicons, grip and more),
each under its own licence. [`THIRD_PARTY_NOTICES.md`](THIRD_PARTY_NOTICES.md) lists them, and
[`docs/dependency-licenses.md`](docs/dependency-licenses.md) lists the licences of the Rust crates
compiled into the binary.

md-render does not distribute the site stylesheets, fonts, images or JavaScript libraries.
`md-render vendor` downloads them from GitHub, GitLab and jsDelivr on your machine. An inline
export embeds them, so take their terms into account before you publish one.
