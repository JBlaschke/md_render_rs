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
//! Ties keep the rule listed first.

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
    /// Indices of rules without ancestors, grouped by their first atom's prefix scope.
    any_ancestors: bool,
}

/// Parse a selector into `(ancestors, scope)`. Returns `None` for an empty or unparsable one.
fn parse_selector(selector: &str) -> Option<(Vec<Scope>, Scope)> {
    let mut parts: Vec<Scope> = Vec::new();
    for part in selector.split_whitespace() {
        parts.push(Scope::new(part).ok()?);
    }
    let scope = parts.pop()?;
    Some((parts, scope))
}

impl<T> RuleSet<T> {
    /// Compile `(selector, value)` pairs. Selectors that do not parse as syntect scopes are
    /// skipped (the tables are static, a unit test asserts that none is skipped).
    pub(crate) fn new(precedence: Precedence, rules: impl IntoIterator<Item = (&'static str, T)>) -> Self {
        let mut out = Vec::new();
        for (selector, value) in rules {
            let Some((ancestors, scope)) = parse_selector(selector) else {
                continue;
            };
            let own = scope.len();
            let anc: u32 = ancestors.iter().map(|a| a.len()).sum();
            let specificity = match precedence {
                Precedence::AncestorFirst if ancestors.is_empty() => own,
                Precedence::AncestorFirst => 100 + own + anc,
                Precedence::LongestFirst => own * 64 + anc,
            };
            out.push(Rule { scope, ancestors, specificity, value });
        }
        let any_ancestors = out.iter().any(|r| !r.ancestors.is_empty());
        RuleSet { rules: out, any_ancestors }
    }

    /// Number of compiled rules.
    pub(crate) fn len(&self) -> usize {
        self.rules.len()
    }

    /// The most specific rule matching `scope` with `parents` (outermost first) further out.
    pub(crate) fn lookup(&self, scope: Scope, parents: &[Scope]) -> Option<&T> {
        let mut best: Option<&Rule<T>> = None;
        for rule in &self.rules {
            if !rule.scope.is_prefix_of(scope) {
                continue;
            }
            if best.is_some_and(|b| b.specificity >= rule.specificity) {
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

    /// Whether any rule has ancestors (lookups then depend on the stack, not just the scope).
    pub(crate) fn has_ancestor_rules(&self) -> bool {
        self.any_ancestors
    }
}

/// Memoizes [`RuleSet::lookup`] for rule sets whose results only depend on the scope (no
/// ancestor rules), or on the scope plus the stack's root scope (the only ancestor the
/// language-conditional rules use is checked by the caller through the key).
pub(crate) struct Memo<'r, T> {
    rules: &'r RuleSet<T>,
    cache: HashMap<(Scope, Scope), Option<&'r T>>,
}

impl<'r, T> Memo<'r, T> {
    pub(crate) fn new(rules: &'r RuleSet<T>) -> Self {
        Memo { rules, cache: HashMap::new() }
    }

    /// Lookup with memoization when the result cannot depend on anything but `scope` and the
    /// root; falls through to a full lookup otherwise.
    pub(crate) fn lookup(&mut self, scope: Scope, parents: &[Scope], context_free: bool) -> Option<&'r T> {
        if !context_free {
            return self.rules.lookup(scope, parents);
        }
        let root = parents.first().copied().unwrap_or_default();
        *self
            .cache
            .entry((scope, root))
            .or_insert_with(|| self.rules.lookup(scope, parents))
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
        assert!(gl.has_ancestor_rules());
    }

    #[test]
    fn ties_keep_first_rule() {
        let r = RuleSet::new(Precedence::LongestFirst, [("comment", 1), ("comment", 2)]);
        assert_eq!(r.lookup(s("comment.line"), &[]), Some(&1));
    }
}
