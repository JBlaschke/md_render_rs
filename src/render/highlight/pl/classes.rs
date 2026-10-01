//! TextMate scope → PrettyLights `pl-*` class scheme for GitHub (GH §7.4–§7.7).
//!
//! The base is the PrettyLights map from primer/github-syntax-light (`github-light.map.json`,
//! GH §7.5), matched by longest whole-atom prefix. github.com's TextMate path uses exactly this
//! map; the grammars differ, though: GitHub runs Linguist's TextMate grammars (and tree-sitter
//! for some languages, GH §7.7), we run the Sublime syntaxes bundled by two-face, whose scope
//! names differ in places. [`Variant`] selects how much adaptation is layered on top of the map;
//! the evaluation in `eval.rs` measures each variant against github.com's output.

use std::collections::HashMap;
use std::sync::OnceLock;

use syntect::parsing::Scope;

use crate::render::highlight::rules::{Precedence, RuleSet};

/// A rule's class: `Some(class)` opens a span, `None` means "no span" (overriding a shorter
/// prefix that would have produced one).
pub(crate) type Class = Option<&'static str>;

/// The PrettyLights map (GH §7.5), verbatim and in file order.
pub(crate) const PRETTYLIGHTS_MAP: &[(&str, &str)] = &[
    ("comment", "pl-c"),
    ("punctuation.definition.comment", "pl-c"),
    ("string.comment", "pl-c"),
    ("constant", "pl-c1"),
    ("entity.name.constant", "pl-c1"),
    ("markup.raw", "pl-c1"),
    ("meta.diff.header", "pl-c1"),
    ("meta.module-reference", "pl-c1"),
    ("meta.output", "pl-c1"),
    ("meta.property-name", "pl-c1"),
    ("support", "pl-c1"),
    ("support.constant", "pl-c1"),
    ("support.variable", "pl-c1"),
    ("variable.language", "pl-c1"),
    ("variable.other.constant", "pl-c1"),
    ("string", "pl-s"),
    ("variable", "pl-v"),
    ("keyword.operator.symbole", "pl-kos"),
    ("keyword.other.mark", "pl-kos"),
    ("string.unquoted.import.ada", "pl-kos"),
    ("entity", "pl-e"),
    ("entity.name", "pl-en"),
    ("storage.modifier.import", "pl-smi"),
    ("storage.modifier.package", "pl-smi"),
    ("storage.type.java", "pl-smi"),
    ("variable.other", "pl-smi"),
    ("variable.parameter.function", "pl-smi"),
    ("source", "pl-s1"),
    ("entity.name.tag", "pl-ent"),
    ("markup.quote", "pl-ent"),
    ("keyword", "pl-k"),
    ("storage", "pl-k"),
    ("storage.type", "pl-k"),
    ("punctuation.definition.string", "pl-pds"),
    ("source.regexp", "pl-pds"),
    ("string.regexp.character-class", "pl-pds"),
    ("punctuation.section.embedded", "pl-pse"),
    ("string.regexp", "pl-sr"),
    ("constant.character.escape", "pl-cce"),
    ("source.ruby.embedded", "pl-sre"),
    ("string.regexp.arbitrary-repitition", "pl-sra"),
    ("sublimelinter.mark.warning", "pl-smw"),
    ("brackethighlighter.unmatched", "pl-bu"),
    ("invalid.broken", "pl-bu"),
    ("invalid.deprecated", "pl-bu"),
    ("invalid.unimplemented", "pl-bu"),
    ("message.error", "pl-bu"),
    ("sublimelinter.mark.error", "pl-bu"),
    ("invalid.illegal", "pl-ii"),
    ("carriage-return", "pl-c2"),
    ("markup.list", "pl-ml"),
    ("meta.separator", "pl-ms"),
    ("markup.heading", "pl-mh"),
    ("markup.italic", "pl-mi"),
    ("markup.bold", "pl-mb"),
    ("markup.deleted", "pl-md"),
    ("meta.diff.header.from-file", "pl-md"),
    ("punctuation.definition.deleted", "pl-md"),
    ("markup.inserted", "pl-mi1"),
    ("meta.diff.header.to-file", "pl-mi1"),
    ("punctuation.definition.inserted", "pl-mi1"),
    ("markup.changed", "pl-mc"),
    ("punctuation.definition.changed", "pl-mc"),
    ("markup.ignored", "pl-mi2"),
    ("markup.untracked", "pl-mi2"),
    ("meta.diff.range", "pl-mdr"),
    ("brackethighlighter.angle", "pl-ba"),
    ("brackethighlighter.curly", "pl-ba"),
    ("brackethighlighter.quote", "pl-ba"),
    ("brackethighlighter.round", "pl-ba"),
    ("brackethighlighter.square", "pl-ba"),
    ("brackethighlighter.tag", "pl-ba"),
    ("sublimelinter.gutter-mark", "pl-sg"),
    ("constant.other.reference.link", "pl-corl"),
    ("string.other.link", "pl-corl"),
];

