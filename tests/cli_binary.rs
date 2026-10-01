//! Subprocess tests of the built `md-render` binary: version, usage errors (exit 2), runtime
//! errors (exit 1), the cache commands, and export to stdout.
//!
//! Every run gets a private `HOME` / `XDG_CACHE_HOME` and no token or flavor variables, so the
//! developer's real cache and credentials are never touched.

use std::fs;
use std::path::Path;
use std::process::{Command, Output, Stdio};

const BIN: &str = env!("CARGO_BIN_EXE_md-render");

/// Run the binary in `cwd` with an isolated environment rooted at `home`.
fn md_render(cwd: &Path, home: &Path, args: &[&str]) -> Output {
    command(cwd, home, args)
        .stdin(Stdio::null())
        .output()
        .expect("running md-render")
}

/// The binary with an isolated environment rooted at `home`.
fn command(cwd: &Path, home: &Path, args: &[&str]) -> Command {
    let mut cmd = Command::new(BIN);
    cmd.args(args)
        .current_dir(cwd)
        .env("HOME", home)
        .env("XDG_CACHE_HOME", home.join(".cache"))
        .env("LOCALAPPDATA", home.join("AppData"))
        .env("NO_COLOR", "1");
    for var in [
        "MD_RENDER_FLAVOR",
        "MD_RENDER_STYLES",
        "GITHUB_TOKEN",
        "GH_TOKEN",
        "GH_ENTERPRISE_TOKEN",
        "GITHUB_ENTERPRISE_TOKEN",
        "GITLAB_TOKEN",
        "RUST_LOG",
        "CLICOLOR_FORCE",
    ] {
        cmd.env_remove(var);
    }
    cmd
}

struct Env {
    _tmp: tempfile::TempDir,
    cwd: std::path::PathBuf,
    home: std::path::PathBuf,
}

fn env() -> Env {
    let tmp = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(tmp.path()).unwrap();
    let cwd = root.join("work");
    let home = root.join("home");
    fs::create_dir_all(&cwd).unwrap();
    fs::create_dir_all(&home).unwrap();
    fs::write(cwd.join("README.md"), "# Hello\n\nWorld.\n").unwrap();
    Env {
        _tmp: tmp,
        cwd,
        home,
    }
}

