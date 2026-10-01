//! `text.shell-session` (Linguist ShellSession: `console`, `shellsession`, `sh-session`).
//!
//! two-face has no shell-session grammar (CQ §1.7), so Linguist's two-rule grammar
//! (atom/language-shellscript `shell-session.cson`) is ported here:
//!
//! ```text
//! match: ^(?:((?:\(\S+\)\s*)?(?:sh\S*?|\w+\S+[@:]\S+(?:\s+\S+)?|\[\S+?[@:][^\n]+?\].*?))\s*)?
//!         ([>$#%❯➜]|\p{Greek})\s+(.*)$
//!   1: entity.other.prompt-prefix.shell-session   → pl-e
//!   2: punctuation.separator.prompt.shell-session → (no class)
//!   3: source.shell, patterns: include source.shell → pl-s1 + shell tokens
//! match: ^.+$   name: meta.output.shell-session   → pl-c1
//! ```
//!
//! Verified against github.com (API probe, 2026-10-01): `user@host:~/dir$ ls` (prefix `pl-e`),
//! `[user@host dir]$`, `sh-5.2$`, `❯`, `λ`, `#`, `%`, `>` prompts; `(venv) $ x`, `  $ x`,
//! `$x` and `PS> x` are output lines; a lone `$` is a prompt (TextMate's `\s+` matches the line
//! terminator); every command is tokenized on its own (an unterminated string ends with the
//! line, the next line is output).

use std::sync::OnceLock;

use regex::Regex;

use super::classes::{Class, Scheme};
use super::{SpanWriter, shell_syntax};
use crate::render::highlight::engine::{self, Limits, LineTokenizer};
use crate::render::highlight::rules::RuleSet;

/// The prompt rule. TextMate matches it against the line *including* its terminator, so the
/// `\s+` after the prompt character may consume the `\n` (a lone `$` is a prompt). Lines are
/// matched here without the terminator; `with_newline` selects the variant that also accepts
/// the end of the line as the separator, so that the leftmost-first match picks the same split
/// as Oniguruma's backtracking does.
fn prompt_regex(with_newline: bool) -> Option<&'static Regex> {
    const PREFIX: &str = r"^(?:((?:\(\S+\)\s*)?(?:sh\S*?|\w+\S+[@:]\S+(?:\s+\S+)?|\[\S+?[@:][^\n]+?\].*?))\s*)?([>$#%❯➜]|\p{Greek})";
    static RE: OnceLock<[Option<Regex>; 2]> = OnceLock::new();
    let [nl, last] = RE.get_or_init(|| {
        [
            Regex::new(&format!(r"{PREFIX}(\s+|$)(.*)$")).ok(),
            Regex::new(&format!(r"{PREFIX}(\s+)(.*)$")).ok(),
        ]
    });
    if with_newline { nl.as_ref() } else { last.as_ref() }
}

/// Render a shell session. Errors (limits, tokenizer failure) make the caller fall back to
/// plain text for the whole block.
pub(super) fn render(
    code: &str,
    scheme: &Scheme,
    overlay: Option<&RuleSet<Class>>,
) -> Result<String, engine::Error> {
    if !Limits::DEFAULT.admits(code) {
        return Err(engine::Error);
    }
    let shell = shell_syntax();
    let mut w = SpanWriter::new(scheme, overlay, code.len() * 2);
    engine::guarded(|| {
        for line in code.split_inclusive('\n') {
            let (content, newline) = match line.strip_suffix('\n') {
                Some(c) => (c, true),
                None => (line, false),
            };
            let re = prompt_regex(newline).ok_or(engine::Error)?;
            match re.captures(content) {
                Some(caps) => {
                    let mut pos = 0;
                    if let Some(prefix) = caps.get(1) {
                        w.push_class(Some("pl-e"));
                        w.text(prefix.as_str());
                        w.pop_class();
                        pos = prefix.end();
                    }
                    let command = caps.get(4).map_or(content.len()..content.len(), |m| m.range());
                    // Whitespace, prompt character and separator: no class.
                    w.text(&content[pos..command.start]);
                    let command = &content[command];
                    if !command.is_empty() {
                        w.push_class(Some("pl-s1"));
                        match shell {
                            Some(syntax) => {
                                // The included grammar's root scope gets no class.
                                w.embed();
                                let mut t = LineTokenizer::new(syntax);
                                t.feed(command, &mut |event, stack| w.event(event, stack))?;
                                // Close what the command left open (e.g. an unterminated string).
                                for _ in 0..t.stack().len() {
                                    w.pop_class();
                                }
                            }
                            None => w.text(command),
                        }
                        w.pop_class();
                    }
                }
                None if !content.is_empty() => {
                    w.push_class(Some("pl-c1"));
                    w.text(content);
                    w.pop_class();
                }
                None => {}
            }
            if newline {
                w.text("\n");
            }
        }
        Ok(())
    })?;
    Ok(w.finish())
}

#[cfg(test)]
mod tests {
    use super::super::{DEFAULT_VARIANT, highlight_with};

    fn session(code: &str) -> String {
        highlight_with(code, "console", DEFAULT_VARIANT).html.unwrap()
    }

    #[test]
    fn prompts_and_output() {
        let html = session("$ ls -la\ntotal 0\n");
        assert_eq!(html, "$ <span class=\"pl-s1\">ls -la</span>\n<span class=\"pl-c1\">total 0</span>");
    }

    #[test]
    fn prompt_prefixes() {
        let html = session("user@host:~/dir$ ls /tmp\n[user@host dir]$ cat f\nsh-5.2$ x\n");
        let lines: Vec<&str> = html.split('\n').collect();
        assert_eq!(lines[0], "<span class=\"pl-e\">user@host:~/dir</span>$ <span class=\"pl-s1\">ls /tmp</span>");
        assert_eq!(lines[1], "<span class=\"pl-e\">[user@host dir]</span>$ <span class=\"pl-s1\">cat f</span>");
        assert_eq!(lines[2], "<span class=\"pl-e\">sh-5.2</span>$ <span class=\"pl-s1\">x</span>");
    }

    #[test]
    fn output_like_lines() {
        let html = session("(venv) $ pip install x\n  $ indented\n$no-space\nPS> Get-Item\n$\n");
        let lines: Vec<&str> = html.split('\n').collect();
        assert_eq!(lines[0], "<span class=\"pl-c1\">(venv) $ pip install x</span>");
        assert_eq!(lines[1], "<span class=\"pl-c1\">  $ indented</span>");
        assert_eq!(lines[2], "<span class=\"pl-c1\">$no-space</span>");
        assert_eq!(lines[3], "<span class=\"pl-c1\">PS&gt; Get-Item</span>");
        assert_eq!(lines[4], "$");
    }

    #[test]
    fn other_prompt_chars() {
        for p in ["#", "%", ">", "❯", "➜", "λ"] {
            let html = session(&format!("{p} x\n"));
            assert_eq!(html, format!("{} <span class=\"pl-s1\">x</span>", p.replace('>', "&gt;")));
        }
    }

    #[test]
    fn unterminated_string_ends_with_line() {
        let html = session("$ echo \"multi\nline\"\n");
        let lines: Vec<&str> = html.split('\n').collect();
        assert_eq!(lines[0].matches("<span").count(), lines[0].matches("</span>").count());
        assert_eq!(lines[1], "<span class=\"pl-c1\">line\"</span>");
    }
}
