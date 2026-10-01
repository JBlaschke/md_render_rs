//! argv → planned configuration, through the library API with an injected [`Host`] (no process,
//! no real environment, no real stdin). Covers GRIP §2.3 (path/address resolution), §2.6 (export
//! naming), D9/D14 (bundle path, served root) and the DESIGN §8 post-parse checks.

use std::collections::HashMap;
use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use md_render::cli::plan::{ExportPlan, ServePlan, VendorPlan};
use md_render::cli::{CliError, Host, Invocation, Plan, invocation};
use md_render::export::{ExportInput, ExportOutput};
use md_render::page::Layout;
use md_render::render::OfflineRenderer;
use md_render::server::Target;
use md_render::{Flavor, Mode, Theme, ThemeSel};

/// A scratch working directory:
/// `README.md`, `80` (a file named like a port), `docs/guide.md`, `docs/README.md`,
/// `nodocs/`, `other/`, `style.css`.
struct Fixture {
    _tmp: tempfile::TempDir,
    /// Canonical path of the working directory.
    cwd: PathBuf,
    cache: PathBuf,
}

impl Fixture {
    fn new() -> Fixture {
        let tmp = tempfile::tempdir().expect("tempdir");
        let cwd = fs::canonicalize(tmp.path())
            .expect("canonical tempdir")
            .join("work");
        fs::create_dir_all(cwd.join("docs")).unwrap();
        fs::create_dir_all(cwd.join("nodocs")).unwrap();
        fs::create_dir_all(cwd.join("other")).unwrap();
        fs::write(cwd.join("README.md"), "# Readme\n").unwrap();
        fs::write(cwd.join("80"), "# eighty\n").unwrap();
        fs::write(cwd.join("docs/guide.md"), "# Guide\n").unwrap();
        fs::write(cwd.join("docs/README.md"), "# Docs\n").unwrap();
        fs::write(cwd.join("style.css"), "body{}").unwrap();
        let cache = cwd.parent().unwrap().join("cache/md-render");
        Fixture {
            _tmp: tmp,
            cwd,
            cache,
        }
    }

    fn host(&self) -> FakeHost {
        FakeHost {
            cwd: self.cwd.clone(),
            env: HashMap::new(),
            stdin: b"# From stdin\n".to_vec(),
            tty: false,
            cache: Some(self.cache.clone()),
            notices: Vec::new(),
        }
    }
}

struct FakeHost {
    cwd: PathBuf,
    env: HashMap<String, OsString>,
    stdin: Vec<u8>,
    tty: bool,
    cache: Option<PathBuf>,
    notices: Vec<String>,
}

impl FakeHost {
    fn env(mut self, name: &str, value: &str) -> Self {
        self.env.insert(name.to_owned(), value.into());
        self
    }
}

impl Host for FakeHost {
    fn cwd(&self) -> io::Result<PathBuf> {
        Ok(self.cwd.clone())
    }
    fn var(&self, name: &str) -> Option<OsString> {
        self.env.get(name).cloned()
    }
    fn read_stdin(&mut self) -> io::Result<Vec<u8>> {
        Ok(std::mem::take(&mut self.stdin))
    }
    fn stdin_is_terminal(&self) -> bool {
        self.tty
    }
    fn cache_dir(&self) -> Option<PathBuf> {
        self.cache.clone()
    }
    fn notice(&mut self, message: &str) {
        self.notices.push(message.to_owned());
    }
}

fn argv(args: &[&str]) -> Vec<String> {
    std::iter::once("md-render")
        .chain(args.iter().copied())
        .map(str::to_owned)
        .collect()
}

fn run_with(host: &mut FakeHost, args: &[&str]) -> Result<Invocation, CliError> {
    invocation(argv(args), host)
}

fn run(fx: &Fixture, args: &[&str]) -> Result<Invocation, CliError> {
    run_with(&mut fx.host(), args)
}

fn serve(fx: &Fixture, args: &[&str]) -> ServePlan {
    match run(fx, args) {
        Ok(Invocation {
            plan: Plan::Serve(p),
            ..
        }) => p,
        other => panic!("{args:?}: expected serve, got {other:?}"),
    }
}

fn export(fx: &Fixture, args: &[&str]) -> ExportPlan {
    match run(fx, args) {
        Ok(Invocation {
            plan: Plan::Export(p),
            ..
        }) => p,
        other => panic!("{args:?}: expected export, got {other:?}"),
    }
}

fn vendor_with(host: &mut FakeHost, args: &[&str]) -> VendorPlan {
    match run_with(host, args) {
        Ok(Invocation {
            plan: Plan::Vendor(p),
            ..
        }) => p,
        other => panic!("{args:?}: expected vendor, got {other:?}"),
    }
}

