//! clap definitions (DESIGN §8; CR §8 "design B": options shared with subcommands are
//! `global = true`, so they work before and after the subcommand name, and a post-parse check
//! rejects serve-only arguments combined with a subcommand).
//!
//! Environment variables (`MD_RENDER_FLAVOR`, `MD_RENDER_STYLES`) are applied after parsing by
//! [`super::plan`], not by clap, so the parse is testable with an injected environment.

use std::ffi::OsString;
use std::path::PathBuf;

use clap::{ArgAction, Args, Parser, Subcommand, ValueEnum};

use crate::flavor::{Mode, Theme};
use crate::page::Layout;

const ABOUT: &str = "Live Markdown preview with GitHub and GitLab flavors (grip-compatible)";

const LONG_ABOUT: &str = "\
Live Markdown preview with GitHub and GitLab flavors (grip-compatible).

Renders offline by default (no network access, nothing leaves the machine), styled with the
real site stylesheets once they have been vendored with `md-render vendor`. The page updates
live while the file changes.";

const AFTER_HELP: &str = "\
Examples:
  md-render                         serve ./README.md (or a listing) on http://localhost:6419/
  md-render docs/guide.md 8080      serve a file on port 8080
  md-render . 0.0.0.0:6419          listen on all interfaces
  md-render README.md [::1]:8080    listen on an IPv6 address
  md-render -f gitlab -b            GitLab flavor, open the browser
  cat notes.md | md-render -        render stdin
  md-render export README.md -      standalone HTML on stdout
  md-render vendor --flavor all     download GitHub and GitLab styles into the style bundle

A lone positional is the ADDRESS when it contains a port (`8080`, `:8080`, `host:8080`), else the
PATH. To serve a file named like a port or a subcommand, write `./8080` or `./vendor`.

Environment:
  MD_RENDER_FLAVOR   default for --flavor
  MD_RENDER_STYLES   default for --styles
  GITHUB_TOKEN, GH_TOKEN, `gh auth token`   GitHub token for --api
  GITLAB_TOKEN, `glab config get token`     GitLab token for --api
  RUST_LOG           log filter (overrides -v/--quiet)";

/// Top-level command line.
#[derive(Parser, Debug, Clone, PartialEq, Eq)]
#[command(
    name = "md-render",
    version,
    about = ABOUT,
    long_about = LONG_ABOUT,
    after_help = AFTER_HELP,
    override_usage = "md-render [OPTIONS] [PATH] [ADDRESS]\n       md-render [OPTIONS] <COMMAND>",
    disable_help_subcommand = true,
    max_term_width = 100
)]
pub struct Cli {
    #[command(flatten)]
    pub global: GlobalArgs,

    #[command(flatten)]
    pub serve: ServeArgs,

    #[command(subcommand)]
    pub command: Option<Command>,
}

/// `--flavor` values; `all` is accepted by `vendor` only.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum FlavorArg {
    /// GitHub (default)
    Github,
    /// GitLab
    Gitlab,
    /// Both flavors (vendor only)
    All,
}

/// `vendor --themes` values.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum)]
pub enum ThemesArg {
    /// GitHub light and dark
    #[default]
    Default,
    /// All 14 GitHub themes
    All,
}

/// Options shared by every command (usable before or after the subcommand).
#[derive(Args, Debug, Clone, Default, PartialEq, Eq)]
pub struct GlobalArgs {
    /// Site flavor to reproduce [env: MD_RENDER_FLAVOR] [default: github]
    #[arg(short = 'f', long, value_enum, global = true, help_heading = "Rendering")]
    pub flavor: Option<FlavorArg>,

    /// Rendering context: repository file view or comment/issue body [default: readme]
    #[arg(long, value_enum, global = true, help_heading = "Rendering")]
    pub mode: Option<Mode>,

    /// Same as --mode comment (grip compatibility)
    #[arg(long, global = true, help_heading = "Rendering")]
    pub user_content: bool,

    /// Repository context (owner/repo, or group/project for GitLab), used by --api and for
    /// reference links
    #[arg(long, value_name = "REPO", global = true, help_heading = "Rendering")]
    pub context: Option<String>,

    /// Page title (default: the file name)
    #[arg(long, global = true, help_heading = "Rendering")]
    pub title: Option<String>,

    /// Color theme: auto (follow the OS), light, dark, or a GitHub theme id such as
    /// dark_dimmed or light_high_contrast [default: auto]
    #[arg(long, value_name = "THEME", value_parser = parse_theme, global = true, help_heading = "Rendering")]
    pub theme: Option<ThemeArg>,

    /// GitHub theme id used in light mode [default: light]
    #[arg(long, value_name = "ID", global = true, help_heading = "Rendering")]
    pub light_theme: Option<String>,

    /// GitHub theme id used in dark mode [default: dark]
    #[arg(long, value_name = "ID", global = true, help_heading = "Rendering")]
    pub dark_theme: Option<String>,

