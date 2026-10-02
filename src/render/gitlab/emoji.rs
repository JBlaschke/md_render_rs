//! `EmojiFilter` (GL §5.13): Unicode emoji and `:alpha_code:` become `<gl-emoji>` elements.
//!
//! Data: GitLab's emoji DB v4 (`https://gitlab.com/-/emojis/4/emojis.json`, the TanukiEmoji
//! index) and `fixtures/emojis/aliases.json`, compacted into `data/gitlab/emoji.json`.

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use dom_query::NodeRef;
use serde::Deserialize;

use super::dom;
use super::filters::FILTER_ITEM_LIMIT;

/// One emoji of GitLab's DB.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Emoji {
    pub name: String,
    pub chars: String,
    pub description: String,
    pub unicode_version: String,
}

#[derive(Deserialize)]
struct RawDb {
    emoji: Vec<(String, String, String, String)>,
    aliases: Vec<(String, String)>,
}

struct Db {
    entries: Vec<Emoji>,
    /// Names and aliases (TanukiEmoji `name_index` / `alpha_code_index`).
    by_name: HashMap<String, usize>,
    /// Codepoints plus the variant without U+FE0F (`codepoints_alternates`).
    by_chars: HashMap<String, usize>,
    /// Characters that can form an emoji element (excluding joiners/selectors/modifiers).
    emoji_chars: HashSet<char>,
}

static DB: LazyLock<Db> = LazyLock::new(|| {
    let raw: RawDb = serde_json::from_str(include_str!("../../../data/gitlab/emoji.json"))
        .unwrap_or(RawDb {
            emoji: Vec::new(),
            aliases: Vec::new(),
        });
    let mut db = Db {
        entries: Vec::with_capacity(raw.emoji.len()),
        by_name: HashMap::with_capacity(raw.emoji.len() + raw.aliases.len()),
        by_chars: HashMap::with_capacity(raw.emoji.len() * 2),
        emoji_chars: HashSet::new(),
    };
    for (i, (name, chars, description, unicode_version)) in raw.emoji.into_iter().enumerate() {
        db.by_name.entry(name.clone()).or_insert(i);
        db.by_chars.entry(chars.clone()).or_insert(i);
        let plain: String = chars.chars().filter(|&c| c != VS16).collect();
        db.by_chars.entry(plain).or_insert(i);
        for c in chars.chars() {
            if !c.is_ascii()
                && !is_modifier(c)
                && !matches!(c, VS15 | VS16 | ZWJ | KEYCAP | TAG_START..=TAG_END)
            {
                db.emoji_chars.insert(c);
            }
        }
        db.entries.push(Emoji {
            name,
            chars,
            description,
            unicode_version,
        });
    }
    for (alias, name) in raw.aliases {
        if let Some(&i) = db.by_name.get(&name) {
            db.by_name.entry(alias).or_insert(i);
        }
    }
    db
});

const VS15: char = '\u{FE0E}';
const VS16: char = '\u{FE0F}';
const ZWJ: char = '\u{200D}';
const KEYCAP: char = '\u{20E3}';
const TAG_START: char = '\u{E0020}';
const TAG_END: char = '\u{E007F}';
const BLACK_FLAG: char = '\u{1F3F4}';

fn is_modifier(c: char) -> bool {
    ('\u{1F3FB}'..='\u{1F3FF}').contains(&c)
}

fn is_regional_indicator(c: char) -> bool {
    ('\u{1F1E6}'..='\u{1F1FF}').contains(&c)
}

/// Looks an emoji up by name or alias (without colons).
pub(super) fn by_name(name: &str) -> Option<&'static Emoji> {
    DB.by_name.get(name).map(|&i| &DB.entries[i])
}

/// Looks an emoji up by its exact codepoints (or the variant without U+FE0F).
pub(super) fn by_chars(chars: &str) -> Option<&'static Emoji> {
    DB.by_chars.get(chars).map(|&i| &DB.entries[i])
}

/// End (char index) of one emoji element starting at `i`.
fn element_end(chars: &[char], i: usize) -> Option<usize> {
    let c = *chars.get(i)?;
    let at = |j: usize| chars.get(j).copied();
    if is_regional_indicator(c) {
        return Some(if at(i + 1).is_some_and(is_regional_indicator) {
            i + 2
        } else {
            i + 1
        });
    }
    if matches!(c, '0'..='9' | '#' | '*') {
        let mut j = i + 1;
        if at(j) == Some(VS16) {
            j += 1;
        }
        return (at(j) == Some(KEYCAP)).then_some(j + 1);
    }
    if c == BLACK_FLAG && at(i + 1).is_some_and(|t| (TAG_START..TAG_END).contains(&t)) {
        let mut j = i + 1;
        while at(j).is_some_and(|t| (TAG_START..TAG_END).contains(&t)) {
            j += 1;
        }
        if at(j) == Some(TAG_END) {
            return Some(j + 1);
        }
    }
    if !DB.emoji_chars.contains(&c) {
        return None;
    }
    let mut j = i + 1;
    if matches!(at(j), Some(VS15 | VS16)) {
        j += 1;
    }
    if at(j).is_some_and(is_modifier) {
        j += 1;
        if at(j) == Some(VS16) {
            j += 1;
        }
    }
    Some(j)
}

