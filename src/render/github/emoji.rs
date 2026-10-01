//! Emoji shortcodes and the `<g-emoji>` wrap list (GH §8.5, CQ §1.6, `S/emoji/README.md`).
//!
//! * `:name:` (regex `:([a-z0-9_+\-]+):`, case-sensitive) becomes the plain gemoji string;
//!   the 26 [`EmojiDb::wrapped`] names become `<g-emoji class="g-emoji" alias="NAME">`; the 23
//!   GitHub custom names become `<img class="emoji">` pointing into the style bundle.
//! * **First match decides** (per DOM text node): if the first regex match (known name or not)
//!   is at the start of the node or preceded by whitespace, every match in the node converts;
//!   otherwise the whole node is left alone.
//! * Raw Unicode equal to one of the 26 wrapped strings (incl. VS16) is wrapped as well, also
//!   inside ZWJ sequences.
//!
//! Data: `data/github/emoji.json`, compacted from gemoji `db/emoji.json` (MIT).

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

use serde::Deserialize;

/// The embedded emoji data (see `data/github/README.md`).
const EMOJI_JSON: &str = include_str!("../../../data/github/emoji.json");

#[derive(Deserialize)]
struct RawDb {
    emoji: HashMap<String, String>,
    wrapped: Vec<String>,
    custom: Vec<String>,
}

/// Lazily parsed emoji tables.
pub(super) struct EmojiDb {
    /// alias -> emoji string (exactly as stored by gemoji, VS16 included).
    pub(super) emoji: HashMap<String, String>,
    /// The 26 aliases GitHub wraps in `<g-emoji>`.
    pub(super) wrapped: HashSet<String>,
    /// The 23 non-Unicode GitHub emoji rendered as images.
    pub(super) custom: HashSet<String>,
    /// (emoji string, alias) for the wrapped set, longest strings first.
    wrapped_chars: Vec<(String, String)>,
}

pub(super) fn db() -> &'static EmojiDb {
    static DB: OnceLock<EmojiDb> = OnceLock::new();
    DB.get_or_init(|| {
        // The data file is part of the source tree; a parse failure is a build defect that
        // the unit tests catch. Degrade to "no emoji" rather than panicking at run time.
        let raw: RawDb = serde_json::from_str(EMOJI_JSON).unwrap_or(RawDb {
            emoji: HashMap::new(),
            wrapped: Vec::new(),
            custom: Vec::new(),
        });
        let mut wrapped_chars: Vec<(String, String)> = raw
            .wrapped
            .iter()
            .filter_map(|a| raw.emoji.get(a).map(|e| (e.clone(), a.clone())))
            .collect();
        wrapped_chars.sort_by(|a, b| b.0.len().cmp(&a.0.len()).then_with(|| a.0.cmp(&b.0)));
        EmojiDb {
            wrapped: raw.wrapped.into_iter().collect(),
            custom: raw.custom.into_iter().collect(),
            emoji: raw.emoji,
            wrapped_chars,
        }
    })
}

/// One piece of a converted text node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Seg {
    Text(String),
    /// `<g-emoji class="g-emoji" alias="ALIAS">CHARS</g-emoji>`
    GEmoji { alias: String, chars: String },
    /// `<img class="emoji" ...>` for a GitHub custom emoji.
    Custom(String),
}

fn is_name_byte(b: u8) -> bool {
    b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'_' | b'+' | b'-')
}

/// Leftmost-first, non-overlapping matches of `:([a-z0-9_+\-]+):` as byte ranges of the whole
/// `:name:`.
fn shortcode_matches(s: &str) -> Vec<(usize, usize)> {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b':' {
            let mut j = i + 1;
            while j < b.len() && is_name_byte(b[j]) {
                j += 1;
            }
            if j > i + 1 && j < b.len() && b[j] == b':' {
                out.push((i, j + 1));
                i = j + 1;
                continue;
            }
        }
        i += 1;
    }
    out
}

/// Ruby `\s`.
fn is_ruby_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r' | '\x0b' | '\x0c')
}

/// Converts one text node. Returns `None` when nothing changes.
pub(super) fn convert(s: &str) -> Option<Vec<Seg>> {
    let db = db();
    let mut segs: Vec<Seg> = Vec::new();
    let mut changed = false;

    let matches = if s.contains(':') { shortcode_matches(s) } else { Vec::new() };
    let first_ok = matches.first().is_some_and(|&(start, _)| {
        start == 0 || s[..start].chars().next_back().is_some_and(is_ruby_space)
    });
    if first_ok {
        let mut last = 0;
        for (start, end) in matches {
            let name = &s[start + 1..end - 1];
            let seg = if db.custom.contains(name) {
                Seg::Custom(name.to_owned())
            } else if let Some(e) = db.emoji.get(name) {
                if db.wrapped.contains(name) {
                    Seg::GEmoji { alias: name.to_owned(), chars: e.clone() }
                } else {
                    Seg::Text(e.clone())
                }
            } else {
                continue;
            };
            push_text(&mut segs, &s[last..start]);
            push_seg(&mut segs, seg);
            last = end;
            changed = true;
        }
        push_text(&mut segs, &s[last..]);
    } else {
        segs.push(Seg::Text(s.to_owned()));
    }

    // Raw Unicode wrapping on the remaining text (GH §8.5: "from shortcode and from raw
    // Unicode in the source").
    let mut out: Vec<Seg> = Vec::with_capacity(segs.len());
    for seg in segs {
        match seg {
            Seg::Text(t) => {
                if wrap_unicode(&t, &mut out, db) {
                    changed = true;
                }
            }
            other => out.push(other),
        }
    }
    changed.then_some(out)
}