/// The error and its exit code.
fn error(fx: &Fixture, args: &[&str]) -> (String, u8) {
    match run(fx, args) {
        Err(e) => (e.to_string(), e.exit_code()),
        Ok(inv) => panic!("{args:?}: expected an error, got {inv:?}"),
    }
}

fn dir(rel: &str) -> Target {
    Target::Dir(PathBuf::from(rel))
}

fn file(rel: &str) -> Target {
    Target::File(PathBuf::from(rel))
}

/// GRIP §2.3 resolution table, plus the IPv6 extension (C2) and the fixed invalid-address
/// behaviour (C10: exit 2 instead of a warning).
#[test]
fn grip_path_and_address_table() {
    let fx = Fixture::new();
    let docs = fx.cwd.join("docs");
    let rows: &[(&[&str], &Path, Target, &str, u16)] = &[
        (&[], &fx.cwd, dir(""), "localhost", 6419),
        (&["80"], &fx.cwd, dir(""), "localhost", 80),
        (&["0"], &fx.cwd, dir(""), "localhost", 0),
        (&[":80"], &fx.cwd, dir(""), "localhost", 80),
        (&["localhost:80"], &fx.cwd, dir(""), "localhost", 80),
        (&["0.0.0.0:8080"], &fx.cwd, dir(""), "0.0.0.0", 8080),
        (&["host.:80"], &fx.cwd, dir(""), "host.", 80),
        (&["./80"], &fx.cwd, file("80"), "localhost", 6419),
        (
            &["README.md"],
            &fx.cwd,
            file("README.md"),
            "localhost",
            6419,
        ),
        (
            &["README.md", "0.0.0.0"],
            &fx.cwd,
            file("README.md"),
            "0.0.0.0",
            6419,
        ),
        (
            &["README.md", "8080"],
            &fx.cwd,
            file("README.md"),
            "localhost",
            8080,
        ),
        (
            &["README.md", "localhost"],
            &fx.cwd,
            file("README.md"),
            "localhost",
            6419,
        ),
        // A valid host name that will not resolve: accepted here, the bind fails (exit 1).
        (
            &["README.md", "6419x"],
            &fx.cwd,
            file("README.md"),
            "6419x",
            6419,
        ),
        (&["docs"], &docs, dir(""), "localhost", 6419),
        (&["docs/"], &docs, dir(""), "localhost", 6419),
        (
            &["docs/guide.md"],
            &docs,
            file("guide.md"),
            "localhost",
            6419,
        ),
        (
            &["docs/../README.md", "1234"],
            &fx.cwd,
            file("README.md"),
            "localhost",
            1234,
        ),
        (&["80", "81"], &fx.cwd, file("80"), "localhost", 81),
        // IPv6 (bracketed; stored without brackets for binding).
        (&["[::1]:8080"], &fx.cwd, dir(""), "::1", 8080),
        (&["[::]:0"], &fx.cwd, dir(""), "::", 0),
        (
            &["README.md", "[::1]"],
            &fx.cwd,
            file("README.md"),
            "::1",
            6419,
        ),
        (
            &["README.md", "[fe80::1]:9"],
            &fx.cwd,
            file("README.md"),
            "fe80::1",
            9,
        ),
    ];
    for (args, root, target, host, port) in rows {
        let p = serve(&fx, args);
        assert_eq!(
            (p.root.as_path(), &p.target, p.host.as_str(), p.port),
            (*root, target, *host, *port),
            "argv {args:?}"
        );
    }
}

#[test]
fn lone_host_names_and_bad_ports_are_paths() {
    let fx = Fixture::new();
    for (arg, hint) in [
        ("0.0.0.0", true),
        ("localhost", true),
        ("70000", false),
        ("[::1]", true),
        ("notes.md", false),
        // CLI-8: extension-less file names are not host names.
        ("notes", false),
        ("README", false),
    ] {
        let (msg, code) = error(&fx, &[arg]);
        assert_eq!(code, 1, "{arg}");
        assert!(
            msg.starts_with(&format!("error: File not found: {arg}")),
            "{msg}"
        );
        assert_eq!(msg.contains(&format!("md-render . {arg}")), hint, "{msg}");
        // Exports never listen: no hint at all.
        let (msg, code) = error(&fx, &["export", arg]);
        assert_eq!(code, 1, "{arg}");
        assert_eq!(msg, format!("error: File not found: {arg}"));
    }
    assert!(
        error(&fx, &["70000"]).0.contains("nor is it a port"),
        "{:?}",
        error(&fx, &["70000"])
    );
    // `_` and unbracketed IPv6 never form an address: as a lone positional they are paths.
    assert!(
        error(&fx, &["my_host:80"])
            .0
            .contains("File not found: my_host:80")
    );
    assert!(error(&fx, &["::1:80"]).0.contains("File not found: ::1:80"));
}

