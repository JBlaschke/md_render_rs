//! Port of Rouge 5.1.0's `Docker` lexer (`lib/rouge/lexers/docker.rb`, MIT). Sublime's
//! Dockerfile grammar scopes only the instruction keywords and quoted strings, so GitLab's
//! classes for instruction arguments cannot come from scopes. Rouge's lexer works per line:
//!
//! 1. `FROM image AS name` → `k` `w` `s` `w` `k` `w` `s`;
//! 2. `ONBUILD <instruction> rest` → `k` `w` `k` `s`;
//! 3. `<instruction> rest` (every instruction but `RUN`) → `k`, the rest of the line `s`;
//! 4. `RUN ` (optionally after `ONBUILD `) → `k`, then the shell lexer up to the end of the
//!    line; a line continuation (`\` + newline) is `se` and keeps the shell going on the next
//!    line;
//! 5. anything else is text, except a comment `#…` where a token starts.
//!
//! Instructions are matched case-insensitively and only at the very start of a line.

use super::Segment;
use super::classes::ForLang;
use super::select_syntax;
use super::words;
use crate::render::highlight::engine::{self, Deadline, Event, Limits, LineTokenizer};

/// `KEYWORDS` of the Rouge lexer, in its order (`RUN` and `ONBUILD` have their own rules).
const KEYWORDS: &[&str] = &[
    "FROM",
    "MAINTAINER",
    "CMD",
    "LABEL",
    "EXPOSE",
    "ENV",
    "ADD",
    "COPY",
    "ENTRYPOINT",
    "VOLUME",
    "USER",
    "WORKDIR",
    "ARG",
    "STOPSIGNAL",
    "HEALTHCHECK",
    "SHELL",
];

/// Ruby's `\s`.
fn is_space(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\r' | b'\x0b' | b'\x0c')
}