    /// Page chrome: site-like file box, or the content only [default: readme]
    #[arg(long, value_enum, global = true, help_heading = "Rendering")]
    pub layout: Option<Layout>,

    /// Style bundle file [env: MD_RENDER_STYLES] [default: <cache dir>/md-render/styles.tar.gz]
    #[arg(long, value_name = "FILE", global = true, help_heading = "Styles")]
    pub styles: Option<PathBuf>,

    /// Extra stylesheet (URL or local file) added after the site CSS; repeatable
    #[arg(long, value_name = "URL|FILE", action = ArgAction::Append, global = true, help_heading = "Styles")]
    pub css: Vec<String>,

    /// Render through the GitHub/GitLab markdown API instead of offline (sends the document
    /// to the site)
    #[arg(long, global = true, help_heading = "API")]
    pub api: bool,

    /// API base URL (GitHub Enterprise: https://HOST/api/v3; self-hosted GitLab: https://HOST)
    #[arg(long, value_name = "URL", global = true, help_heading = "API")]
    pub api_url: Option<String>,

    /// API token (prefer the GITHUB_TOKEN / GITLAB_TOKEN environment variables: command-line
    /// arguments are visible to other users)
    #[arg(long, value_name = "TOKEN", global = true, help_heading = "API")]
    pub token: Option<String>,

    /// Print nothing but errors
    #[arg(short, long, global = true, conflicts_with = "verbose", help_heading = "Output")]
    pub quiet: bool,

    /// More log output (-v: debug, -vv: trace)
    #[arg(short, long, action = ArgAction::Count, global = true, help_heading = "Output")]
    pub verbose: u8,

    /// grip compatibility: no effect (warns)
    #[arg(long, global = true, hide = true)]
    pub wide: bool,

    /// grip compatibility: rejected (GitHub removed password authentication)
    #[arg(long, value_name = "USERNAME", global = true, hide = true)]
    pub user: Option<String>,

    /// grip compatibility: rejected (use --token)
    #[arg(long, value_name = "PASSWORD", global = true, hide = true)]
    pub pass: Option<String>,
}

/// Arguments of the default (serve) command. Not global: combining them with a subcommand is
/// a usage error (see `plan::reject_serve_args`).
#[derive(Args, Debug, Clone, Default, PartialEq, Eq)]
pub struct ServeArgs {
    /// File to render, or a directory (its README is shown, else a listing); `-` reads stdin
    /// [default: .]
    #[arg(value_name = "PATH", value_parser = clap::value_parser!(OsString))]
    pub path: Option<OsString>,

    /// Where to listen: <host>[:<port>], <port>, or [<ipv6>][:<port>] [default: localhost:6419]
    #[arg(value_name = "ADDRESS", value_parser = clap::value_parser!(OsString))]
    pub address: Option<OsString>,

    /// Open the page in the browser once the server is listening
    #[arg(short, long, help_heading = "Serve")]
    pub browser: bool,

    /// Do not update the page when the file changes
    #[arg(long, visible_alias = "no-reload", help_heading = "Serve")]
    pub norefresh: bool,

    /// Poll for file changes every MS milliseconds instead of using OS notifications (network
    /// file systems) [default MS: 300]
    #[arg(
        long,
        value_name = "MS",
        num_args = 0..=1,
        require_equals = true,
        default_missing_value = "300",
        value_parser = clap::value_parser!(u64).range(10..=600_000),
        help_heading = "Serve"
    )]
    pub poll: Option<u64>,

    /// Serve this directory (it must contain PATH), e.g. the repository root, so that
    /// root-relative links like /docs/x.md work
    #[arg(long, value_name = "DIR", help_heading = "Serve")]
    pub root: Option<PathBuf>,

    /// Serve files whose symlinks point outside the served root
    #[arg(long, help_heading = "Serve")]
    pub follow_symlinks: bool,

    /// Download the site styles on startup when the bundle lacks them
    #[arg(long, help_heading = "Serve")]
    pub auto_vendor: bool,

    /// grip compatibility: export instead of serving (`--export [PATH] [OUT]`, OUT `-` = stdout)
    #[arg(long, help_heading = "Compatibility")]
    pub export: bool,

    /// With --export: link the styles from a directory next to the output instead of inlining
    #[arg(long, help_heading = "Compatibility")]
    pub no_inline: bool,

    /// grip compatibility: same as `md-render cache clear`
    #[arg(long, help_heading = "Compatibility")]
    pub clear: bool,
}