#[test]
fn invalid_addresses_are_usage_errors() {
    let fx = Fixture::new();
    for addr in [
        "bad::addr",
        "foo_bar",
        "host:",
        "host:70000",
        "[::1]:",
        "[nope]:80",
        "::1",
    ] {
        let (msg, code) = error(&fx, &["README.md", addr]);
        assert_eq!(code, 2, "{addr}: {msg}");
        assert!(msg.contains(&format!("invalid address '{addr}'")), "{msg}");
    }
    let (msg, _) = error(&fx, &["README.md", "::1"]);
    assert!(
        msg.contains("put IPv6 addresses in brackets: [::1]:6419"),
        "{msg}"
    );
}

#[test]
fn a_port_shadowing_an_existing_file_warns() {
    let fx = Fixture::new();
    let inv = run(&fx, &["80"]).unwrap();
    assert!(
        inv.warnings.iter().any(|w| w.contains("use ./80")),
        "{:?}",
        inv.warnings
    );
    let inv = run(&fx, &["81"]).unwrap();
    assert!(inv.warnings.is_empty(), "{:?}", inv.warnings);
}

#[test]
fn served_root_rules() {
    let fx = Fixture::new();
    let p = serve(&fx, &["docs/guide.md", "--root", "."]);
    assert_eq!((p.root, p.target), (fx.cwd.clone(), file("docs/guide.md")));
    let p = serve(&fx, &["--root", ".", "docs"]);
    assert_eq!((p.root, p.target), (fx.cwd.clone(), dir("docs")));
    let p = serve(&fx, &["--root", "docs", "docs"]);
    assert_eq!((p.root, p.target), (fx.cwd.join("docs"), dir("")));

    let (msg, code) = error(&fx, &["docs/guide.md", "--root", "other"]);
    assert_eq!(code, 2);
    assert!(msg.contains("does not contain docs/guide.md"), "{msg}");
    let (msg, code) = error(&fx, &["--root", "missing", "README.md"]);
    assert_eq!(code, 2);
    assert!(msg.contains("'--root <DIR>'"), "{msg}");
    let (msg, code) = error(&fx, &["--root", "README.md", "README.md"]);
    assert_eq!(code, 2);
    assert!(msg.contains("not a directory"), "{msg}");
}

#[cfg(unix)]
#[test]
fn symlinks_keep_their_name_and_escapes_warn() {
    let fx = Fixture::new();
    let outside = fx.cwd.parent().unwrap().join("outside.md");
    fs::write(&outside, "x").unwrap();
    std::os::unix::fs::symlink(&outside, fx.cwd.join("link.md")).unwrap();
    std::os::unix::fs::symlink(fx.cwd.join("README.md"), fx.cwd.join("inner.md")).unwrap();

    let inv = run(&fx, &["link.md"]).unwrap();
    let Plan::Serve(p) = &inv.plan else { panic!() };
    assert_eq!(p.target, file("link.md"));
    assert!(
        inv.warnings.iter().any(|w| w.contains("--follow-symlinks")),
        "{:?}",
        inv.warnings
    );
    assert!(
        run(&fx, &["--follow-symlinks", "link.md"])
            .unwrap()
            .warnings
            .is_empty()
    );
    let inv = run(&fx, &["inner.md"]).unwrap();
    assert!(inv.warnings.is_empty(), "{:?}", inv.warnings);
}

#[test]
fn stdin_is_read_eagerly() {
    let fx = Fixture::new();
    let mut host = fx.host();
    host.stdin = b"\xEF\xBB\xBF# Hi \xFF\n".to_vec();
    host.tty = true;
    let inv = run_with(&mut host, &["-", "8000"]).unwrap();
    let Plan::Serve(p) = &inv.plan else {
        panic!("{inv:?}")
    };
    assert_eq!(
        p.target,
        Target::Stdin {
            name: "stdin".into(),
            text: "# Hi \u{FFFD}\n".into()
        }
    );
    assert_eq!((p.root.as_path(), p.port), (fx.cwd.as_path(), 8000));
    assert!(
        inv.warnings.iter().any(|w| w.contains("not valid UTF-8")),
        "{:?}",
        inv.warnings
    );
    assert_eq!(host.notices.len(), 1);
    assert!(
        host.notices[0].starts_with("Reading Markdown from stdin"),
        "{:?}",
        host.notices
    );

    // No TTY hint when piped or quiet.
    let mut host = fx.host();
    run_with(&mut host, &["-"]).unwrap();
    assert!(host.notices.is_empty());
    let mut host = fx.host();
    host.tty = true;
    run_with(&mut host, &["-q", "-"]).unwrap();
    assert!(host.notices.is_empty());
}

