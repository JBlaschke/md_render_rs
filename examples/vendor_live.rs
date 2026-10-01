//! Vendor the real GitHub and GitLab styles into a bundle (network access required).
//!
//! ```text
//! cargo run --example vendor_live -- OUTPUT.tar.gz [github|gitlab|all] [--all-themes] [--no-libs] [--quiet]
//! ```
//!
//! This is the manual end-to-end check of `md-render vendor` that `cargo test` cannot do (it
//! never touches the network). It vendors into OUTPUT (updating it in place if it exists),
//! then reopens the bundle exactly like a server would and lists its contents.

use std::path::PathBuf;
use std::process::ExitCode;

use md_render::Flavor;
use md_render::styles::Bundle;
use md_render::styles::vendor::{self, VendorOptions};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> anyhow::Result<()> {
    const USAGE: &str = "usage: vendor_live OUTPUT.tar.gz [github|gitlab|all] [--all-themes] \
                         [--no-libs] [--quiet]";
    // Never default to the user's real cache bundle: the output is always explicit.
    let mut opts = VendorOptions {
        output: PathBuf::new(),
        ..VendorOptions::default()
    };
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "github" => opts.flavors = vec![Flavor::Github],
            "gitlab" => opts.flavors = vec![Flavor::Gitlab],
            "all" => opts.flavors = Flavor::ALL.to_vec(),
            "--all-themes" => opts.all_github_themes = true,
            "--no-libs" => opts.libs = false,
            "--quiet" => opts.quiet = true,
            "-h" | "--help" => {
                println!("{USAGE}");
                return Ok(());
            }
            other if other.starts_with('-') => anyhow::bail!("unknown option {other}\n{USAGE}"),
            other => opts.output = PathBuf::from(other),
        }
    }
    if opts.output.as_os_str().is_empty() {
        anyhow::bail!("no output file given\n{USAGE}");
    }

    let report = vendor::vendor(&opts)?;
    println!(
        "vendored {} files ({} bytes) into {}; {} non-fatal failures",
        report.files,
        report.bytes,
        report.output.display(),
        report.failures.len()
    );
    for f in &report.failures {
        println!(
            "  failure: {} ({}) {}{}",
            f.url,
            f.status.map_or_else(|| "-".to_owned(), |s| s.to_string()),
            f.error,
            f.referenced_by
                .as_deref()
                .map(|r| format!(" [referenced by {r}]"))
                .unwrap_or_default()
        );
    }

    // Reopen it exactly like a server would, and list what is inside.
    let bundle = Bundle::open(&report.output)?;
    println!("etag {}", bundle.etag());
    for (flavor, fm) in &bundle.manifest().flavors {
        println!(
            "{flavor}: {} stylesheets, {} assets, scripts {:?}, themes {:?}",
            fm.stylesheets.len(),
            fm.assets.len(),
            fm.scripts.keys().collect::<Vec<_>>(),
            fm.themes
        );
        for s in &fm.stylesheets {
            println!(
                "  {:<11} {:<22} {:>9}  {}{}",
                s.role,
                s.theme.as_deref().unwrap_or("-"),
                s.size,
                s.path,
                s.media
                    .as_deref()
                    .map(|m| format!("  media={m}"))
                    .unwrap_or_default()
            );
        }
    }
    println!("files:");
    for path in bundle.paths() {
        println!("  {:>9}  {path}", bundle.file_size(path).unwrap_or(0));
    }
    Ok(())
}
