//! YAML front matter as a table (README/file context only, GH §8.12; CQ §1.2 notes).
//!
//! GitHub parses the front matter with Ruby's Psych (YAML 1.1 flavoured scalar resolution)
//! and prints every value with Ruby's `to_s`. We read events from yaml-rust2 (which keeps
//! scalar styles, so `'yes'` stays a string while plain `yes` becomes `true`) and resolve
//! plain scalars the way Psych's `ScalarScanner` does.

use std::collections::HashMap;
use std::sync::OnceLock;

use regex::Regex;
use yaml_rust2::parser::{Event, Parser};
use yaml_rust2::scanner::TScalarStyle;

use crate::html::push_text;

/// Front matter found at the top of a document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct FrontMatter {
    /// Number of source lines the front matter occupies (both delimiter lines included).
    pub lines: usize,
    /// Length in bytes of the last (closing delimiter) line, for `data-sourcepos`.
    pub last_line_len: usize,
    /// The table markup (GH §8.12), without the `markdown-accessiblity-table` wrapper; `None`
    /// for an empty front matter block.
    pub table: Option<String>,
}

/// Result of [`extract`].
#[derive(Debug)]
pub(super) enum Extracted {
    /// No front matter at byte 0.
    None,
    /// Front matter recognised and rendered.
    Table(FrontMatter),
    /// Delimited block found but not usable (invalid YAML, not a mapping/sequence): the caller
    /// renders the document as plain Markdown (what the API does) and shows the message.
    Invalid(String),
}

/// Recognises a front matter block at byte 0 (`---` line, YAML, `---` or `...` line; trailing
/// blanks allowed on the delimiter lines, like Jekyll).
pub(super) fn extract(src: &str) -> Extracted {
    let mut lines = src.split_inclusive('\n');
    let Some(first) = lines.next() else {
        return Extracted::None;
    };
    if !first.ends_with('\n') || first.trim_end_matches(['\n', ' ', '\t']) != "---" {
        return Extracted::None;
    }
    let body_start = first.len();
    let mut offset = body_start;
    for (index, line) in lines.enumerate() {
        let count = index + 2; // 1-based line number of `line`
        let bare = line.trim_end_matches(['\n', ' ', '\t']);
        if bare == "---" || bare == "..." {
            let yaml = &src[body_start..offset];
            let last_line_len = line.trim_end_matches('\n').len();
            return match parse_yaml(yaml) {
                Ok(None) => Extracted::Table(FrontMatter {
                    lines: count,
                    last_line_len,
                    table: None,
                }),
                Ok(Some(v @ (Value::Map(_) | Value::Seq(_)))) => Extracted::Table(FrontMatter {
                    lines: count,
                    last_line_len,
                    table: Some(render_top(&v)),
                }),
                Ok(Some(Value::Scalar(_))) => {
                    Extracted::Invalid("front matter is not a YAML mapping".to_owned())
                }
                Err(e) => Extracted::Invalid(format!("invalid YAML front matter: {e}")),
            };
        }
        offset += line.len();
    }
    Extracted::None
}

// ------------------------------------------------------------------------------------------
// YAML -> value tree

#[derive(Debug, Clone, PartialEq)]
enum Value {
    /// Already converted to Ruby's `to_s` text.
    Scalar(String),
    Seq(Vec<Value>),
    /// Insertion-ordered; duplicate keys replace the value in place (Ruby `Hash`).
    Map(Vec<(String, Value)>),
}

/// Limits protecting against hostile front matter (deep nesting, alias bombs).
const MAX_DEPTH: usize = 64;
const MAX_NODES: usize = 50_000;

enum Frame {
    Seq(Vec<Value>, usize),
    Map(Vec<(String, Value)>, Option<String>, usize),
}

struct Builder {
    stack: Vec<Frame>,
    anchors: HashMap<usize, Value>,
    root: Option<Value>,
    nodes: usize,
}

impl Builder {
    fn count(&mut self, v: &Value) -> Result<(), String> {
        fn size(v: &Value) -> usize {
            match v {
                Value::Scalar(_) => 1,
                Value::Seq(items) => 1 + items.iter().map(size).sum::<usize>(),
                Value::Map(items) => 1 + items.iter().map(|(_, v)| 1 + size(v)).sum::<usize>(),
            }
        }
        self.nodes += size(v);
        if self.nodes > MAX_NODES {
            Err("front matter too large".to_owned())
        } else {
            Ok(())
        }
    }