#[test]
fn rendering_options() {
    let fx = Fixture::new();
    let p = serve(&fx, &[]);
    let r = &p.render;
    assert_eq!(
        (r.flavor, r.mode, r.layout),
        (Flavor::Github, Mode::Readme, Layout::Readme)
    );
    assert_eq!(r.theme, ThemeSel::default());
    assert_eq!(
        (r.title.as_deref(), r.context.as_deref(), r.api.as_ref()),
        (None, None, None)
    );
    assert_eq!(r.bundle_path, fx.cache.join("styles.tar.gz"));
    assert!(p.live && !p.open_browser && p.poll.is_none() && !p.follow_symlinks && !p.auto_vendor);

    let p = serve(
        &fx,
        &[
            "-f",
            "gitlab",
            "--mode",
            "comment",
            "--context",
            "g/p",
            "--title",
            "T",
            "--theme",
            "dark",
            "--layout",
            "bare",
            "--css",
            "style.css",
            "--css",
            "https://e.org/x.css",
            "-b",
            "--no-reload",
            "--poll",
            "--follow-symlinks",
            "--auto-vendor",
        ],
    );
    let r = &p.render;
    assert_eq!(
        (r.flavor, r.mode, r.layout),
        (Flavor::Gitlab, Mode::Comment, Layout::Bare)
    );
    assert_eq!(
        (r.title.as_deref(), r.context.as_deref()),
        (Some("T"), Some("g/p"))
    );
    assert_eq!(r.theme.theme, Theme::Dark);
    assert_eq!(
        r.extra_css,
        [
            fx.cwd.join("style.css").to_string_lossy().into_owned(),
            "https://e.org/x.css".into()
        ]
    );
    assert!(!p.live && p.open_browser && p.follow_symlinks && p.auto_vendor);
    assert_eq!(p.poll, Some(Duration::from_millis(300)));

    assert_eq!(
        serve(&fx, &["--poll=50"]).poll,
        Some(Duration::from_millis(50))
    );
    assert!(!serve(&fx, &["--norefresh"]).live);
    assert_eq!(serve(&fx, &["--user-content"]).render.mode, Mode::Comment);
    assert_eq!(
        serve(&fx, &["--user-content", "--mode", "comment"])
            .render
            .mode,
        Mode::Comment
    );
    assert_eq!(error(&fx, &["--user-content", "--mode", "readme"]).1, 2);
    assert_eq!(serve(&fx, &["--context", "  "]).render.context, None);
    let (msg, code) = error(&fx, &["--css", "missing.css"]);
    assert_eq!(code, 1);
    assert!(msg.contains("--css missing.css: no such file"), "{msg}");
    assert_eq!(error(&fx, &["--poll=1"]).1, 2);
    assert_eq!(error(&fx, &["--layout", "wide"]).1, 2);
}

#[test]
fn themes() {
    let fx = Fixture::new();
    let sel = |args: &[&str]| serve(&fx, args).render.theme;
    let t = |theme, light: &str, dark: &str| ThemeSel {
        theme,
        light: light.into(),
        dark: dark.into(),
    };
    assert_eq!(sel(&["--theme", "light"]), t(Theme::Light, "light", "dark"));
    assert_eq!(
        sel(&["--theme", "dark_dimmed"]),
        t(Theme::Dark, "light", "dark_dimmed")
    );
    assert_eq!(
        sel(&["--theme", "light_high_contrast"]),
        t(Theme::Light, "light_high_contrast", "dark")
    );
    assert_eq!(
        sel(&[
            "--light-theme",
            "light_colorblind",
            "--dark-theme",
            "dark_tritanopia"
        ]),
        t(Theme::Auto, "light_colorblind", "dark_tritanopia")
    );
    assert_eq!(
        sel(&["--theme", "dark_dimmed", "--dark-theme", "dark_dimmed"]),
        t(Theme::Dark, "light", "dark_dimmed")
    );
    assert_eq!(
        error(&fx, &["--theme", "dark_dimmed", "--dark-theme", "dark"]).1,
        2
    );
    assert_eq!(error(&fx, &["--theme", "solarized"]).1, 2);
    assert_eq!(error(&fx, &["--dark-theme", "nope"]).1, 2);
    assert_eq!(error(&fx, &["-f", "gitlab", "--theme", "dark_dimmed"]).1, 2);
    assert_eq!(
        serve(&fx, &["-f", "gitlab", "--theme", "dark"])
            .render
            .theme,
        t(Theme::Dark, "light", "dark")
    );
}

