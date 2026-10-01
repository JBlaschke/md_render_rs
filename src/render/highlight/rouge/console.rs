//! Port of Rouge 5.1.0's `ConsoleLexer` (`lib/rouge/lexers/console.rb`, MIT) with its default
//! options, the GL §8.4 "console" special case. Per line (including its `\n`):
//!
//! 1. an elision (`/\A\s*(?:<[.]+>|[.]+)\s*\z/`, e.g. `...`) → `c`; both lexers reset;
//! 2. a prompt (`/^.*?(?:\$|#|>|;)/`: everything up to the *first* `$`, `#`, `>` or `;`, so
//!    `user@host:~$` is one prompt) → `gp`, then leading ASCII whitespace → `w`, then the rest
//!    is lexed by the shell lexer, whose state carries over from the previous prompt line;
//! 3. anything else → `go` (output); the shell lexer is reset.
//!
//! The shell part uses the bash syntax with the GitLab class scheme for Rouge's `shell` tag.

use super::classes::ForLang;
use super::{Segment, select_syntax};
use crate::render::highlight::engine::{self, Event, Limits, LineTokenizer};

/// Rouge's default prompt end characters (`$ # > ;`).
fn prompt_end(line: &str) -> Option<usize> {
    line.find(['$', '#', '>', ';']).map(|i| i + 1)
}

/// `/\A\s*(?:<[.]+>|[.]+)\s*\z/` (Ruby `\s` is ASCII whitespace).
fn is_elision(line: &str) -> bool {
    let t = line.trim_matches(|c: char| c.is_ascii_whitespace() || c == '\x0b');
    let dots = |s: &str| !s.is_empty() && s.bytes().all(|b| b == b'.');
    dots(t) || t.strip_prefix('<').and_then(|s| s.strip_suffix('>')).is_some_and(dots)
}

/// Tokenize a console session into segments.
pub(super) fn segments(
    code: &str,
    shell: &ForLang<'_>,
    out: &mut Vec<Segment>,
) -> Result<(), engine::Error> {
    if !Limits::DEFAULT.admits(code) {
        return Err(engine::Error);
    }
    let syntax = select_syntax("shell", "sh");
    engine::guarded(|| {
        let mut tokenizer = syntax.map(LineTokenizer::new);
        let mut start = 0usize;
        for line in code.split_inclusive('\n') {
            let base = start;
            start += line.len();
            if is_elision(line) {
                out.push(Segment::new(Some("c"), base..start));
                if let Some(t) = tokenizer.as_mut() {
                    t.reset();
                }
            } else if let Some(end) = prompt_end(line) {
                out.push(Segment::new(Some("gp"), base..base + end));
                let rest = &line[end..];
                let ws = rest.len()
                    - rest.trim_start_matches(|c: char| c.is_ascii_whitespace() || c == '\x0b').len();
                if ws > 0 {
                    out.push(Segment::new(Some("w"), base + end..base + end + ws));
                }
                let command = &line[end + ws..];
                let cmd_base = base + end + ws;
                match tokenizer.as_mut() {
                    Some(t) if !command.is_empty() => {
                        let mut offset = cmd_base;
                        t.feed(command, &mut |event, stack| {
                            if let Event::Text(text) = event {
                                out.push(Segment::new(shell.classify(stack), offset..offset + text.len()));
                                offset += text.len();
                            }
                        })?;
                    }
                    _ if !command.is_empty() => out.push(Segment::new(None, cmd_base..start)),
                    _ => {}
                }
            } else {
                out.push(Segment::new(Some("go"), base..start));
                if let Some(t) = tokenizer.as_mut() {
                    t.reset();
                }
            }
        }
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_detection() {
        assert_eq!(prompt_end("$ ls\n"), Some(1));
        assert_eq!(prompt_end("user@host:~$ ls\n"), Some(12));
        assert_eq!(prompt_end("foo > bar\n"), Some(5));
        assert_eq!(prompt_end("   Compiling foo v0.1.0\n"), None);
    }

    #[test]
    fn elisions() {
        assert!(is_elision("...\n"));
        assert!(is_elision("  <...>  \n"));
        assert!(!is_elision("\n"));
        assert!(!is_elision("<>\n"));
        assert!(!is_elision("..x\n"));
    }
}