/// Rules layered on top of the map for every language ([`Variant::Extended`] and
/// [`Variant::Hybrid`]): they translate Sublime-syntax scope names into the classes the
/// corresponding Linguist TextMate grammar scopes get on github.com (GH §7.6).
const EXTENDED: &[(&str, Class)] = &[];

/// Per-language overlays ([`Variant::Hybrid`]), keyed by Linguist `tm_scope`. For languages
/// github.com highlights with tree-sitter (GH §7.7) they approximate the tree-sitter capture →
/// class mapping; for TextMate languages they bridge grammar differences.
const OVERLAYS: &[(&str, &[(&str, Class)])] = &[];

/// Class scheme variants (see the module docs).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Variant {
    /// The PrettyLights map alone (GH §7.5, "minimum viable").
    Map,
    /// Map + [`EXTENDED`].
    Extended,
    /// Map + [`EXTENDED`] + the per-language [`OVERLAYS`].
    Hybrid,
}

impl Variant {
    pub(crate) const ALL: [Variant; 3] = [Variant::Map, Variant::Extended, Variant::Hybrid];
}

/// The variant used by `highlight::github` (chosen by measurement, see `eval.rs`).
pub(crate) const DEFAULT_VARIANT: Variant = Variant::Hybrid;

/// A compiled class scheme.
pub(crate) struct Scheme {
    base: RuleSet<Class>,
    overlays: HashMap<&'static str, RuleSet<Class>>,
}

impl Scheme {
    fn build(variant: Variant) -> Scheme {
        let map = PRETTYLIGHTS_MAP.iter().map(|&(s, c)| (s, Some(c)));
        let extended = EXTENDED.iter().copied();
        let base = match variant {
            Variant::Map => RuleSet::new(Precedence::LongestFirst, map),
            Variant::Extended | Variant::Hybrid => {
                RuleSet::new(Precedence::LongestFirst, map.chain(extended))
            }
        };
        let overlays = match variant {
            Variant::Hybrid => OVERLAYS
                .iter()
                .map(|&(lang, rules)| {
                    (lang, RuleSet::new(Precedence::LongestFirst, rules.iter().copied()))
                })
                .collect(),
            _ => HashMap::new(),
        };
        Scheme { base, overlays }
    }

    /// The scheme for `variant` (compiled once).
    pub(crate) fn get(variant: Variant) -> &'static Scheme {
        static SCHEMES: OnceLock<[Scheme; 3]> = OnceLock::new();
        let all = SCHEMES.get_or_init(|| Variant::ALL.map(Scheme::build));
        match variant {
            Variant::Map => &all[0],
            Variant::Extended => &all[1],
            Variant::Hybrid => &all[2],
        }
    }

    /// The overlay for a Linguist `tm_scope`, if any.
    pub(crate) fn overlay(&self, tm_scope: &str) -> Option<&RuleSet<Class>> {
        self.overlays.get(tm_scope)
    }

    /// Class for a pushed `scope` with `parents` further out: the overlay's verdict if it has a
    /// matching rule, else the base table's.
    pub(crate) fn classify(
        &self,
        overlay: Option<&RuleSet<Class>>,
        scope: Scope,
        parents: &[Scope],
    ) -> Class {
        if let Some(class) = overlay.and_then(|o| o.lookup(scope, parents)) {
            return *class;
        }
        self.base.lookup(scope, parents).copied().flatten()
    }

    #[cfg(test)]
    pub(crate) fn rejected(&self) -> Vec<&'static str> {
        let mut out = self.base.rejected().to_vec();
        for o in self.overlays.values() {
            out.extend_from_slice(o.rejected());
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(x: &str) -> Scope {
        Scope::new(x).unwrap()
    }

    #[test]
    fn all_selectors_compile() {
        for v in Variant::ALL {
            assert!(Scheme::get(v).rejected().is_empty(), "{v:?}: {:?}", Scheme::get(v).rejected());
        }
        assert_eq!(PRETTYLIGHTS_MAP.len(), 75);
    }

    #[test]
    fn map_longest_prefix() {
        let m = Scheme::get(Variant::Map);
        let c = |x: &str| m.classify(None, s(x), &[]);
        assert_eq!(c("comment.line.double-slash.rust"), Some("pl-c"));
        assert_eq!(c("entity.name.function.rust"), Some("pl-en"));
        assert_eq!(c("entity.name.tag.html"), Some("pl-ent"));
        assert_eq!(c("entity.other.attribute-name.html"), Some("pl-e"));
        assert_eq!(c("string.quoted.double.rust"), Some("pl-s"));
        assert_eq!(c("punctuation.definition.string.begin.rust"), Some("pl-pds"));
        assert_eq!(c("constant.character.escape.rust"), Some("pl-cce"));
        assert_eq!(c("storage.type.java"), Some("pl-smi"));
        assert_eq!(c("storage.type.function.rust"), Some("pl-k"));
        assert_eq!(c("meta.function.rust"), None);
        assert_eq!(c("punctuation.terminator.rust"), None);
    }
}