#[test]
fn flavor_and_styles_from_the_environment() {
    let fx = Fixture::new();
    let mut host = fx
        .host()
        .env("MD_RENDER_FLAVOR", "gitlab")
        .env("MD_RENDER_STYLES", "s/b.tar.gz");
    let Plan::Serve(p) = run_with(&mut host, &[]).unwrap().plan else {
        panic!()
    };
    assert_eq!(p.render.flavor, Flavor::Gitlab);
    assert_eq!(p.render.bundle_path, fx.cwd.join("s/b.tar.gz"));
    // Flags beat the environment.
    let Plan::Serve(p) = run_with(&mut host, &["-f", "github", "--styles", "/x/y.tar.gz"])
        .unwrap()
        .plan
    else {
        panic!()
    };
    assert_eq!(p.render.flavor, Flavor::Github);
    assert_eq!(p.render.bundle_path, Path::new("/x/y.tar.gz"));
    // Case-insensitive; empty means unset.
    let mut host = fx.host().env("MD_RENDER_FLAVOR", "GitLab");
    let Plan::Serve(p) = run_with(&mut host, &[]).unwrap().plan else {
        panic!()
    };
    assert_eq!(p.render.flavor, Flavor::Gitlab);
    let mut host = fx
        .host()
        .env("MD_RENDER_FLAVOR", "")
        .env("MD_RENDER_STYLES", "");
    let Plan::Serve(p) = run_with(&mut host, &[]).unwrap().plan else {
        panic!()
    };
    assert_eq!(
        (p.render.flavor, p.render.bundle_path),
        (Flavor::Github, fx.cache.join("styles.tar.gz"))
    );
    // Invalid values are usage errors; `all` is for vendor only.
    for bad in ["bitbucket", "all"] {
        let mut host = fx.host().env("MD_RENDER_FLAVOR", bad);
        let err = run_with(&mut host, &[]).unwrap_err();
        assert_eq!(err.exit_code(), 2);
        assert!(err.to_string().contains("MD_RENDER_FLAVOR"), "{err}");
    }
    assert_eq!(error(&fx, &["--flavor", "all"]).1, 2);
    assert_eq!(error(&fx, &["--flavor", "bitbucket"]).1, 2);
}

#[test]
fn api_options() {
    let fx = Fixture::new();
    assert_eq!(serve(&fx, &[]).render.api, None);
    let api = serve(
        &fx,
        &[
            "--api",
            "--token",
            "t0k",
            "--api-url",
            "https://ghe.example/api/v3",
        ],
    )
    .render
    .api
    .unwrap();
    assert_eq!(api.flavor, Flavor::Github);
    assert_eq!(
        (api.api_url.as_deref(), api.token.as_deref()),
        (Some("https://ghe.example/api/v3"), Some("t0k"))
    );
    // Serving falls back to offline rendering with a banner; exports fail loudly (GRIP C27).
    assert!(api.fallback_offline);
    assert!(
        !export(&fx, &["--api", "export"])
            .render
            .api
            .unwrap()
            .fallback_offline
    );
    assert_eq!(
        serve(&fx, &["-f", "gitlab", "--api"])
            .render
            .api
            .unwrap()
            .flavor,
        Flavor::Gitlab
    );
    // Tokens never show up in debug output.
    let dbg = format!("{:?}", run(&fx, &["--api", "--token", "s3cr3t"]).unwrap());
    assert!(!dbg.contains("s3cr3t"), "{dbg}");

    assert_eq!(error(&fx, &["--api", "--api-url", "ftp://x"]).1, 2);

    // CLI-6: credentials in --api-url are a usage error that never repeats them.
    for (flavor, url, hint) in [
        (
            "gitlab",
            "http://oauth2:s3cr3t@127.0.0.1:7249",
            "GITLAB_TOKEN",
        ),
        (
            "github",
            "https://me:s3cr3t@ghe.example/api/v3",
            "GH_ENTERPRISE_TOKEN",
        ),
        ("github", "https://s3cr3t@ghe.example", "GITHUB_TOKEN"),
    ] {
        let (msg, code) = error(&fx, &["-f", flavor, "--api", "--api-url", url]);
        assert_eq!(code, 2, "{msg}");
        assert!(
            msg.contains("credentials in the URL are not supported") && msg.contains(hint),
            "{msg}"
        );
        assert!(msg.contains("***@") && !msg.contains("s3cr3t"), "{msg}");
        let (msg, _) = error(&fx, &["-f", flavor, "--api", "--api-url", url, "export"]);
        assert!(!msg.contains("s3cr3t"), "{msg}");
    }
    // Unparseable URLs are shown redacted as well.
    let (msg, code) = error(
        &fx,
        &["--api", "--api-url", "http://u:s3cr3t@h:99999/x?k=s3cr3t"],
    );
    assert_eq!(code, 2, "{msg}");
    assert!(!msg.contains("s3cr3t"), "{msg}");
    let (msg, _) = error(&fx, &["vendor", "--github-source", "ftp://u:s3cr3t@h/"]);
    assert!(!msg.contains("s3cr3t"), "{msg}");

    let inv = run(&fx, &["--token", "x"]).unwrap();
    assert!(
        inv.warnings
            .iter()
            .any(|w| w.contains("no effect without --api")),
        "{:?}",
        inv.warnings
    );
}

