//! `md-render vendor` (owner: styles). Contract: docs/DESIGN.md §3.5, §4.

use std::path::PathBuf;

use crate::flavor::Flavor;
use crate::styles::Failure;

/// Default page scraped for GitHub stylesheets.
pub const GITHUB_SOURCE: &str = "https://github.com/joeyespo/grip";
/// Default page scraped for GitLab stylesheets.
pub const GITLAB_SOURCE: &str = "https://gitlab.com/gitlab-org/gitlab-foss";

/// Options of one vendoring run.
#[derive(Clone, Debug)]
pub struct VendorOptions {
    pub flavors: Vec<Flavor>,
    pub output: PathBuf,
    pub github_source: String,
    pub gitlab_source: String,
    /// Vendor all 14 GitHub themes instead of light + dark.
    pub all_github_themes: bool,
    /// Also vendor client libraries (MathJax / KaTeX / mermaid) and emoji images.
    pub libs: bool,
    pub quiet: bool,
}

/// Summary of a vendoring run.
#[derive(Clone, Debug, Default)]
pub struct VendorReport {
    pub output: PathBuf,
    pub files: usize,
    pub bytes: u64,
    pub failures: Vec<Failure>,
}

/// Download the requested flavors' styles into the bundle at `opts.output`.
pub fn vendor(opts: &VendorOptions) -> anyhow::Result<VendorReport> {
    let _ = opts;
    anyhow::bail!("vendoring is not implemented yet")
}