/// End of a maximal emoji sequence (elements joined by ZWJ), approximating unicode-emoji's
/// `REGEX_VALID_INCLUDE_TEXT`.
fn sequence_end(chars: &[char], i: usize) -> Option<usize> {
    let mut j = element_end(chars, i)?;
    while chars.get(j) == Some(&ZWJ) {
        match element_end(chars, j + 1) {
            Some(k) => j = k,
            None => break,
        }
    }
    Some(j)
}

/// A piece of rewritten text.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Piece<'a> {
    Text(String),
    Emoji(&'a Emoji),
}

/// Unicode pass (`emoji_unicode_element_unicode_filter`). `budget` counts replacements.
fn unicode_pass(text: &str, budget: &mut usize) -> Vec<Piece<'static>> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut buf = String::new();
    let mut i = 0;
    let mut replaced = 0usize;
    while i < chars.len() {
        let c = chars[i];
        let candidate = !c.is_ascii()
            || (matches!(c, '0'..='9' | '#' | '*')
                && matches!(chars.get(i + 1), Some(&VS16 | &KEYCAP)));
        if candidate
            && replaced < FILTER_ITEM_LIMIT
            && let Some(end) = sequence_end(&chars, i)
        {
            let seq: String = chars[i..end].iter().collect();
            // IGNORE_UNICODE_EMOJIS = %w[™ © ®]
            if !matches!(seq.as_str(), "™" | "©" | "®")
                && let Some(e) = by_chars(&seq)
            {
                if !buf.is_empty() {
                    out.push(Piece::Text(std::mem::take(&mut buf)));
                }
                out.push(Piece::Emoji(e));
                *budget += 1;
                replaced += 1;
                i = end;
                continue;
            }
            buf.push_str(&seq);
            i = end;
            continue;
        }
        buf.push(c);
        i += 1;
    }
    if !buf.is_empty() {
        out.push(Piece::Text(buf));
    }
    out
}

/// Ruby `[[:alnum:]]` (approximated with `char::is_alphanumeric`).
fn is_alnum(c: char) -> bool {
    c.is_alphanumeric()
}

/// Alpha-code pass on one text piece: TanukiEmoji's `alpha_code_pattern`,
/// `(?<=[^[:alnum:]:]|\n|^):(names):(?=[^[:alnum:]:]|$)`. Piece boundaries count as `^`/`$`
/// (they are the text-node edges or a `</gl-emoji>`/`<gl-emoji` tag in GitLab's HTML string).
fn alpha_pass(text: &str, out: &mut Vec<Piece<'static>>, budget: &mut usize, replaced: &mut usize) {
    let bytes = text.as_bytes();
    let mut last = 0;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b':' || *replaced >= FILTER_ITEM_LIMIT {
            i += 1;
            continue;
        }
        let before_ok = text[..i]
            .chars()
            .next_back()
            .is_none_or(|p| p == '\n' || !(is_alnum(p) || p == ':'));
        let Some(rel) = text[i + 1..].find(':') else {
            break;
        };
        let j = i + 1 + rel;
        let name = &text[i + 1..j];
        let after_ok = text[j + 1..]
            .chars()
            .next()
            .is_none_or(|n| n == '\n' || !(is_alnum(n) || n == ':'));
        if before_ok
            && after_ok
            && !name.is_empty()
            && let Some(e) = by_name(name)
        {
            if last < i {
                out.push(Piece::Text(text[last..i].to_owned()));
            }
            out.push(Piece::Emoji(e));
            *budget += 1;
            *replaced += 1;
            last = j + 1;
            i = j + 1;
            continue;
        }
        i += 1;
    }
    if last < text.len() {
        out.push(Piece::Text(text[last..].to_owned()));
    }
}

/// Both passes on one text node's content; `None` when nothing changes.
pub(super) fn rewrite(text: &str, budget: &mut usize) -> Option<Vec<Piece<'static>>> {
    let has_colon = text.contains(':');
    if text.is_ascii() && !has_colon {
        return None;
    }
    let pieces = if text.is_ascii() {
        vec![Piece::Text(text.to_owned())]
    } else {
        unicode_pass(text, budget)
    };
    let mut changed = pieces.iter().any(|p| matches!(p, Piece::Emoji(_)));
    let pieces = if has_colon {
        let mut out = Vec::with_capacity(pieces.len());
        let mut replaced = 0usize;
        for p in pieces {
            match p {
                Piece::Text(t) => {
                    let before = out.len();
                    alpha_pass(&t, &mut out, budget, &mut replaced);
                    if out[before..].iter().any(|p| matches!(p, Piece::Emoji(_))) {
                        changed = true;
                    }
                }
                e => out.push(e),
            }
        }
        out
    } else {
        pieces
    };
    changed.then_some(pieces)
}