#[test]
fn grip_compatibility_flags() {
    let fx = Fixture::new();
    let inv = run(&fx, &["--wide"]).unwrap();
    assert!(
        inv.warnings
            .iter()
            .any(|w| w.contains("--wide has no effect"))
    );
    for args in [
        &["--user", "me"][..],
        &["--pass", "pw"],
        &["--user=me", "--pass=pw", "export"],
    ] {
        let (msg, code) = error(&fx, args);
        assert_eq!(code, 2);
        assert!(msg.contains("--token"), "{msg}");
    }
    let (msg, code) = error(&fx, &["-p", "8080"]);
    assert_eq!(code, 2);
    assert!(msg.contains("instead of -p"), "{msg}");
    let (msg, code) = error(&fx, &["-a", "0.0.0.0"]);
    assert_eq!(code, 2);
    assert!(msg.contains("instead of -a"), "{msg}");
    assert_eq!(error(&fx, &["--no-inline"]).1, 2);

    let inv = run(&fx, &["-q"]).unwrap();
    assert!(inv.quiet && inv.verbose == 0);
    assert_eq!(run(&fx, &["-vv"]).unwrap().verbose, 2);
    assert_eq!(error(&fx, &["-q", "-v"]).1, 2);
}

/// GRIP §2.6 output naming, for both `export` and grip's `--export`.
#[test]
fn export_naming() {
    let fx = Fixture::new();
    let file_in = |p: &ExportPlan| match &p.input {
        ExportInput::File(f) => f.clone(),
        other => panic!("{other:?}"),
    };
    let file_out = |p: &ExportPlan| match &p.output {
        ExportOutput::File(f) => Some(f.clone()),
        ExportOutput::Stdout => None,
    };
    let rows: &[(&[&str], &str, Option<&str>)] = &[
        (&["export"], "README.md", Some("README.html")),
        (&["--export"], "README.md", Some("README.html")),
        (
            &["export", "docs"],
            "docs/README.md",
            Some("docs/README.html"),
        ),
        (
            &["docs", "--export"],
            "docs/README.md",
            Some("docs/README.html"),
        ),
        (
            &["export", "docs/guide.md"],
            "docs/guide.md",
            Some("docs/guide.html"),
        ),
        (
            &["--export", "docs/guide.md"],
            "docs/guide.md",
            Some("docs/guide.html"),
        ),
        (
            &["export", "README.md", "out/x.html"],
            "README.md",
            Some("out/x.html"),
        ),
        (
            &["--export", "README.md", "out/x.html"],
            "README.md",
            Some("out/x.html"),
        ),
        (&["export", "README.md", "-"], "README.md", None),
        (&["--export", "README.md", "-"], "README.md", None),
        // grip: in export mode the second positional is never an address.
        (
            &["--export", "README.md", "8080"],
            "README.md",
            Some("8080"),
        ),
        (&["export", "80"], "80", Some("80.html")),
    ];
    for (args, input, output) in rows {
        let p = export(&fx, args);
        assert_eq!(file_in(&p), fx.cwd.join(input), "{args:?}");
        assert_eq!(file_out(&p), output.map(|o| fx.cwd.join(o)), "{args:?}");
        assert!(p.inline, "{args:?}");
        assert_eq!(p.render.api, None);
    }
    // stdin → stdout by default.
    for args in [
        &["export", "-"][..],
        &["-", "--export"],
        &["--export", "-", "-"],
    ] {
        let p = export(&fx, args);
        assert!(
            matches!(&p.input, ExportInput::Stdin { name, text } if name == "stdin" && text == "# From stdin\n")
        );
        assert!(matches!(p.output, ExportOutput::Stdout), "{args:?}");
    }
    let p = export(&fx, &["export", "-", "out.html"]);
    assert_eq!(file_out(&p), Some(fx.cwd.join("out.html")));

    // --no-inline before or after the subcommand, and with grip's --export.
    for args in [
        &["export", "--no-inline"][..],
        &["--no-inline", "export"],
        &["--export", "--no-inline"],
    ] {
        assert!(!export(&fx, args).inline, "{args:?}");
    }

    // Errors.
    let (msg, code) = error(&fx, &["--export", "foo.html"]);
    assert_eq!(code, 1);
    assert!(msg.contains("File not found: foo.html"), "{msg}");
    let (msg, code) = error(&fx, &["export", "nodocs"]);
    assert_eq!(code, 1);
    assert!(msg.contains("No README found at nodocs"), "{msg}");
    let (msg, code) = error(&fx, &["export", "README.md", "README.md"]);
    assert_eq!(code, 2);
    assert!(msg.contains("would overwrite its input"), "{msg}");
    for args in [
        &["--export", "-b"][..],
        &["--export", "--root", "."],
        &["--export", "--poll"],
    ] {
        assert_eq!(error(&fx, args).1, 2, "{args:?}");
    }
}

