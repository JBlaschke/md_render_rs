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
    // An all-digit ADDRESS out of port range is a usage error, reported before anything else
    // (no bundle warning, never a `cannot listen on 0.1.17.112`).
    for port in ["70000", "99999"] {
        let (code, _, stderr) = run(&e, &["-q", "--styles", "/nonexistent.tar.gz", ".", port]);
        assert_eq!(code, 2, "{port}: {stderr}");
        assert!(
            stderr.starts_with(&format!(
                "error: invalid address '{port}': port {port} is out of range (0-65535)"
            )),
            "{stderr}"
        );
    }
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
    // CLI-21: the shadowing hint survives a failed plan and comes before the error.
    let dir = e.cwd.join("empty");
    fs::write(dir.join("export"), "# x\n").unwrap();
    let out = md_render(&dir, &e.home, &["export"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "{stderr}");
    assert_eq!(
        stderr,
        "md-render: warning: running the 'export' subcommand; to serve the file 'export' use \
         ./export\nerror: No README found at .\n"
    );
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

/// Q-LOG-1 / Q-DECODE-1: export diagnostics are log events, so `-q` and `RUST_LOG` silence
/// them; a file and the same bytes on stdin get the same UTF-8 warning; the fallback hint
/// names the bundle given with `--styles` (STY-1); html5ever's "foster parenting not
/// implemented" noise is not shown.
#[test]
fn export_diagnostics_are_log_events() {
    let e = env();
    let bytes = b"\xEF\xBB\xBF# a\xFFb\n\n<table>x<tr><td>a</td></tr></table>\n";
    fs::write(e.cwd.join("bad.md"), bytes).unwrap();
    let styles = e.cwd.join("missing.tar.gz");
    let styles = styles.to_str().unwrap();
    let export = |args: &[&str], rust_log: Option<&str>| {
        let mut cmd = command(&e.cwd, &e.home, args);
        if let Some(v) = rust_log {
            cmd.env("RUST_LOG", v);
        }
        let mut child = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        {
            use std::io::Write as _;
            child.stdin.take().unwrap().write_all(bytes).unwrap();
        }
        let out = child.wait_with_output().unwrap();
        assert!(out.status.success(), "{args:?}: {out:?}");
        String::from_utf8_lossy(&out.stderr).into_owned()
    };
    let utf8 = "is not valid UTF-8 (first invalid byte at offset 6); invalid bytes were replaced";

    let file = export(&["export", "--styles", styles, "bad.md", "out.html"], None);
    let bad = e.cwd.join("bad.md");
    assert!(
        file.contains(&format!("md-render: warning: {} {utf8}", bad.display())),
        "{file}"
    );
    assert!(
        file.contains(&format!(
            "md-render: hint: run `md-render vendor --flavor github -o {styles}`"
        )),
        "{file}"
    );
    assert!(!file.contains("foster parenting"), "{file}");
    let stdin = export(&["export", "--styles", styles, "-", "out2.html"], None);
    assert!(
        stdin.contains(&format!("md-render: warning: stdin {utf8}")),
        "{stdin}"
    );

    for (args, rust_log) in [
        (
            &["export", "-q", "--styles", styles, "bad.md", "o.html"][..],
            None,
        ),
        (
            &["export", "--styles", styles, "bad.md", "o.html"][..],
            Some("off"),
        ),
        (
            &["export", "--styles", styles, "-", "o.html"][..],
            Some("off"),
        ),
    ] {
        let stderr = export(args, rust_log);
        assert!(
            stderr.is_empty(),
            "{args:?} RUST_LOG={rust_log:?}: {stderr}"
        );
    }
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

/// `md-render export -` reads stdin and writes the page to stdout (no file is written).
#[test]
fn stdin_export_to_stdout_is_planned_before_running() {
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
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    let html = String::from_utf8_lossy(&out.stdout);
    assert!(
        html.contains("<html") && html.contains("From stdin"),
        "{html}"
    );
    let written: Vec<_> = fs::read_dir(&e.cwd)
        .unwrap()
        .map(|d| d.unwrap().file_name())
        .collect();
    assert_eq!(written, ["README.md"], "export - wrote a file");
}

/// A minimal HTTP/1.1 server on 127.0.0.1 answering GETs from `files` (404 otherwise), one
/// connection at a time, for as long as the test process runs. Returns `host:port`.
fn mock_site(files: Vec<(&'static str, &'static str)>) -> String {
    use std::io::{BufRead as _, BufReader, Write as _};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut reader = BufReader::new(stream);
            let mut request_line = String::new();
            if reader.read_line(&mut request_line).is_err() {
                continue;
            }
            // Skip the headers.
            let mut line = String::new();
            while reader.read_line(&mut line).is_ok_and(|n| n > 2) {
                line.clear();
            }
            let path = request_line.split_whitespace().nth(1).unwrap_or("/");
            let (status, body) = match files.iter().find(|(p, _)| *p == path) {
                Some((_, body)) => ("200 OK", *body),
                None => ("404 Not Found", "not found"),
            };
            let kind = if path.ends_with(".css") {
                "text/css"
            } else {
                "text/html"
            };
            let mut stream = reader.into_inner();
            let _ = write!(
                stream,
                "HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\n\
                 Connection: close\r\n\r\n{body}",
                body.len()
            );
        }
    });
    addr
}