fn run(e: &Env, args: &[&str]) -> (i32, String, String) {
    let out = md_render(&e.cwd, &e.home, args);
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn version() {
    let e = env();
    for flag in ["-V", "--version"] {
        let (code, stdout, stderr) = run(&e, &[flag]);
        assert_eq!(code, 0, "{stderr}");
        assert_eq!(stdout, format!("md-render {}\n", env!("CARGO_PKG_VERSION")));
    }
}

#[test]
fn help_exits_zero() {
    let e = env();
    let (code, stdout, _) = run(&e, &["--help"]);
    assert_eq!(code, 0);
    assert!(
        stdout.contains("Usage: md-render [OPTIONS] [PATH] [ADDRESS]"),
        "{stdout}"
    );
    let (code, stdout, _) = run(&e, &["export", "-h"]);
    assert_eq!(code, 0);
    assert!(stdout.contains("md-render export"), "{stdout}");
}

#[test]
fn usage_errors_exit_2() {
    let e = env();
    for args in [
        &["--bogus"][..],
        &["a", "b", "c"],
        &["-p", "8080"],
        &["--port=8080"],
        &["-a", "0.0.0.0"],
        &["README.md", "bad::addr"],
        &["README.md", "vendor"],
        &["--flavor", "bitbucket"],
        &["--user", "me"],
        &["--theme", "solarized"],
    ] {
        let (code, stdout, stderr) = run(&e, args);
        assert_eq!(code, 2, "{args:?}: {stderr}");
        assert!(
            stdout.is_empty(),
            "{args:?}: usage errors go to stderr, got {stdout:?}"
        );
        assert!(stderr.starts_with("error: "), "{args:?}: {stderr}");
    }
    let (_, _, stderr) = run(&e, &["-p", "8080"]);
    assert!(stderr.contains("instead of -p"), "{stderr}");
    let (_, _, stderr) = run(&e, &["README.md", "vendor"]);
    assert!(stderr.contains("use ./vendor"), "{stderr}");
    // `cache` without an action shows its help on stderr, still a usage error.
    let (code, stdout, stderr) = run(&e, &["cache"]);
    assert_eq!((code, stdout.as_str()), (2, ""));
    assert!(stderr.contains("Usage: md-render cache"), "{stderr}");
}

#[test]
fn runtime_errors_exit_1() {
    let e = env();
    let (code, _, stderr) = run(&e, &["nonexist.md"]);
    assert_eq!(code, 1);
    assert_eq!(stderr, "error: File not found: nonexist.md\n");
    fs::create_dir(e.cwd.join("empty")).unwrap();
    let (code, _, stderr) = run(&e, &["export", "empty"]);
    assert_eq!(code, 1);
    assert!(stderr.contains("No README found at empty"), "{stderr}");
}

#[test]
fn warnings_go_to_stderr_and_quiet_silences_them() {
    let e = env();
    let (code, stdout, stderr) = run(&e, &["--wide", "cache", "path"]);
    assert_eq!(code, 0);
    assert!(stdout.ends_with("styles.tar.gz\n"), "{stdout}");
    assert!(stderr.contains("--wide has no effect"), "{stderr}");
    let (_, _, stderr) = run(&e, &["-q", "--wide", "cache", "path"]);
    assert!(stderr.is_empty(), "{stderr}");
}

#[test]
fn cache_path_and_clear_stay_in_the_cache_directory() {
    let e = env();
    let (code, stdout, stderr) = run(&e, &["cache", "path"]);
    assert_eq!(code, 0, "{stderr}");
    let bundle = std::path::PathBuf::from(stdout.trim_end());
    assert_eq!(
        bundle.file_name().and_then(|n| n.to_str()),
        Some("styles.tar.gz")
    );
    // Only continue when the cache really is inside the private HOME.
    assert!(
        bundle.starts_with(&e.home),
        "cache path {} escapes the test HOME",
        bundle.display()
    );

    let cache_dir = bundle.parent().unwrap();
    fs::create_dir_all(cache_dir).unwrap();
    fs::write(&bundle, b"old bundle").unwrap();
    let mine = e.cwd.join("mine.tar.gz");
    fs::write(&mine, b"keep me").unwrap();

    let (code, stdout, stderr) = run(&e, &["--styles", "mine.tar.gz", "cache", "clear"]);
    assert_eq!(code, 0, "{stderr}");
    assert!(stdout.starts_with("Cache cleared"), "{stdout}");
    assert!(stdout.contains("Kept the style bundle"), "{stdout}");
    assert!(!bundle.exists() && !cache_dir.exists());
    assert_eq!(fs::read(&mine).unwrap(), b"keep me");

    // grip's --clear, on an empty cache.
    let (code, stdout, _) = run(&e, &["--clear"]);
    assert_eq!(code, 0);
    assert!(stdout.starts_with("Cache is already empty"), "{stdout}");
    let (code, stdout, _) = run(&e, &["--clear", "-q"]);
    assert_eq!((code, stdout.as_str()), (0, ""));
}

#[test]
fn stdin_export_to_stdout_is_planned_before_running() {
    // Until the page owner lands `export`, the run fails at the export step (exit 1), never with a
    // usage error: the CLI part (reading stdin, choosing stdout) succeeded.
    let e = env();
    let mut child = command(&e.cwd, &e.home, &["export", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    {
        use std::io::Write as _;
        let mut stdin = child.stdin.take().unwrap();
        stdin.write_all(b"# From stdin\n").unwrap();
    }
    let out = child.wait_with_output().unwrap();
    let code = out.status.code().unwrap_or(-1);
    assert!(code == 0 || code == 1, "unexpected exit code {code}");
    if code == 0 {
        let html = String::from_utf8_lossy(&out.stdout);
        assert!(html.contains("From stdin"), "{html}");
    }
}

/// Needs `md_render::export::export` (owner: page); enable once it is implemented.
#[test]
#[ignore = "needs the export implementation (owner: page)"]
fn export_to_stdout() {
    let e = env();
    let (code, stdout, stderr) = run(&e, &["export", "README.md", "-"]);
    assert_eq!(code, 0, "{stderr}");
    assert!(
        stdout.contains("<html") && stdout.contains("Hello"),
        "{stdout}"
    );
    assert!(!e.cwd.join("README.html").exists());

    let (code, _, stderr) = run(&e, &["--export"]);
    assert_eq!(code, 0, "{stderr}");
    assert!(
        fs::read_to_string(e.cwd.join("README.html"))
            .unwrap()
            .contains("Hello")
    );
}
