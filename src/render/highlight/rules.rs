//! Scope selector rules shared by both class schemes.
//!
//! A rule is `"[ANCESTOR ...] SCOPE"`: it matches a scope when `SCOPE` is a whole-atom prefix of
//! it (`string` matches `string.quoted.double.rust`, not `stringy`) and, for every `ANCESTOR`,
//! some scope further out on the stack has that ancestor as a whole-atom prefix (GH §7.5,
//! GL §8.4). Among the matching rules the most specific one wins; two specificity orders exist:
//!
//! * [`Precedence::AncestorFirst`] — GitLab's `scope_map.py` (GL §8.4): any rule with ancestors
//!   beats every plain rule (`100 + atoms`), otherwise the longer prefix wins.
//! * [`Precedence::LongestFirst`] — CSS-like (used for GitHub's PrettyLights map, GH §7.5/§7.6):
//!   the longer `SCOPE` prefix wins; at equal length the rule with more ancestor atoms wins.
//!
//! Ties keep the rule listed first (as `scope_map.py`'s strict `>` comparison does).

use std::collections::HashMap;

use syntect::parsing::Scope;

/// How rule specificity is ranked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Precedence {
    AncestorFirst,
    LongestFirst,
}

#[derive(Debug)]
struct Rule<T> {
    scope: Scope,
    ancestors: Vec<Scope>,
    specificity: u32,
    value: T,
}

/// A compiled, immutable rule table.
#[derive(Debug)]
pub(crate) struct RuleSet<T> {
    rules: Vec<Rule<T>>,
    /// Rule indices (in table order) keyed by the first atom of the rule's scope: a rule can only
    /// match a scope with the same first atom, so a lookup only scans one bucket.
    by_first_atom: HashMap<u16, Vec<usize>>,
    /// Selectors that failed to parse (a unit test asserts that there are none).
    rejected: Vec<&'static str>,
}

/// Parse a selector into `(ancestors, scope)`. Returns `None` for an empty or unparsable one.
fn parse_selector(selector: &str) -> Option<(Vec<Scope>, Scope)> {
    let mut parts: Vec<Scope> = Vec::new();
    for part in selector.split_whitespace() {
        parts.push(Scope::new(part).ok()?);
    }
    let scope = parts.pop()?;
    if scope.is_empty() {
        return None;
    }
    Some((parts, scope))
}

impl<T> RuleSet<T> {
    /// Compile `(selector, value)` pairs. Selectors that do not parse as syntect scopes are
    /// skipped and recorded (see [`RuleSet::rejected`]).
    pub(crate) fn new(
        precedence: Precedence,
        rules: impl IntoIterator<Item = (&'static str, T)>,
    ) -> Self {
        let mut out = Vec::new();
        let mut rejected = Vec::new();
        let mut by_first_atom: HashMap<u16, Vec<usize>> = HashMap::new();
        for (selector, value) in rules {
            let Some((ancestors, scope)) = parse_selector(selector) else {
                rejected.push(selector);
                continue;
            };
            let own = scope.len();
            let anc: u32 = ancestors.iter().map(|a| a.len()).sum();
            let specificity = match precedence {
                Precedence::AncestorFirst if ancestors.is_empty() => own,
                Precedence::AncestorFirst => 100 + own + anc,
                Precedence::LongestFirst => own * 64 + anc,
            };
            by_first_atom.entry(scope.atom_at(0)).or_default().push(out.len());
            out.push(Rule { scope, ancestors, specificity, value });
        }
        RuleSet { rules: out, by_first_atom, rejected }
    }

    /// Number of compiled rules.
    pub(crate) fn len(&self) -> usize {
        self.rules.len()
    }

    /// Selectors that could not be compiled.
    pub(crate) fn rejected(&self) -> &[&'static str] {
        &self.rejected
    }

    /// The most specific rule matching `scope` with `parents` (outermost first) further out.
    pub(crate) fn lookup(&self, scope: Scope, parents: &[Scope]) -> Option<&T> {
        let bucket = self.by_first_atom.get(&scope.atom_at(0))?;
        let mut best: Option<&Rule<T>> = None;
        for &i in bucket {
            let rule = &self.rules[i];
            if best.is_some_and(|b| b.specificity >= rule.specificity) {
                continue;
            }
            if !rule.scope.is_prefix_of(scope) {
                continue;
            }
            if !rule
                .ancestors
                .iter()
                .all(|a| parents.iter().any(|p| a.is_prefix_of(*p)))
            {
                continue;
            }
            best = Some(rule);
        }
        best.map(|r| &r.value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(x: &str) -> Scope {
        Scope::new(x).unwrap()
    }

    #[test]
    fn whole_atom_prefix() {
        let r = RuleSet::new(Precedence::LongestFirst, [("string", 1), ("string.quoted", 2)]);
        assert_eq!(r.lookup(s("string.quoted.double.rust"), &[]), Some(&2));
        assert_eq!(r.lookup(s("string.unquoted"), &[]), Some(&1));
        assert_eq!(r.lookup(s("stringy"), &[]), None);
        assert_eq!(r.lookup(s("comment"), &[]), None);
        assert_eq!(r.len(), 2);
    }

    #[test]
    fn ancestor_precedence_orders() {
        let rules = [("meta.mapping.key string", 10), ("string.quoted.double", 20)];
        let gl = RuleSet::new(Precedence::AncestorFirst, rules);
        let gh = RuleSet::new(Precedence::LongestFirst, rules);
        let parents = [s("source.json"), s("meta.mapping.key.json")];
        let scope = s("string.quoted.double.json");
        assert_eq!(gl.lookup(scope, &parents), Some(&10));
        assert_eq!(gh.lookup(scope, &parents), Some(&20));
        // Without the ancestor only the plain rule applies.
        assert_eq!(gl.lookup(scope, &[s("source.json")]), Some(&20));
    }

    #[test]
    fn ties_keep_first_rule() {
        let r = RuleSet::new(Precedence::LongestFirst, [("comment", 1), ("comment", 2)]);
        assert_eq!(r.lookup(s("comment.line"), &[]), Some(&1));
    }

    #[test]
    fn bad_selectors_are_recorded() {
        let r = RuleSet::new(Precedence::LongestFirst, [("", 1), ("  ", 2), ("ok", 3)]);
        assert_eq!(r.len(), 1);
        assert_eq!(r.rejected(), &["", "  "]);
    }
}