/// CLI-15: a successful `vendor` prints its summary once (the vendor module's
/// "wrote … (N files, … uncompressed, … on disk)"), not a second "Wrote N files (… KiB)" line in
/// other units.
#[test]
fn vendor_prints_one_summary() {
    let e = env();
    let page = "<!DOCTYPE html><html><head>\
        <link rel=\"stylesheet\" href=\"/assets/light-0123456789abcdef.css\">\
        <link rel=\"stylesheet\" href=\"/assets/dark-0123456789abcdef.css\">\
        <link rel=\"stylesheet\" href=\"/assets/primer-primitives-0123456789abcdef.css\">\
        <link rel=\"stylesheet\" href=\"/assets/primer-0123456789abcdef.css\">\
        <link rel=\"stylesheet\" href=\"/assets/global-0123456789abcdef.css\">\
        <link rel=\"stylesheet\" href=\"/assets/github-0123456789abcdef.css\">\
        <link rel=\"stylesheet\" href=\"/assets/repository-0123456789abcdef.css\">\
        <link rel=\"stylesheet\" href=\"/assets/code-0123456789abcdef.css\">\
        </head><body>hi</body></html>";
    let css = ".markdown-body{color:#1f2328}\n";
    let addr = mock_site(vec![
        ("/page.html", page),
        ("/assets/light-0123456789abcdef.css", css),
        ("/assets/dark-0123456789abcdef.css", css),
        ("/assets/primer-primitives-0123456789abcdef.css", css),
        ("/assets/primer-0123456789abcdef.css", css),
        ("/assets/global-0123456789abcdef.css", css),
        ("/assets/github-0123456789abcdef.css", css),
        ("/assets/repository-0123456789abcdef.css", css),
        ("/assets/code-0123456789abcdef.css", css),
    ]);
    let source = format!("http://{addr}/page.html");
    let mut cmd = command(
        &e.cwd,
        &e.home,
        &[
            "vendor",
            "--flavor",
            "github",
            "--no-libs",
            "--github-source",
            &source,
            "-o",
            "bundle.tar.gz",
        ],
    );
    for var in [
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "http_proxy",
        "https_proxy",
        "all_proxy",
    ] {
        cmd.env_remove(var);
    }
    let out = cmd.stdin(Stdio::null()).output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    assert!(e.cwd.join("bundle.tar.gz").is_file(), "{stderr}");
    let summaries: Vec<&str> = stderr
        .lines()
        .filter(|l| l.trim_start().to_ascii_lowercase().starts_with("wrote "))
        .collect();
    assert_eq!(summaries.len(), 1, "{stderr}");
    assert!(summaries[0].contains("bundle.tar.gz"), "{stderr}");
    assert!(!stderr.contains("KiB"), "{stderr}");
}

/// `export FILE -` writes to stdout; `--export` (grip) writes `<stem>.html`.
#[test]
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