#[test]
fn export_render_options() {
    let fx = Fixture::new();
    // ExportConfig has no extra stylesheets: say so instead of ignoring --css silently.
    let inv = run(&fx, &["export", "--css", "style.css"]).unwrap();
    assert!(
        inv.warnings
            .iter()
            .any(|w| w.contains("--css has no effect on exports")),
        "{:?}",
        inv.warnings
    );
    assert!(run(&fx, &["export"]).unwrap().warnings.is_empty());
    let p = export(
        &fx,
        &[
            "--flavor", "gitlab", "export", "--title", "T", "--layout", "bare", "--mode", "comment",
        ],
    );
    assert_eq!(
        (p.render.flavor, p.render.mode, p.render.layout),
        (Flavor::Gitlab, Mode::Comment, Layout::Bare)
    );
    assert_eq!(p.render.title.as_deref(), Some("T"));
    let cfg = p.into_config(Arc::new(OfflineRenderer));
    assert_eq!(
        (cfg.flavor, cfg.mode, cfg.title.as_deref()),
        (Flavor::Gitlab, Mode::Comment, Some("T"))
    );
    assert!(cfg.inline);
    assert_eq!(cfg.renderer.name(), "offline");
}

#[test]
fn vendor_options() {
    let fx = Fixture::new();
    let default_bundle = fx.cache.join("styles.tar.gz");
    let v = vendor_with(&mut fx.host(), &["vendor"]);
    assert_eq!(v.options.flavors, Flavor::ALL);
    assert_eq!(v.options.output, default_bundle);
    assert!(v.create_parent && v.options.libs && !v.options.all_github_themes && !v.options.quiet);
    assert_eq!(v.options.github_source, "https://github.com/joeyespo/grip");
    assert_eq!(
        v.options.gitlab_source,
        "https://gitlab.com/gitlab-org/gitlab-foss"
    );

    // --flavor before or after the subcommand (design B, CR §8).
    assert_eq!(
        vendor_with(&mut fx.host(), &["--flavor", "gitlab", "vendor"])
            .options
            .flavors,
        [Flavor::Gitlab]
    );
    assert_eq!(
        vendor_with(&mut fx.host(), &["vendor", "-f", "github"])
            .options
            .flavors,
        [Flavor::Github]
    );
    assert_eq!(
        vendor_with(&mut fx.host(), &["vendor", "--flavor", "all"])
            .options
            .flavors,
        Flavor::ALL
    );
    // MD_RENDER_FLAVOR selects the rendering flavor, not what is vendored.
    let mut host = fx.host().env("MD_RENDER_FLAVOR", "gitlab");
    assert_eq!(
        vendor_with(&mut host, &["vendor"]).options.flavors,
        Flavor::ALL
    );

    let v = vendor_with(
        &mut fx.host(),
        &[
            "-q",
            "vendor",
            "-o",
            "b.tar.gz",
            "--themes",
            "all",
            "--no-libs",
            "--github-source",
            "https://github.com/o/r",
        ],
    );
    assert_eq!(v.options.output, fx.cwd.join("b.tar.gz"));
    assert!(!v.create_parent && !v.options.libs && v.options.all_github_themes && v.options.quiet);
    assert_eq!(v.options.github_source, "https://github.com/o/r");

    let v = vendor_with(&mut fx.host(), &["--styles", "s.tar.gz", "vendor"]);
    assert_eq!(
        (v.options.output, v.create_parent),
        (fx.cwd.join("s.tar.gz"), false)
    );
    let mut host = fx.host().env("MD_RENDER_STYLES", "/abs/e.tar.gz");
    assert_eq!(
        vendor_with(&mut host, &["vendor"]).options.output,
        Path::new("/abs/e.tar.gz")
    );

    assert_eq!(error(&fx, &["vendor", "--gitlab-source", "not a url"]).1, 2);
    assert_eq!(error(&fx, &["vendor", "--themes", "some"]).1, 2);
}