    fn value(&mut self, v: Value, anchor: usize) {
        if anchor != 0 {
            self.anchors.insert(anchor, v.clone());
        }
        match self.stack.last_mut() {
            None => {
                if self.root.is_none() {
                    self.root = Some(v);
                }
            }
            Some(Frame::Seq(items, _)) => items.push(v),
            Some(Frame::Map(items, key, _)) => match key.take() {
                None => *key = Some(key_text(&v)),
                Some(k) => {
                    if let Some(slot) = items.iter_mut().find(|(ek, _)| *ek == k) {
                        slot.1 = v;
                    } else {
                        items.push((k, v));
                    }
                }
            },
        }
    }
}

/// Ruby `to_s` of a (rare) non-scalar mapping key; good enough for display.
fn key_text(v: &Value) -> String {
    match v {
        Value::Scalar(s) => s.clone(),
        Value::Seq(items) => {
            let parts: Vec<String> = items.iter().map(key_text).collect();
            format!("[{}]", parts.join(", "))
        }
        Value::Map(items) => {
            let parts: Vec<String> = items
                .iter()
                .map(|(k, v)| format!("{k} => {}", key_text(v)))
                .collect();
            format!("{{{}}}", parts.join(", "))
        }
    }
}

/// Parses the first YAML document; `Ok(None)` for an empty document.
fn parse_yaml(src: &str) -> Result<Option<Value>, String> {
    let mut parser = Parser::new_from_str(src);
    let mut b = Builder {
        stack: Vec::new(),
        anchors: HashMap::new(),
        root: None,
        nodes: 0,
    };
    loop {
        let (ev, _) = parser.next_token().map_err(|e| e.to_string())?;
        match ev {
            Event::StreamEnd | Event::DocumentEnd => break,
            Event::Nothing | Event::StreamStart | Event::DocumentStart => {}
            Event::Alias(id) => {
                let v = b
                    .anchors
                    .get(&id)
                    .cloned()
                    .unwrap_or(Value::Scalar(String::new()));
                b.count(&v)?;
                b.value(v, 0);
            }
            Event::Scalar(text, style, anchor, tag) => {
                let tag_suffix = tag.as_ref().map(|t| t.suffix.as_str());
                let v = Value::Scalar(resolve_scalar(&text, style, tag_suffix));
                b.count(&v)?;
                b.value(v, anchor);
            }
            Event::SequenceStart(anchor, _) => {
                if b.stack.len() >= MAX_DEPTH {
                    return Err("front matter nested too deeply".to_owned());
                }
                b.stack.push(Frame::Seq(Vec::new(), anchor));
            }
            Event::MappingStart(anchor, _) => {
                if b.stack.len() >= MAX_DEPTH {
                    return Err("front matter nested too deeply".to_owned());
                }
                b.stack.push(Frame::Map(Vec::new(), None, anchor));
            }
            Event::SequenceEnd => {
                if let Some(Frame::Seq(items, anchor)) = b.stack.pop() {
                    b.nodes += 1;
                    b.value(Value::Seq(items), anchor);
                }
            }
            Event::MappingEnd => {
                if let Some(Frame::Map(items, _, anchor)) = b.stack.pop() {
                    b.nodes += 1;
                    b.value(Value::Map(items), anchor);
                }
            }
        }
        if b.root.is_some() && b.stack.is_empty() {
            break;
        }
    }
    Ok(b.root)
}

// ------------------------------------------------------------------------------------------
// Psych scalar resolution + Ruby `to_s`

fn re(cell: &'static OnceLock<Regex>, pattern: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pattern).expect("static front matter regex"))
}

fn resolve_scalar(text: &str, style: TScalarStyle, tag: Option<&str>) -> String {
    if style != TScalarStyle::Plain || tag == Some("str") {
        return text.to_owned();
    }
    resolve_plain(text)
}

