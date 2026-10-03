//! `THIRD_PARTY_NOTICES.md` stays well-formed (Q-TESTS-1): its code fences are balanced (an
//! unclosed fence swallows every license text after it), every upstream section states its
//! license and copyright, every license text is a fenced block, and every in-page link points
//! at a heading of the file.

use std::collections::BTreeSet;

const NOTICES: &str = include_str!("../THIRD_PARTY_NOTICES.md");

/// The section headings of the notices with their body lines, fenced lines left out.
struct Section<'a> {
    level: usize,
    title: &'a str,
    body: Vec<&'a str>,
    fenced: usize,
}

/// An opening or closing code fence (CommonMark: up to 3 spaces of indent, 3+ backticks or
/// tildes): the fence character and its length.
fn fence(line: &str) -> Option<(char, usize, &str)> {
    let indent = line.len() - line.trim_start_matches(' ').len();
    if indent > 3 {
        return None;
    }
    let rest = &line[indent..];
    let c = rest.chars().next().filter(|c| *c == '`' || *c == '~')?;
    let n = rest.chars().take_while(|x| *x == c).count();
    (n >= 3).then(|| (c, n, &rest[n..]))
}

/// Split `text` into sections; an error if a fence is never closed.
fn try_sections(text: &str) -> Result<Vec<Section<'_>>, String> {
    let mut out: Vec<Section<'_>> = Vec::new();
    let mut open: Option<(char, usize, usize)> = None;
    for (i, line) in text.lines().enumerate() {
        let lineno = i + 1;
        match (open, fence(line)) {
            (None, Some((c, n, info))) => {
                if c == '`' && info.contains('`') {
                    return Err(format!(
                        "line {lineno}: a backtick fence's info string cannot contain backticks"
                    ));
                }
                open = Some((c, n, lineno));
                if let Some(s) = out.last_mut() {
                    s.fenced += 1;
                }
                continue;
            }
            (Some((c, n, _)), Some((c2, n2, info)))
                if c == c2 && n2 >= n && info.trim().is_empty() =>
            {
                open = None;
                continue;
            }
            (Some(_), _) => continue,
            (None, None) => {}
        }
        let level = line.chars().take_while(|c| *c == '#').count();
        if (1..=6).contains(&level) && line[level..].starts_with(' ') {
            out.push(Section {
                level,
                title: line[level..].trim(),
                body: Vec::new(),
                fenced: 0,
            });
        } else if let Some(s) = out.last_mut() {
            s.body.push(line);
        }
    }
    match open {
        Some((c, n, lineno)) => Err(format!(
            "the code fence `{}` opened on line {lineno} is never closed",
            c.to_string().repeat(n)
        )),
        None => Ok(out),
    }
}

/// The sections of `text`, whose fences must all be closed.
fn sections(text: &str) -> Vec<Section<'_>> {
    try_sections(text).unwrap_or_else(|e| panic!("THIRD_PARTY_NOTICES.md: {e}"))
}

/// GitHub's heading anchor (github-slugger): lowercase, keep letters, digits, `-`, `_` and
/// spaces, spaces become `-`.
fn slug(title: &str) -> String {
    title
        .to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | ' '))
        .map(|c| if c == ' ' { '-' } else { c })
        .collect()
}

/// The `### ` sections below the `## ` heading `group`.
fn group<'a, 's>(all: &'a [Section<'s>], group: &str) -> Vec<&'a Section<'s>> {
    let start = all
        .iter()
        .position(|s| s.level == 2 && s.title == group)
        .unwrap_or_else(|| panic!("THIRD_PARTY_NOTICES.md has no `## {group}` section"));
    all[start + 1..]
        .iter()
        .take_while(|s| s.level > 2)
        .filter(|s| s.level == 3)
        .collect()
}

#[test]
fn code_fences_are_balanced() {
    let all = sections(NOTICES);
    let fenced: usize = all.iter().map(|s| s.fenced).sum();
    assert!(
        fenced >= 7,
        "only {fenced} fenced blocks: the license texts are missing"
    );
    // The checker itself: an unclosed fence is caught.
    let broken = "# a\n\n```text\nMIT\n\n## b\n";
    let err = try_sections(broken)
        .err()
        .expect("unclosed fence not caught");
    assert!(err.contains("line 3"), "{err}");
    let longer = "# a\n\n````text\n```\n````\n## b\n";
    assert_eq!(sections(longer).len(), 2);
}

#[test]
fn every_upstream_section_states_license_and_copyright() {
    let all = sections(NOTICES);
    let upstream = group(&all, "Ported code and embedded data");
    assert!(upstream.len() >= 10, "{} upstream sections", upstream.len());
    for s in &upstream {
        let has = |label: &str| {
            s.body
                .iter()
                .any(|l| l.starts_with(&format!("* **{label}")))
        };
        assert!(
            has("License:") || has("Licenses and copyright:"),
            "`### {}` has no `* **License:**` line",
            s.title
        );
        assert!(
            s.body
                .iter()
                .any(|l| l.to_lowercase().contains("copyright")),
            "`### {}` names no copyright (say so when upstream names none)",
            s.title
        );
        assert!(
            has("Used for:"),
            "`### {}` has no `* **Used for:**` line",
            s.title
        );
    }
    // Each upstream is in the summary table and each table row has a section.
    let table: BTreeSet<String> = NOTICES
        .lines()
        .filter_map(|l| l.strip_prefix("| ["))
        .filter_map(|l| l.split_once("](#"))
        .filter_map(|(_, rest)| rest.split_once(')'))
        .map(|(anchor, _)| anchor.to_owned())
        .collect();
    let sections: BTreeSet<String> = upstream.iter().map(|s| slug(s.title)).collect();
    assert_eq!(
        table, sections,
        "summary table and upstream sections differ"
    );
}

#[test]
fn every_license_text_is_a_fenced_block() {
    let all = sections(NOTICES);
    let texts = group(&all, "License texts");
    assert!(texts.len() >= 7, "{} license texts", texts.len());
    for s in texts {
        assert!(
            s.fenced >= 1,
            "`### {}` has no fenced license text",
            s.title
        );
    }
}

#[test]
fn in_page_links_point_at_headings() {
    let anchors: BTreeSet<String> = sections(NOTICES).iter().map(|s| slug(s.title)).collect();
    assert_eq!(slug("diff.tmbundle"), "difftmbundle");
    assert_eq!(slug("Apache License 2.0"), "apache-license-20");
    let mut rest = NOTICES;
    let mut seen = 0;
    while let Some(i) = rest.find("](#") {
        let after = &rest[i + 3..];
        let end = after.find(')').expect("unclosed in-page link");
        let anchor = &after[..end];
        assert!(
            anchors.contains(anchor),
            "link to #{anchor}, but no heading has that anchor"
        );
        seen += 1;
        rest = &after[end..];
    }
    assert!(seen > 20, "only {seen} in-page links");
}