#[test]
fn cache_commands() {
    let fx = Fixture::new();
    let path = |args: &[&str], host: &mut FakeHost| match run_with(host, args).unwrap().plan {
        Plan::CachePath(p) => p,
        other => panic!("{other:?}"),
    };
    assert_eq!(
        path(&["cache", "path"], &mut fx.host()),
        fx.cache.join("styles.tar.gz")
    );
    assert_eq!(
        path(&["--styles", "x.tar.gz", "cache", "path"], &mut fx.host()),
        fx.cwd.join("x.tar.gz")
    );
    assert_eq!(
        path(&["cache", "path", "--styles", "/y.tar.gz"], &mut fx.host()),
        Path::new("/y.tar.gz")
    );
    let mut host = fx.host().env("MD_RENDER_STYLES", "/e.tar.gz");
    assert_eq!(path(&["cache", "path"], &mut host), Path::new("/e.tar.gz"));

    let clear = |args: &[&str], host: &mut FakeHost| match run_with(host, args).unwrap().plan {
        Plan::CacheClear(c) => c,
        other => panic!("{other:?}"),
    };
    for args in [
        &["cache", "clear"][..],
        &["--clear"],
        &["--clear", "nonexistent.md"],
    ] {
        let c = clear(args, &mut fx.host());
        assert_eq!(
            (c.cache_dir.as_path(), c.kept_bundle),
            (fx.cache.as_path(), None),
            "{args:?}"
        );
    }
    // A bundle outside the cache directory is never deleted, one inside goes with it.
    let c = clear(
        &["--styles", "mine.tar.gz", "cache", "clear"],
        &mut fx.host(),
    );
    assert_eq!(c.kept_bundle, Some(fx.cwd.join("mine.tar.gz")));
    let inside = fx.cache.join("other.tar.gz");
    let c = clear(
        &["--styles", inside.to_str().unwrap(), "--clear"],
        &mut fx.host(),
    );
    assert_eq!(c.kept_bundle, None);

    let mut host = fx.host();
    host.cache = None;
    let err = run_with(&mut host, &["cache", "clear"]).unwrap_err();
    assert_eq!(err.exit_code(), 1);
    assert_eq!(error(&fx, &["cache"]).1, 2);
    assert_eq!(error(&fx, &["cache", "purge"]).1, 2);
}

/// DESIGN §8 / CR §8 post-parse check: a subcommand plus serve-only arguments.
#[test]
fn subcommands_reject_serve_arguments() {
    let fx = Fixture::new();
    for (args, needle) in [
        (
            &["README.md", "vendor"][..],
            "unexpected argument 'README.md' before subcommand 'vendor' (to render a file named 'vendor' use ./vendor)",
        ),
        (&["8080", "export"], "before subcommand 'export'"),
        (
            &["-b", "vendor"],
            "'--browser' cannot be used with the 'vendor' subcommand",
        ),
        (
            &["--root", ".", "export"],
            "'--root' cannot be used with the 'export' subcommand",
        ),
        (
            &["--poll", "cache", "path"],
            "'--poll' cannot be used with the 'cache' subcommand",
        ),
        (&["--export", "export"], "'--export' cannot be used"),
        (&["--clear", "vendor"], "'--clear' cannot be used"),
        (&["--norefresh", "export"], "'--norefresh' cannot be used"),
        (
            &["--no-inline", "vendor"],
            "'--no-inline' cannot be used with the 'vendor' subcommand",
        ),
    ] {
        let (msg, code) = error(&fx, args);
        assert_eq!(code, 2, "{args:?}");
        assert!(msg.contains(needle), "{args:?}: {msg}");
    }
    // Serve-only flags after the subcommand are unknown there (clap error, exit 2).
    assert_eq!(error(&fx, &["vendor", "-b"]).1, 2);
    assert_eq!(error(&fx, &["export", "--root", "."]).1, 2);
    // `./vendor` and `-- vendor` name a file called `vendor`.
    fs::write(fx.cwd.join("vendor"), "# v").unwrap();
    assert_eq!(serve(&fx, &["./vendor"]).target, file("vendor"));
    assert_eq!(serve(&fx, &["--", "vendor"]).target, file("vendor"));
    // `help` is not a subcommand (a file may be called `help`).
    assert!(error(&fx, &["help"]).0.contains("File not found: help"));
}

#[test]
fn serve_config_conversion() {
    let fx = Fixture::new();
    let p = serve(
        &fx,
        &[
            "--theme",
            "dark",
            "-b",
            "--title",
            "X",
            "README.md",
            "[::1]:7000",
        ],
    );
    let cfg = p.clone().into_config(Arc::new(OfflineRenderer));
    assert_eq!(cfg.root, fx.cwd);
    assert_eq!(cfg.target, file("README.md"));
    assert_eq!((cfg.host.as_str(), cfg.port), ("::1", 7000));
    assert_eq!(
        (cfg.flavor, cfg.mode, cfg.layout),
        (Flavor::Github, Mode::Readme, Layout::Readme)
    );
    assert_eq!(cfg.theme.theme, Theme::Dark);
    assert_eq!(cfg.title.as_deref(), Some("X"));
    assert!(cfg.live && cfg.open_browser && !cfg.quiet && !cfg.auto_vendor && !cfg.follow_symlinks);
    assert_eq!(cfg.bundle_path, fx.cache.join("styles.tar.gz"));
    assert_eq!(cfg.renderer.name(), "offline");
    assert_eq!(p.render.renderer().unwrap().name(), "offline");
}