fn push_text(segs: &mut Vec<Seg>, t: &str) {
    if t.is_empty() {
        return;
    }
    if let Some(Seg::Text(prev)) = segs.last_mut() {
        prev.push_str(t);
    } else {
        segs.push(Seg::Text(t.to_owned()));
    }
}

fn push_seg(segs: &mut Vec<Seg>, seg: Seg) {
    match seg {
        Seg::Text(t) => push_text(segs, &t),
        other => segs.push(other),
    }
}

/// Splits `t` at occurrences of the wrapped emoji strings; returns whether any was found.
fn wrap_unicode(t: &str, out: &mut Vec<Seg>, db: &EmojiDb) -> bool {
    // All 26 wrapped strings are a non-ASCII base character followed by U+FE0F.
    if t.is_ascii() || !t.contains('\u{fe0f}') {
        push_seg(out, Seg::Text(t.to_owned()));
        return false;
    }
    let mut found = false;
    let mut last = 0;
    let mut i = 0;
    while i < t.len() {
        let rest = &t[i..];
        if let Some((chars, alias)) =
            db.wrapped_chars.iter().find(|(c, _)| rest.starts_with(c.as_str()))
        {
            push_text(out, &t[last..i]);
            out.push(Seg::GEmoji { alias: alias.clone(), chars: chars.clone() });
            i += chars.len();
            last = i;
            found = true;
            continue;
        }
        i += rest.chars().next().map_or(1, char::len_utf8);
    }
    push_text(out, &t[last..]);
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(s: &str) -> String {
        match convert(s) {
            None => s.to_owned(),
            Some(segs) => segs
                .iter()
                .map(|seg| match seg {
                    Seg::Text(t) => t.clone(),
                    Seg::GEmoji { alias, chars } => format!("<g:{alias}:{chars}>"),
                    Seg::Custom(n) => format!("<img:{n}>"),
                })
                .collect(),
        }
    }

    #[test]
    fn data_loads() {
        let db = db();
        assert_eq!(db.emoji.len(), 1913);
        assert_eq!(db.wrapped.len(), 26);
        assert_eq!(db.custom.len(), 23);
        assert_eq!(db.emoji["heart"], "❤️");
        assert_eq!(db.emoji["+1"], "👍");
    }

    /// `S/emoji/README.md` rule 6 and `S/probes/emoji{3,4,5}.gfm.html`.
    #[test]
    fn first_match_decides() {
        let cases = [
            ("a [:smile:] b", "a [:smile:] b"),
            ("[:smile:] c", "[:smile:] c"),
            ("1:smile:2 :smile:", "1:smile:2 :smile:"),
            (":smile: 1:smile:2", "😄 1😄2"),
            ("A (:smile:) B :smile:.", "A (:smile:) B :smile:."),
            ("see :smile: and x:smile:", "see 😄 and x😄"),
            ("a :notanemoji: then (:smile:)", "a :notanemoji: then (😄)"),
            ("Joined:smile:no", "Joined:smile:no"),
            ("p :smile:a", "p 😄a"),
            ("x :smile::smile: y", "x 😄😄 y"),
            ("Note: see :smile:", "Note: see 😄"),
            ("time 10:30 :smile:", "time 10:30 😄"),
            ("M :SMILE: N", "M :SMILE: N"),
            (" a:smile:", " a:smile:"),
            ("a\n:smile: b(:rocket:)", "a\n😄 b(🚀)"),
        ];
        for (input, want) in cases {
            assert_eq!(render(input), want, "input {input:?}");
        }
    }

    #[test]
    fn wrapped_and_custom() {
        assert_eq!(render(":warning: x"), "<g:warning:⚠️> x");
        assert_eq!(render("go ↗️ not ↗"), "go <g:arrow_upper_right:↗️> not ↗");
        assert_eq!(render("🧑\u{200d}✈️"), "🧑\u{200d}<g:airplane:✈️>");
        assert_eq!(render(":octocat: :shipit:"), "<img:octocat> <img:shipit>");
        assert_eq!(render("plain text"), "plain text");
        assert!(convert("no emoji here").is_none());
        assert!(convert("x:smile:").is_none());
    }
}