/// Psych `ScalarScanner#tokenize` for plain scalars, followed by Ruby `to_s`.
fn resolve_plain(s: &str) -> String {
    static INT: OnceLock<Regex> = OnceLock::new();
    static FLOAT: OnceLock<Regex> = OnceLock::new();
    static TIME: OnceLock<Regex> = OnceLock::new();
    static DATE: OnceLock<Regex> = OnceLock::new();

    match s {
        "" | "~" | "null" | "Null" | "NULL" => return String::new(),
        _ => {}
    }
    let lower = s.to_ascii_lowercase();
    if matches!(lower.as_str(), "yes" | "true" | "on") {
        return "true".to_owned();
    }
    if matches!(lower.as_str(), "no" | "false" | "off") {
        return "false".to_owned();
    }
    if matches!(lower.as_str(), ".inf" | "+.inf") {
        return "Infinity".to_owned();
    }
    if lower == "-.inf" {
        return "-Infinity".to_owned();
    }
    if lower == ".nan" {
        return "NaN".to_owned();
    }
    let date = re(&DATE, r"^(\d{4})-(\d{1,2})-(\d{1,2})$");
    if let Some(c) = date.captures(s) {
        let (y, m, d) = (num(&c[1]), num(&c[2]), num(&c[3]));
        if (1..=12).contains(&m) && d >= 1 && d <= days_in_month(y, m) {
            return format!("{y:04}-{m:02}-{d:02}");
        }
        return s.to_owned();
    }
    let time = re(
        &TIME,
        r"^(-?\d{4})-(\d{1,2})-(\d{1,2})(?:[Tt]|\s+)(\d{1,2}):(\d\d):(\d\d)(?:\.\d*)?\s*(Z|[-+]\d{1,2}(?::?\d\d)?)?$",
    );
    if let Some(c) = time.captures(s) {
        let (y, mo, d) = (&c[1], num(&c[2]), num(&c[3]));
        let (h, mi, se) = (num(&c[4]), num(&c[5]), num(&c[6]));
        let base = format!("{y}-{mo:02}-{d:02} {h:02}:{mi:02}:{se:02}");
        return match c.get(7).map(|m| m.as_str()) {
            Some("Z") => format!("{base} UTC"),
            Some(z) => format!("{base} {}", ruby_offset(z)),
            // Psych: `Time.at(utc)` in the server's zone; github.com runs in UTC.
            None => format!("{base} +0000"),
        };
    }
    let int = re(
        &INT,
        r"^(?:[-+]?0b[0-1_,]+|[-+]?0[0-7_,]+|[-+]?(?:0|[1-9][0-9_,]*)|[-+]?0x[0-9a-fA-F_,]+)$",
    );
    if int.is_match(s) {
        return ruby_integer(s).unwrap_or_else(|| s.to_owned());
    }
    let float = re(
        &FLOAT,
        r"^[-+]?(?:[0-9][0-9_,]*)?\.[0-9]*(?:[eE][-+]?[0-9]+)?$",
    );
    if float.is_match(s) && s.bytes().any(|b| b.is_ascii_digit()) {
        let clean: String = s.chars().filter(|&c| c != '_' && c != ',').collect();
        if let Ok(f) = clean.parse::<f64>() {
            return ruby_float(f);
        }
    }
    s.to_owned()
}

fn num(s: &str) -> u32 {
    s.parse().unwrap_or(0)
}

fn days_in_month(y: u32, m: u32) -> u32 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ if (y.is_multiple_of(4) && !y.is_multiple_of(100)) || y.is_multiple_of(400) => 29,
        _ => 28,
    }
}

/// `+2`, `-05:00`, `+0530` -> `+0200`, `-0500`, `+0530` (Ruby `Time#to_s` offset format).
fn ruby_offset(z: &str) -> String {
    let (sign, rest) = z.split_at(1);
    let digits: String = rest.chars().filter(char::is_ascii_digit).collect();
    let (h, m) = if rest.contains(':') {
        let mut it = rest.splitn(2, ':');
        (num(it.next().unwrap_or("0")), num(it.next().unwrap_or("0")))
    } else if digits.len() > 2 {
        (
            num(&digits[..digits.len() - 2]),
            num(&digits[digits.len() - 2..]),
        )
    } else {
        (num(&digits), 0)
    };
    format!("{sign}{h:02}{m:02}")
}

fn ruby_integer(s: &str) -> Option<String> {
    let clean: String = s.chars().filter(|&c| c != '_' && c != ',').collect();
    let (neg, body) = match clean.as_bytes().first() {
        Some(b'-') => (true, &clean[1..]),
        Some(b'+') => (false, &clean[1..]),
        _ => (false, clean.as_str()),
    };
    let v = if let Some(h) = body.strip_prefix("0x") {
        i128::from_str_radix(h, 16).ok()?
    } else if let Some(b) = body.strip_prefix("0b") {
        i128::from_str_radix(b, 2).ok()?
    } else if body.len() > 1 && body.starts_with('0') {
        i128::from_str_radix(&body[1..], 8).ok()?
    } else {
        body.parse::<i128>().ok()?
    };
    Some(if neg { (-v).to_string() } else { v.to_string() })
}