/// Ruby's `\w` (ASCII).
fn is_word(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// Length of the leading run of `\s` in `s`, or of `\s` other than a newline (`in_line`).
fn spaces(s: &[u8], in_line: bool) -> usize {
    s.iter()
        .take_while(|&&b| is_space(b) && !(in_line && b == b'\n'))
        .count()
}

/// Whether `s` starts with `word` (ASCII case-insensitive).
fn starts_with_ci(s: &[u8], word: &str) -> bool {
    s.len() >= word.len() && s[..word.len()].eq_ignore_ascii_case(word.as_bytes())
}

/// `(KEYWORDS)`, optionally followed by `\b`, at the start of `s`: the keyword's length (the
/// first alternative that matches, as in the regex).
fn keyword(s: &[u8], bounded: bool) -> Option<usize> {
    KEYWORDS
        .iter()
        .find(|k| starts_with_ci(s, k) && (!bounded || s.get(k.len()).is_none_or(|&b| !is_word(b))))
        .map(|k| k.len())
}

/// `^(FROM)(\s+)(.*)(\s+)(AS)(\s+)(.*)` on a line without its newline: the ends of the groups
/// after `FROM`.
fn from_as(line: &[u8]) -> Option<[usize; 6]> {
    if !starts_with_ci(line, "FROM") {
        return None;
    }
    let image = 4 + spaces(&line[4..], true);
    if image == 4 {
        return None;
    }
    // The greedy `(.*)` leaves the last `\s(AS)\s` it can, with one blank before `AS`.
    (image..line.len()).rev().find_map(|i| {
        let rest = &line[i..];
        let found = is_space(rest[0])
            && starts_with_ci(&rest[1..], "AS")
            && rest.get(3).is_some_and(|&b| is_space(b));
        found.then(|| {
            let name = i + 3 + spaces(&line[i + 3..], true);
            [image, i, i + 1, i + 3, name, line.len()]
        })
    })
}

/// `^(ONBUILD)(\s+)(KEYWORDS)(.*)`: the ends of the blanks and of the keyword.
fn onbuild(line: &[u8]) -> Option<(usize, usize)> {
    if !starts_with_ci(line, "ONBUILD") {
        return None;
    }
    let ws = 7 + spaces(&line[7..], true);
    let kw = keyword(&line[ws..], false)?;
    (ws > 7).then_some((ws, ws + kw))
}

/// `^(ONBUILD\s+)?RUN(\s+)`: the length of the match (its blanks may take the newline).
fn run_keyword(line: &[u8]) -> Option<usize> {
    let mut i = 0;
    if starts_with_ci(line, "ONBUILD") {
        let ws = spaces(&line[7..], false);
        if ws == 0 {
            return None;
        }
        i = 7 + ws;
    }
    if !starts_with_ci(&line[i..], "RUN") {
        return None;
    }
    let ws = spaces(&line[i + 3..], false);
    (ws > 0).then_some(i + 3 + ws)
}

/// End of the `(\\.|[^\n\\])+` chunk starting at `pos` (`pos` itself if there is none).
fn run_chunk(line: &str, mut pos: usize) -> usize {
    let bytes = line.as_bytes();
    while pos < bytes.len() {
        match bytes[pos] {
            b'\n' => break,
            b'\\' => match line[pos + 1..].chars().next() {
                Some('\n') | None => break,
                Some(c) => pos += 1 + c.len_utf8(),
            },
            _ => pos += 1,
        }
    }
    pos
}

/// The root state's fallback rules on `content[pos..]` (`\s+`, `#.*?$`, `\w+`, `[^\w]+`): text,
/// except a comment that starts where a token does.
fn root_text(
    content: &[u8],
    mut pos: usize,
    push: &mut impl FnMut(Option<&'static str>, usize, usize),
) {
    let plain = pos;
    while pos < content.len() {
        let b = content[pos];
        if b == b'#' {
            push(None, plain, pos);
            push(Some("c"), pos, content.len());
            return;
        }
        let rest = &content[pos..];
        pos += if is_space(b) {
            rest.iter().take_while(|&&b| is_space(b)).count()
        } else if is_word(b) {
            rest.iter().take_while(|&&b| is_word(b)).count()
        } else {
            rest.iter().take_while(|&&b| !is_word(b)).count()
        };
    }
    push(None, plain, content.len());
}

/// Tokenize a Dockerfile into segments covering all of `code`; `shell` classifies the `RUN`
/// commands (followed by Rouge's shell word rules).
pub(super) fn segments(
    code: &str,
    shell: &ForLang<'_>,
    out: &mut Vec<Segment>,
) -> Result<(), engine::Error> {
    if !Limits::DEFAULT.admits(code) {
        return Err(engine::Error);
    }
    let syntax = select_syntax("shell", "sh");
    let shell_words = words::for_tag("shell");
    let deadline = Deadline::start(&Limits::DEFAULT);
    engine::guarded(|| {
        let mut tokenizer = syntax.map(LineTokenizer::new);
        // Whether the next line starts in the `:run` state.
        let mut in_run = false;
        let mut base = 0usize;
        for line in code.split_inclusive('\n') {
            deadline.check()?;
            let bytes = line.as_bytes();
            let content = line.strip_suffix('\n').unwrap_or(line).as_bytes();
            let mut push = |class: Option<&'static str>, from: usize, to: usize| {
                if from < to {
                    out.push(Segment::new(class, base + from..base + to));
                }
            };
            let mut pos = 0usize;
            let mut run = std::mem::take(&mut in_run);
            if !run {
                if let Some([image, ws, as_kw, as_end, name, end]) = from_as(content) {
                    push(Some("k"), 0, 4);
                    push(Some("w"), 4, image);
                    push(Some("s"), image, ws);
                    push(Some("w"), ws, as_kw);
                    push(Some("k"), as_kw, as_end);
                    push(Some("w"), as_end, name);
                    push(Some("s"), name, end);
                    pos = end;
                } else if let Some((ws, kw)) = onbuild(content) {
                    push(Some("k"), 0, 7);
                    push(Some("w"), 7, ws);
                    push(Some("k"), ws, kw);
                    push(Some("s"), kw, content.len());
                    pos = content.len();
                } else if let Some(kw) = keyword(content, true) {
                    push(Some("k"), 0, kw);
                    push(Some("s"), kw, content.len());
                    pos = content.len();
                } else if let Some(kw) = run_keyword(bytes) {
                    push(Some("k"), 0, kw);
                    pos = kw;
                    run = true;
                    if let Some(t) = tokenizer.as_mut() {
                        t.reset();
                    }
                }
            }
            if run {
                // `:run`: the shell up to the line end; `\` + any character is an escape.
                let mut shell_segments = Vec::new();
                // `RUN`'s blanks took the newline: the commands start on the next line.
                in_run = pos == bytes.len() && bytes.last() == Some(&b'\n');
                while pos < bytes.len() {
                    let at = base + pos;
                    if bytes[pos] == b'\n' {
                        shell_segments.push(Segment::new(None, at..at + 1));
                        break;
                    }
                    if bytes[pos] == b'\\' {
                        let len = line[pos + 1..].chars().next().map_or(0, char::len_utf8);
                        // `\\.` takes a newline too (`/m`); a lone `\` at the end matches no
                        // rule (Rouge's error token).
                        let class = if len == 0 { "err" } else { "se" };
                        shell_segments.push(Segment::new(Some(class), at..at + 1 + len));
                        in_run = bytes.get(pos + 1) == Some(&b'\n');
                        pos += 1 + len;
                        if in_run {
                            break;
                        }
                        continue;
                    }
                    let end = run_chunk(line, pos);
                    match tokenizer.as_mut() {
                        Some(t) => {
                            let mut offset = at;
                            t.feed(&line[pos..end], &mut |event, stack| {
                                if let Event::Text(text) = event {
                                    shell_segments.push(Segment::new(
                                        shell.classify(stack),
                                        offset..offset + text.len(),
                                    ));
                                    offset += text.len();
                                }
                            })?;
                        }
                        None => shell_segments.push(Segment::new(None, at..base + end)),
                    }
                    pos = end;
                }
                out.extend(match shell_words {
                    Some(w) => w.refine(code, shell_segments),
                    None => shell_segments,
                });
            } else {
                let mut push = |class: Option<&'static str>, from: usize, to: usize| {
                    if from < to {
                        out.push(Segment::new(class, base + from..base + to));
                    }
                };
                root_text(content, pos, &mut push);
                push(None, content.len(), bytes.len());
            }
            base += line.len();
        }
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instruction_rules() {
        assert_eq!(keyword(b"ENTRYPOINT [\"x\"]", true), Some(10));
        assert_eq!(keyword(b"env A=1", true), Some(3));
        assert_eq!(keyword(b"ENVX", true), None);
        assert_eq!(keyword(b"RUN x", true), None);
        // `FROM a AS b`: the last ` AS `, case-insensitive.
        assert_eq!(from_as(b"FROM a as b"), Some([5, 6, 7, 9, 10, 11]));
        assert_eq!(from_as(b"FROM a"), None);
        assert_eq!(from_as(b"FROM a AS"), None);
        assert_eq!(onbuild(b"ONBUILD COPY . ."), Some((8, 12)));
        assert_eq!(onbuild(b"ONBUILD RUN x"), None);
        assert_eq!(run_keyword(b"RUN x\n"), Some(4));
        assert_eq!(run_keyword(b"onbuild  run  x"), Some(14));
        assert_eq!(run_keyword(b"RUN\n"), Some(4));
        assert_eq!(run_keyword(b"RUNX"), None);
        assert_eq!(run_chunk("a \\\" b \\\n", 0), 7);
    }

    /// The segments cover the code exactly once, in order, whatever the input.
    #[test]
    fn segments_cover_the_code() {
        let scheme = super::super::classes::Scheme::get(super::super::DEFAULT_VARIANT);
        for code in [
            "",
            "\\",
            "RUN",
            "RUN ",
            "RUN \\",
            "RUN \\\n",
            "RUN a \\\n\\\nb\n",
            "RUN\n\n",
            "FROM",
            "FROM AS AS AS",
            "FROM a AS b AS c\n",
            "ONBUILD",
            "ONBUILD RUN é \\é\n",
            "  FROM x\n#c\n= #x\n",
            "COPY a b\r\n",
            "é\u{202E}# x\n",
        ] {
            let mut out = Vec::new();
            segments(code, &scheme.for_lang("shell"), &mut out).unwrap();
            let mut pos = 0;
            for s in &out {
                assert_eq!(s.range.start, pos, "{code:?}: {out:?}");
                assert!(s.range.end > s.range.start, "{code:?}: {out:?}");
                pos = s.range.end;
            }
            assert_eq!(pos, code.len(), "{code:?}: {out:?}");
        }
    }
}
