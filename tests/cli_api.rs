//! `--api` through the built binary against a local mock API: which token is sent where
//! (CLI-3), and that credentials embedded in `--api-url` never show up in the output (CLI-6).
//!
//! Every run gets a private `HOME` and a `PATH` without `gh` / `glab`, so neither the
//! developer's real tokens nor their credential helpers are ever used.

use std::fs;
use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::net::{TcpListener, TcpStream};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;

const BIN: &str = env!("CARGO_BIN_EXE_md-render");

/// Variables that could carry a real token or change the output.
const SCRUBBED: [&str; 10] = [
    "GITHUB_TOKEN",
    "GH_TOKEN",
    "GH_ENTERPRISE_TOKEN",
    "GITHUB_ENTERPRISE_TOKEN",
    "GITLAB_TOKEN",
    "GH_HOST",
    "MD_RENDER_FLAVOR",
    "MD_RENDER_STYLES",
    "RUST_LOG",
    "CLICOLOR_FORCE",
];

/// Request heads (lower-cased header names) seen by a mock API.
type Seen = Arc<Mutex<Vec<Vec<(String, String)>>>>;

fn read_head(stream: &mut TcpStream) -> Vec<(String, String)> {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut headers = Vec::new();
    let mut length = 0usize;
    let mut first = true;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 {
            break;
        }
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if first {
            headers.push((":request".to_owned(), line.to_owned()));
            first = false;
        } else if let Some((name, value)) = line.split_once(':') {
            let name = name.trim().to_ascii_lowercase();
            if name == "content-length" {
                length = value.trim().parse().unwrap_or(0);
            }
            headers.push((name, value.trim().to_owned()));
        }
    }
    let mut body = vec![0; length];
    let _ = reader.read_exact(&mut body);
    headers
}

/// A mock API answering every request with `status` and `body`.
fn mock_api(status: &str, content_type: &str, body: &'static str) -> (u16, Seen) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen: Seen = Arc::default();
    let log = seen.clone();
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let head = read_head(&mut stream);
            log.lock().unwrap().push(head);
            let _ = stream.write_all(response.as_bytes());
        }
    });
    (port, seen)
}

fn header<'a>(head: &'a [(String, String)], name: &str) -> Option<&'a str> {
    head.iter()
        .find(|(k, _)| k == name)
        .map(|(_, v)| v.as_str())
}

struct Run {
    code: i32,
    stdout: String,
    stderr: String,
}

/// Run the binary in a fresh directory holding `README.md`, with `env` added.
fn md_render(args: &[&str], env: &[(&str, &str)]) -> Run {
    let tmp = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(tmp.path()).unwrap();
    let home = root.join("home");
    let bin = root.join("bin"); // an empty PATH entry: no gh, no glab
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&bin).unwrap();
    fs::write(root.join("README.md"), "# Hello\n").unwrap();
    let mut cmd = Command::new(BIN);
    cmd.args(args)
        .current_dir(&root)
        .env("HOME", &home)
        .env("XDG_CACHE_HOME", home.join(".cache"))
        .env("PATH", &bin)
        .env("NO_COLOR", "1")
        .stdin(Stdio::null());
    for var in SCRUBBED {
        cmd.env_remove(var);
    }
    for (k, v) in env {
        cmd.env(k, v);
    }
    let out = cmd.output().expect("running md-render");
    Run {
        code: out.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    }
}

#[test]
fn an_enterprise_api_gets_the_enterprise_token_not_the_github_com_one() {
    let (port, seen) = mock_api("200 OK", "text/html", "<h1>From mock</h1>");
    let api_url = format!("http://127.0.0.1:{port}/api/v3");
    let run = md_render(
        &["--api", "--api-url", &api_url, "export", "README.md", "-"],
        &[
            ("GITHUB_TOKEN", "dotcom-token-not-real"),
            ("GH_ENTERPRISE_TOKEN", "ghe-token-not-real"),
        ],
    );
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert!(run.stdout.contains("From mock"), "{}", run.stdout);
    assert!(
        run.stderr.contains("token from $GH_ENTERPRISE_TOKEN"),
        "{}",
        run.stderr
    );
    let seen = seen.lock().unwrap();
    assert_eq!(
        header(&seen[0], "authorization"),
        Some("Bearer ghe-token-not-real")
    );
    assert!(!run.stderr.contains("not-real") && !run.stdout.contains("not-real"));

    // Only a github.com token: nothing is sent, and the hint names the enterprise variables.
    let (port, seen) = mock_api("200 OK", "text/html", "<h1>From mock</h1>");
    let api_url = format!("http://127.0.0.1:{port}/api/v3");
    let run = md_render(
        &["--api", "--api-url", &api_url, "export", "README.md", "-"],
        &[("GITHUB_TOKEN", "dotcom-token-not-real")],
    );
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert!(
        run.stderr.contains("no token: set GH_ENTERPRISE_TOKEN"),
        "{}",
        run.stderr
    );
    assert_eq!(header(&seen.lock().unwrap()[0], "authorization"), None);

    // GITHUB_ENTERPRISE_TOKEN is honoured too.
    let (port, seen) = mock_api("200 OK", "text/html", "<h1>From mock</h1>");
    let api_url = format!("http://127.0.0.1:{port}/api/v3");
    let run = md_render(
        &["--api", "--api-url", &api_url, "export", "README.md", "-"],
        &[("GITHUB_ENTERPRISE_TOKEN", "ghe2-token-not-real")],
    );
    assert_eq!(run.code, 0, "{}", run.stderr);
    assert_eq!(
        header(&seen.lock().unwrap()[0], "authorization"),
        Some("Bearer ghe2-token-not-real")
    );
}