/// Ruby `Float#to_s`: shortest round-trip digits, fixed notation for exponents -4..16,
/// otherwise `d.de+XX`.
fn ruby_float(f: f64) -> String {
    if f.is_nan() {
        return "NaN".to_owned();
    }
    if f.is_infinite() {
        return if f > 0.0 {
            "Infinity".to_owned()
        } else {
            "-Infinity".to_owned()
        };
    }
    if f == 0.0 {
        return if f.is_sign_negative() {
            "-0.0".to_owned()
        } else {
            "0.0".to_owned()
        };
    }
    let sci = format!("{f:e}"); // e.g. "-1.5e-7", "1e20"
    let (mant, exp) = sci.split_once('e').unwrap_or((&sci, "0"));
    let exp: i32 = exp.parse().unwrap_or(0);
    let (neg, mant) = mant.strip_prefix('-').map_or((false, mant), |m| (true, m));
    let digits: String = mant.chars().filter(char::is_ascii_digit).collect();
    let sign = if neg { "-" } else { "" };
    if (-4..16).contains(&exp) {
        let point = exp + 1; // digits before the decimal point
        let s = if point <= 0 {
            format!("0.{}{}", "0".repeat(point.unsigned_abs() as usize), digits)
        } else if (point as usize) >= digits.len() {
            format!("{}{}.0", digits, "0".repeat(point as usize - digits.len()))
        } else {
            format!(
                "{}.{}",
                &digits[..point as usize],
                &digits[point as usize..]
            )
        };
        format!("{sign}{s}")
    } else {
        let frac = if digits.len() > 1 { &digits[1..] } else { "0" };
        let esign = if exp < 0 { '-' } else { '+' };
        format!(
            "{sign}{}.{frac}e{esign}{:02}",
            &digits[..1],
            exp.unsigned_abs()
        )
    }
}

// ------------------------------------------------------------------------------------------
// Table markup (exact whitespace from `S/ctx/frontmatter.ctx.html`)

fn render_top(v: &Value) -> String {
    match v {
        Value::Map(items) => {
            let mut s = String::from("<table>\n  <tbody>\n");
            for (k, v) in items {
                s.push_str("  <tr>\n    <th>");
                push_text(&mut s, k);
                s.push_str("</th>\n    <td>");
                render_inline(&mut s, v);
                s.push_str("</td>\n  </tr>\n");
            }
            s.push_str("  </tbody>\n</table>");
            s
        }
        other => {
            let mut s = String::new();
            render_inline(&mut s, other);
            s.truncate(s.trim_end_matches('\n').len());
            s
        }
    }
}