/// Subcommands.
#[derive(Subcommand, Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Write a standalone HTML file
    #[command(
        long_about = "Write a standalone HTML file.\n\nThe output defaults to PATH with the \
                      extension .html (a directory's README.html); `-` writes to stdout. With PATH \
                      `-` the Markdown is read from stdin and the HTML written to stdout."
    )]
    Export(ExportArgs),

    /// Download the site stylesheets, fonts and client libraries into the style bundle
    #[command(
        long_about = "Download the site stylesheets, fonts and client libraries into the style \
                      bundle.\n\nSelect the flavors with the global --flavor github|gitlab|all \
                      (default: all; MD_RENDER_FLAVOR does not apply here). Re-vendoring one \
                      flavor keeps the other one in the bundle; running servers pick up the new \
                      styles automatically.\n\nThe bundle is written to -o FILE, else --styles / \
                      MD_RENDER_STYLES, else the cache directory (see `md-render cache path`)."
    )]
    Vendor(VendorArgs),

    /// Manage the style cache
    Cache(CacheArgs),
}

/// `md-render export`.
#[derive(Args, Debug, Clone, Default, PartialEq, Eq)]
pub struct ExportArgs {
    /// Markdown file or directory (its README); `-` reads stdin [default: .]
    #[arg(value_name = "PATH", value_parser = clap::value_parser!(OsString))]
    pub path: Option<OsString>,

    /// Output file; `-` writes to stdout [default: PATH with extension .html]
    #[arg(value_name = "OUT", value_parser = clap::value_parser!(OsString))]
    pub out: Option<OsString>,

    /// Link the styles from a directory next to the output instead of inlining them
    #[arg(long)]
    pub no_inline: bool,
}

/// `md-render vendor`.
#[derive(Args, Debug, Clone, Default, PartialEq, Eq)]
pub struct VendorArgs {
    /// Bundle file to write [default: --styles, $MD_RENDER_STYLES, or the cache bundle]
    #[arg(short, long, value_name = "FILE")]
    pub output: Option<PathBuf>,

    /// Which GitHub themes to download
    #[arg(long, value_enum, default_value_t = ThemesArg::Default)]
    pub themes: ThemesArg,

    /// Skip the client libraries (MathJax, KaTeX, mermaid) and emoji images
    #[arg(long)]
    pub no_libs: bool,

    /// Page scraped for GitHub stylesheets [default: https://github.com/joeyespo/grip]
    #[arg(long, value_name = "URL")]
    pub github_source: Option<String>,

    /// Page scraped for GitLab stylesheets [default: https://gitlab.com/gitlab-org/gitlab-foss]
    #[arg(long, value_name = "URL")]
    pub gitlab_source: Option<String>,
}

/// `md-render cache`.
#[derive(Args, Debug, Clone, PartialEq, Eq)]
#[command(subcommand_required = true, arg_required_else_help = true)]
pub struct CacheArgs {
    #[command(subcommand)]
    pub action: CacheAction,
}

/// `md-render cache <action>`.
#[derive(Subcommand, Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheAction {
    /// Delete the downloaded style bundle(s) in the cache directory
    Clear,
    /// Print the path of the style bundle in use
    Path,
}

/// The 14 GitHub theme ids (GH §4.4).
pub const GITHUB_THEMES: [&str; 14] = [
    "light",
    "light_high_contrast",
    "light_colorblind",
    "light_colorblind_high_contrast",
    "light_tritanopia",
    "light_tritanopia_high_contrast",
    "dark",
    "dark_high_contrast",
    "dark_colorblind",
    "dark_colorblind_high_contrast",
    "dark_tritanopia",
    "dark_tritanopia_high_contrast",
    "dark_dimmed",
    "dark_dimmed_high_contrast",
];

/// `--theme` value: a mode, or a concrete GitHub theme id (which implies the mode).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ThemeArg {
    Mode(Theme),
    /// A GitHub theme id other than `light`/`dark`; `light_*` ids imply light mode.
    Id(String),
}

/// Parse `--theme`.
pub fn parse_theme(s: &str) -> Result<ThemeArg, String> {
    match s {
        "auto" => Ok(ThemeArg::Mode(Theme::Auto)),
        "light" => Ok(ThemeArg::Mode(Theme::Light)),
        "dark" => Ok(ThemeArg::Mode(Theme::Dark)),
        id if GITHUB_THEMES.contains(&id) => Ok(ThemeArg::Id(id.to_owned())),
        _ => Err(format!(
            "expected auto, light, dark, or a GitHub theme id ({})",
            GITHUB_THEMES[1..].iter().filter(|t| **t != "dark").copied().collect::<Vec<_>>().join(", ")
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory as _;

    #[test]
    fn clap_definition_is_consistent() {
        Cli::command().debug_assert();
    }

    #[test]
    fn theme_values() {
        assert_eq!(parse_theme("auto"), Ok(ThemeArg::Mode(Theme::Auto)));
        assert_eq!(parse_theme("dark_dimmed"), Ok(ThemeArg::Id("dark_dimmed".into())));
        assert!(parse_theme("solarized").unwrap_err().contains("light_high_contrast"));
        assert!(parse_theme("Dark").is_err());
    }
}