/// `<gl-emoji title data-name data-unicode-version>CHARS</gl-emoji>` (Gitlab::Emoji.gl_emoji_tag).
fn gl_emoji<'a>(root: &NodeRef<'a>, e: &Emoji) -> NodeRef<'a> {
    let el = dom::new_element(
        root.tree,
        "gl-emoji",
        &[
            ("title", &e.description),
            ("data-name", &e.name),
            ("data-unicode-version", &e.unicode_version),
        ],
    );
    el.append_child(&dom::new_text(root.tree, &e.chars));
    el
}

/// The filter: text nodes outside `pre`/`code`/`tt` and outside `a[data-footnote-backref]`.
pub(super) fn filter(root: &NodeRef<'_>) {
    let mut count = 0usize;
    for id in dom::text_nodes(root) {
        if count >= FILTER_ITEM_LIMIT {
            break;
        }
        let t = root.tree.get_unchecked(&id);
        let content = dom::text_of(&t).unwrap_or_default();
        if content.is_ascii() && !content.contains(':') {
            continue;
        }
        if dom::has_ancestor(&t, &["pre", "code", "tt"], root) {
            continue;
        }
        let in_backref = {
            let mut cur = t.parent();
            let mut found = false;
            while let Some(p) = cur {
                if p.id == root.id {
                    break;
                }
                if dom::is(&p, "a") && p.has_attr("data-footnote-backref") {
                    found = true;
                    break;
                }
                cur = p.parent();
            }
            found
        };
        if in_backref {
            continue;
        }
        let Some(pieces) = rewrite(&content, &mut count) else {
            continue;
        };
        for p in pieces {
            let node = match p {
                Piece::Text(s) => dom::new_text(root.tree, &s),
                Piece::Emoji(e) => gl_emoji(root, e),
            };
            t.insert_before(&node);
        }
        t.remove_from_parent();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dom_query::Document;

    fn run(html: &str) -> String {
        let doc = Document::fragment(html);
        let root = doc.html_root();
        filter(&root);
        root.inner_html().to_string()
    }

    #[test]
    fn db_loads() {
        let smile = by_name("smile").expect("smile");
        assert_eq!(smile.chars, "😄");
        assert_eq!(smile.description, "grinning face with smiling eyes");
        assert_eq!(smile.unicode_version, "6.0");
        assert_eq!(
            by_name("grinning_face_with_smiling_eyes"),
            Some(smile),
            "alias"
        );
        assert!(by_name(":) ").is_none());
    }

    #[test]
    fn alpha_codes_with_boundaries() {
        assert_eq!(
            run("<p>Hi :smile: x</p>"),
            "<p>Hi <gl-emoji title=\"grinning face with smiling eyes\" data-name=\"smile\" data-unicode-version=\"6.0\">😄</gl-emoji> x</p>"
        );
        assert_eq!(run("<p>a:smile:</p>"), "<p>a:smile:</p>");
        assert_eq!(run("<p>:smile::smile:</p>"), "<p>:smile::smile:</p>");
        assert_eq!(
            run("<p>:nonexistent_emoji:</p>"),
            "<p>:nonexistent_emoji:</p>"
        );
        assert!(run("<p>(:+1:)</p>").contains("data-name=\"thumbsup\""));
        assert_eq!(
            run("<code>:smile:</code><pre>:smile:</pre>"),
            "<code>:smile:</code><pre>:smile:</pre>"
        );
    }

    #[test]
    fn unicode_emoji() {
        assert_eq!(
            run("<p>ok ✅</p>"),
            "<p>ok <gl-emoji title=\"check mark button\" data-name=\"white_check_mark\" data-unicode-version=\"6.0\">✅</gl-emoji></p>"
        );
        assert_eq!(run("<p>™ © ®</p>"), "<p>™ © ®</p>");
        // ZWJ sequence and skin tone stay one element.
        let fam = run("<p>👨‍👩‍👧</p>");
        assert_eq!(fam.matches("<gl-emoji").count(), 1, "{fam}");
        let thumbs = run("<p>👍🏽</p>");
        assert_eq!(thumbs.matches("<gl-emoji").count(), 1, "{thumbs}");
        // Keycap.
        assert_eq!(run("<p>1️⃣</p>").matches("<gl-emoji").count(), 1);
        assert_eq!(run("<p>123</p>"), "<p>123</p>");
        // Text-presentation selector: not in the index, left alone.
        assert_eq!(run("<p>↩\u{FE0E}</p>"), "<p>↩\u{FE0E}</p>");
    }

    #[test]
    fn footnote_backrefs_skipped() {
        assert_eq!(
            run("<a data-footnote-backref=\"\">↩</a>"),
            "<a data-footnote-backref=\"\">↩</a>"
        );
    }
}