fn render_inline(s: &mut String, v: &Value) {
    match v {
        Value::Scalar(t) => push_text(s, t),
        Value::Seq(items) => {
            s.push_str("<table>\n  <tbody>\n");
            if !items.is_empty() {
                s.push_str("  <tr>\n");
                for item in items {
                    s.push_str("  <td><div dir=\"auto\">");
                    render_inline(s, item);
                    s.push_str("</div></td>\n");
                }
                s.push_str("  </tr>\n");
            }
            s.push_str("  </tbody>\n</table>\n");
        }
        Value::Map(items) => {
            s.push_str("<table>\n  <thead>\n  <tr>\n");
            for (k, _) in items {
                s.push_str("  <th>");
                push_text(s, k);
                s.push_str("</th>\n");
            }
            s.push_str("  </tr>\n  </thead>\n  <tbody>\n  <tr>\n");
            for (_, v) in items {
                s.push_str("  <td><div dir=\"auto\">");
                render_inline(s, v);
                s.push_str("</div></td>\n");
            }
            s.push_str("  </tr>\n  </tbody>\n</table>\n");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(src: &str) -> String {
        match extract(src) {
            Extracted::Table(fm) => fm.table.unwrap_or_default(),
            other => panic!("expected a table, got {other:?}"),
        }
    }

    #[test]
    fn detection() {
        assert!(matches!(extract("# no\n"), Extracted::None));
        assert!(matches!(extract("\n---\na: 1\n---\n"), Extracted::None));
        assert!(matches!(extract("---\na: 1\n"), Extracted::None));
        assert!(matches!(extract("---"), Extracted::None));
        match extract("---  \ntitle: x\n---\nbody") {
            Extracted::Table(fm) => {
                assert_eq!(fm.lines, 3);
                assert_eq!(fm.last_line_len, 3);
            }
            other => panic!("{other:?}"),
        }
        match extract("---\n---\n") {
            Extracted::Table(fm) => assert_eq!(fm.table, None),
            other => panic!("{other:?}"),
        }
        assert!(matches!(extract("---\na: [\n---\n"), Extracted::Invalid(_)));
        assert!(matches!(
            extract("---\njust text\n---\n"),
            Extracted::Invalid(_)
        ));
        assert!(matches!(extract("---\na: 1\n...\n"), Extracted::Table(_)));
    }

    #[test]
    fn exact_markup() {
        let t = table("---\ntitle: Usage\ntags: [Kubeconform, Usage]\nv:\n  fpt: '*'\n---\n");
        assert_eq!(
            t,
            "<table>\n  <tbody>\n  <tr>\n    <th>title</th>\n    <td>Usage</td>\n  </tr>\n  <tr>\n    <th>tags</th>\n    <td><table>\n  <tbody>\n  <tr>\n  <td><div dir=\"auto\">Kubeconform</div></td>\n  <td><div dir=\"auto\">Usage</div></td>\n  </tr>\n  </tbody>\n</table>\n</td>\n  </tr>\n  <tr>\n    <th>v</th>\n    <td><table>\n  <thead>\n  <tr>\n  <th>fpt</th>\n  </tr>\n  </thead>\n  <tbody>\n  <tr>\n  <td><div dir=\"auto\">*</div></td>\n  </tr>\n  </tbody>\n</table>\n</td>\n  </tr>\n  </tbody>\n</table>"
        );
        assert!(
            table("---\ne: []\n---\n")
                .contains("<td><table>\n  <tbody>\n  </tbody>\n</table>\n</td>")
        );
        assert!(table("---\nd: >\n  folded\n  text\n---\n").contains("<td>folded text\n</td>"));
        assert!(table("---\nh: a<b>&c\n---\n").contains("<td>a&lt;b&gt;&amp;c</td>"));
    }

    /// Psych scalar resolution + Ruby `to_s` (GH §8.12, CQ §1.2).
    #[test]
    fn scalars() {
        let cases = [
            ("2021-07-02T00:00:00Z", "2021-07-02 00:00:00 UTC"),
            ("2023-09-07T16:21:44+02:00", "2023-09-07 16:21:44 +0200"),
            ("2001-12-14 21:59:43.10 -5", "2001-12-14 21:59:43 -0500"),
            ("2001-12-15 2:59:43.10", "2001-12-15 02:59:43 +0000"),
            ("2021-7-2", "2021-07-02"),
            ("2021-02-30", "2021-02-30"),
            ("yes", "true"),
            ("Off", "false"),
            ("y", "y"),
            ("~", ""),
            ("null", ""),
            ("012", "10"),
            ("0x1F", "31"),
            ("1_000", "1000"),
            ("42", "42"),
            ("1.5", "1.5"),
            ("1.0", "1.0"),
            ("1.", "1.0"),
            (".5", "0.5"),
            ("1.0e+20", "1.0e+20"),
            ("0.00001", "1.0e-05"),
            (".inf", "Infinity"),
            ("plain words", "plain words"),
        ];
        for (input, want) in cases {
            assert_eq!(resolve_plain(input), want, "input {input:?}");
        }
        assert_eq!(
            resolve_scalar("yes", TScalarStyle::SingleQuoted, None),
            "yes"
        );
        assert_eq!(resolve_scalar("42", TScalarStyle::Plain, Some("str")), "42");
    }

    #[test]
    fn hostile_inputs() {
        let deep = format!("---\na: {}{}\n---\n", "[".repeat(200), "]".repeat(200));
        assert!(matches!(extract(&deep), Extracted::Invalid(_)));
        let bomb = "---\na: &a [x,x,x,x,x,x,x,x,x,x]\nb: &b [*a,*a,*a,*a,*a,*a,*a,*a,*a,*a]\nc: &c [*b,*b,*b,*b,*b,*b,*b,*b,*b,*b]\nd: &d [*c,*c,*c,*c,*c,*c,*c,*c,*c,*c]\ne: [*d,*d,*d,*d,*d,*d,*d,*d,*d,*d]\n---\n";
        assert!(matches!(extract(bomb), Extracted::Invalid(_)));
        let dup = table("---\na: 1\nb: 2\na: 3\n---\n");
        assert!(dup.find("<th>a</th>\n    <td>3</td>").is_some());
        assert_eq!(dup.matches("<th>a</th>").count(), 1);
    }
}
