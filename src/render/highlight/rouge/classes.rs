//! TextMate scope → Rouge short-class scheme for GitLab (GL §8.3/§8.4).
//!
//! A port of `R/tools/scope_map.py` (the 214 rules of `R/tools/scope_map_rules.txt`, verbatim
//! and in the same order) plus measured improvements ([`Variant::Improved`]):
//!
//! ```text
//! for scope in scope_stack (innermost -> outermost):
//!     rule = most specific rule matching scope   // whole-atom prefix, longest wins;
//!                                                // "ANCESTOR SCOPE" rules beat plain rules
//!                                                // when the ancestor is further out
//!     if no rule or rule == INHERIT: continue    // INHERIT = transparent delimiter scopes
//!     return rule                                // a class; "" means no span
//! return ""
//! ```

use std::collections::HashMap;
use std::sync::OnceLock;

use syntect::parsing::Scope;

use crate::render::highlight::rules::{Precedence, RuleSet};

/// What a rule says about a scope.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Rule {
    /// Transparent: look at the enclosing scope.
    Inherit,
    /// Plain text, no span (Rouge `Text`).
    Plain,
    /// A Rouge short class.
    Span(&'static str),
}

use Rule::{Inherit, Plain, Span};

/// `scope_map.py` RULES (GL §8.4), verbatim and in order (ties keep the first rule).
pub(crate) const BASELINE_RULES: &[(&str, Rule)] = &[
    ("punctuation.definition.string", Inherit),
    ("punctuation.definition.comment", Inherit),
    ("punctuation.definition.variable", Inherit),
    ("punctuation.definition.heading", Inherit),
    ("punctuation.definition.bold", Inherit),
    ("punctuation.definition.italic", Inherit),
    ("punctuation.definition.raw", Inherit),
    ("punctuation.definition.constant", Inherit),
    ("punctuation.definition.keyword", Inherit),
    ("punctuation.definition.annotation", Inherit),
    ("punctuation.definition.decorator", Inherit),
    ("punctuation.definition.entity", Inherit),
    ("punctuation.definition.symbol", Inherit),
    ("punctuation.definition.inserted", Inherit),
    ("punctuation.definition.deleted", Inherit),
    ("punctuation.definition.from-file", Inherit),
    ("punctuation.definition.to-file", Inherit),
    ("punctuation.definition.range", Inherit),
    ("punctuation.definition.preprocessor", Inherit),
    ("punctuation.definition.directive", Inherit),
    ("punctuation.definition.regex", Inherit),
    ("comment", Span("c")),
    ("comment.line", Span("c1")),
    ("comment.line.shebang", Span("ch")),
    ("comment.block", Span("cm")),
    ("comment.block.documentation", Span("cd")),
    ("comment.line.documentation", Span("cd")),
    ("comment.line.double-slash.documentation", Span("cd")),
    ("comment.line.triple-slash", Span("cd")),
    ("meta.shebang", Span("ch")),
    ("meta.preprocessor", Span("cp")),
    ("keyword.control.directive", Span("cp")),
    ("keyword.control.import.include", Span("cp")),
    ("string.quoted.other.lt-gt.include", Span("cpf")),
    ("meta.preprocessor.include string", Span("cpf")),
    ("keyword", Span("k")),
    ("keyword.control", Span("k")),
    ("keyword.control.import", Span("kn")),
    ("keyword.control.export", Span("kn")),
    ("keyword.other.import", Span("kn")),
    ("keyword.other.use", Span("kn")),
    ("keyword.other.package", Span("kn")),
    ("keyword.other.namespace", Span("kn")),
    ("keyword.declaration", Span("kd")),
    ("keyword.other.fn", Span("kd")),
    ("storage.type.function", Span("kd")),
    ("storage.type.class", Span("kd")),
    ("storage.type.struct", Span("kd")),
    ("storage.type.enum", Span("kd")),
    ("storage.type.trait", Span("kd")),
    ("storage.type.impl", Span("kd")),
    ("storage.type.module", Span("kd")),
    ("storage.type.namespace", Span("kd")),
    ("storage.type.interface", Span("kd")),
    ("storage.type.type", Span("kd")),
    ("storage.modifier", Span("kd")),
    ("storage.type", Span("kt")),
    ("storage.type.primitive", Span("kt")),
    ("storage.type.builtin", Span("kt")),
    ("support.type", Span("kt")),
    ("support.type.primitive", Span("kt")),
    ("keyword.type", Span("kt")),
    ("keyword.other.reserved", Span("kr")),
    ("keyword.operator", Span("o")),
    ("keyword.operator.word", Span("ow")),
    ("keyword.operator.logical.python", Span("ow")),
    ("keyword.operator.new", Span("k")),
    ("keyword.operator.expression", Span("ow")),
    ("constant.language", Span("kc")),
    ("constant.language.boolean", Span("kc")),
    ("constant.language.null", Span("kc")),
    ("variable.language", Span("bp")),
    ("variable.language.this", Span("k")),
    ("variable.language.self", Span("bp")),
    ("entity.name.function", Span("nf")),
    ("entity.name.function.preprocessor", Span("cp")),
    ("meta.function-call entity.name.function", Span("nf")),
    ("variable.function", Span("nf")),
    ("support.function", Span("nb")),
    ("support.function.builtin", Span("nb")),
    ("entity.name.class", Span("nc")),
    ("entity.name.type", Span("nc")),
    ("entity.name.struct", Span("nc")),
    ("entity.name.enum", Span("nc")),
    ("entity.name.trait", Span("nc")),
    ("entity.name.interface", Span("nc")),
    ("entity.name.impl", Span("nc")),
    ("entity.other.inherited-class", Span("nc")),
    ("support.class", Span("nc")),
    ("entity.name.namespace", Span("nn")),
    ("entity.name.module", Span("nn")),
    ("entity.name.package", Span("nn")),
    ("entity.name.section", Span("nn")),
    ("entity.name.tag", Span("nt")),
    ("punctuation.definition.tag", Span("nt")),
    ("entity.other.attribute-name", Span("na")),
    ("entity.other.attribute-name.class.css", Span("nc")),
    ("entity.other.attribute-name.id.css", Span("nn")),
    ("entity.other.pseudo-class", Span("nd")),
    ("entity.other.pseudo-element", Span("nd")),
    ("entity.name.label", Span("nl")),
    ("entity.name.decorator", Span("nd")),
    ("meta.annotation", Span("nd")),
    ("meta.decorator", Span("nd")),
    ("storage.type.annotation", Span("nd")),
    ("entity.name.constant", Span("no")),
    ("variable.other.constant", Span("no")),
    ("constant.other", Span("no")),
    ("support.constant", Span("no")),
    ("entity.name.macro", Span("nf")),
    ("support.macro", Span("nf")),
    ("variable.parameter", Span("n")),
    ("variable.other.readwrite.instance", Span("vi")),
    ("variable.other.readwrite.class", Span("vc")),
    ("variable.other.readwrite.global", Span("vg")),
    ("variable.other.member", Span("py")),
    ("variable.other.property", Span("py")),
    ("support.variable.property", Span("py")),
    ("meta.property-name", Span("py")),
    ("support.type.property-name", Span("py")),
    ("support.type.property-name.css", Span("py")),
    ("support.type.property-name.json", Span("nl")),
    ("meta.mapping.key string", Span("na")),
    ("entity.name.tag.yaml", Span("na")),
    ("variable.other", Span("n")),
    ("variable", Span("nv")),
    ("variable.other.readwrite.shell", Span("nv")),
    ("variable.other.normal.shell", Span("nv")),
    ("support.function.builtin.shell", Span("nb")),
    ("entity.name.entity.other", Span("ni")),
    ("constant.character.entity", Span("ni")),
    ("entity.other.attribute-name.parameter", Span("nt")),
    ("string", Span("s")),
    ("string.quoted.single", Span("s1")),
    ("string.quoted.double", Span("s2")),
    ("string.quoted.triple", Span("s")),
    ("string.quoted.docstring", Span("sd")),
    ("comment.block.documentation.python", Span("sd")),
    ("string.quoted.other", Span("s")),
    ("string.quoted.backtick", Span("sb")),
    ("string.interpolated", Span("sb")),
    ("string.template", Span("s2")),
    ("string.regexp", Span("sr")),
    ("string.unquoted", Span("s")),
    ("string.unquoted.heredoc", Span("sh")),
    ("string.quoted.heredoc", Span("sh")),
    ("string.other.link", Span("nv")),
    ("constant.other.symbol", Span("ss")),
    ("string.symbol", Span("ss")),
    ("constant.character", Span("sc")),
    ("constant.character.escape", Span("se")),
    ("constant.character.format", Span("si")),
    ("constant.other.placeholder", Span("si")),
    ("meta.interpolation", Span("si")),
    ("punctuation.section.interpolation", Span("si")),
    ("punctuation.definition.template-expression", Span("si")),
    ("punctuation.section.embedded", Span("si")),
    ("storage.type.string", Span("sa")),
    ("constant.numeric", Span("m")),
    ("constant.numeric.integer", Span("mi")),
    ("constant.numeric.decimal", Span("mi")),
    ("constant.numeric.float", Span("mf")),
    ("constant.numeric.integer.hexadecimal", Span("mh")),
    ("constant.numeric.hex", Span("mh")),
    ("constant.numeric.integer.octal", Span("mo")),
    ("constant.numeric.octal", Span("mo")),
    ("constant.numeric.integer.binary", Span("mb")),
    ("constant.numeric.binary", Span("mb")),
    ("constant.other.date", Span("ld")),
    ("constant.other.datetime", Span("ld")),
    ("constant.other.color", Span("mh")),
    ("punctuation", Span("p")),
    ("punctuation.separator.key-value.mapping.yaml", Span("pi")),
    ("punctuation.definition.block.sequence.item.yaml", Span("pi")),
    ("punctuation.accessor", Span("p")),
    ("punctuation.separator.namespace", Span("p")),
    ("markup.heading", Span("gh")),
    ("markup.heading.1", Span("gh")),
    ("markup.heading.2", Span("gu")),
    ("markup.heading.3", Span("gu")),
    ("markup.heading.4", Span("gu")),
    ("markup.heading.5", Span("gu")),
    ("markup.heading.6", Span("gu")),
    ("meta.diff.header", Span("gh")),
    ("meta.diff.header.from-file", Span("gd")),
    ("meta.diff.header.to-file", Span("gi")),
    ("meta.header.from-file", Span("gd")),
    ("meta.header.to-file", Span("gi")),
    ("meta.diff.range", Span("p")),
    ("meta.range.unified", Span("p")),
    ("meta.toc-list.line-number.diff", Span("p")),
    ("punctuation.separator.decimal", Inherit),
    ("meta.mapping.key.json string", Span("nl")),
    ("meta.diff.index", Span("gh")),
    ("markup.inserted", Span("gi")),
    ("markup.deleted", Span("gd")),
    ("markup.changed", Span("gh")),
    ("markup.bold", Span("gs")),
    ("markup.italic", Span("ge")),
    ("markup.underline.link", Span("sx")),
    ("markup.raw", Span("sb")),
    ("markup.raw.code-fence", Span("sb")),
    ("markup.quote", Span("gt")),
    ("markup.list.numbered.bullet", Span("p")),
    ("markup.list.unnumbered.bullet", Span("p")),
    ("punctuation.definition.list_item", Span("p")),
    ("meta.link.inline.description", Span("nv")),
    ("constant.other.language-name", Span("nl")),
    ("markup.output", Span("go")),
    ("markup.prompt", Span("gp")),
    ("meta.prompt", Span("gp")),
    ("invalid", Span("err")),
    ("invalid.illegal", Span("err")),
    ("invalid.deprecated", Span("err")),
];

/// Rules that replace (same selector) or extend [`BASELINE_RULES`] for every language in
/// [`Variant::Improved`].
const IMPROVED_RULES: &[(&str, Rule)] = &[];

/// `Rouge::Lexers::Rust`: strings `s` (placeholders included), chars `sc`, `<`/`>` operators,
/// macros and attributes `nd` throughout, `->` a keyword, lifetimes `nv`, type names plain
/// (`n` via the word rules).
const RUST: &[(&str, Rule)] = &[
    ("string.quoted.double", Span("s")),
    ("string.quoted.single", Span("sc")),
    ("constant.other.placeholder", Inherit),
    ("punctuation.definition.generic", Span("o")),
    ("support.macro", Span("nd")),
    ("meta.annotation", Span("nd")),
    ("meta.annotation punctuation", Span("nd")),
    ("variable.annotation", Span("nd")),
    ("meta.function.return-type punctuation.separator", Span("k")),
    ("storage.modifier.lifetime", Span("nv")),
    ("entity.name.label", Span("nv")),
    ("entity.name.struct", Plain),
    ("entity.name.impl", Plain),
    ("entity.name.type", Plain),
    ("entity.name.enum", Plain),
    ("entity.name.trait", Plain),
    ("variable.other.member", Plain),
    ("constant.numeric.integer.hexadecimal", Span("mi")),
];

/// `Rouge::Lexers::Python`: string delimiters (and prefixes) `sh`, contents `s`, docstrings
/// alike; `def`/`class` plain keywords; interpolations lexed as code; `self` and calls `n`.
const PYTHON: &[(&str, Rule)] = &[
    ("string", Span("s")),
    ("punctuation.definition.string", Span("sh")),
    ("comment.block.documentation", Span("s")),
    ("comment.block.documentation punctuation.definition.comment", Span("sh")),
    ("storage.type.function", Span("k")),
    ("storage.type.class", Span("k")),
    ("keyword.declaration", Span("k")),
    ("meta.import-name", Span("nn")),
    ("meta.statement.import meta.qualified-name", Span("nn")),
    ("meta.interpolation", Plain),
    ("storage.modifier.conversion", Span("si")),
    ("variable.language", Plain),
    ("variable.function", Plain),
    ("variable.annotation", Span("nd")),
    ("entity.other.inherited-class", Plain),
    ("punctuation.separator.annotation", Span("o")),
    ("punctuation.definition.numeric", Inherit),
    ("source.regexp constant", Span("s")),
    ("source.regexp keyword", Span("s")),
];

/// `Rouge::Lexers::Javascript` / `Typescript`: quote delimiters `dl`, template literals `s2`
/// with `p` interpolation braces, names via the word rules (`nx`), object keys `na`, `=>` and
/// `?:` as in the lexer.
const JAVASCRIPT: &[(&str, Rule)] = &[
    ("punctuation.definition.string", Span("dl")),
    ("string.quoted.other", Span("s2")),
    ("string.quoted.other punctuation.definition.string", Span("s2")),
    ("string.template", Span("s2")),
    ("string.template punctuation.definition.string", Span("s2")),
    ("punctuation.section.interpolation", Span("p")),
    ("punctuation.definition.template-expression", Span("p")),
    ("meta.interpolation", Plain),
    ("meta.template.expression", Plain),
    ("meta.embedded", Plain),
    ("string.regexp keyword", Span("sr")),
    ("string.regexp punctuation", Span("sr")),
    ("keyword.operator.ternary", Span("p")),
    ("storage.type.function.arrow", Span("o")),
    ("variable.other", Plain),
    ("variable.parameter", Plain),
    ("variable.language", Plain),
    ("support.type.object", Plain),
    ("support.variable", Plain),
    ("meta.property.object", Plain),
    ("meta.mapping.key", Span("na")),
    ("comment.block.documentation", Span("cm")),
    ("meta.brace", Span("p")),
    ("keyword.operator.type.annotation", Span("p")),
    ("keyword.operator.optional", Span("p")),
    ("keyword.control.export", Span("k")),
    ("entity.name.type", Plain),
    ("variable.object.property", Span("nl")),
    ("punctuation.definition.typeparameters", Span("o")),
];

/// `Rouge::Lexers::Go`: every comment `c`, strings `s`, numbers `m`, `.` an operator, names via
/// the word rules.
const GO: &[(&str, Rule)] = &[
    ("comment", Span("c")),
    ("string", Span("s")),
    ("constant.other.placeholder", Inherit),
    ("constant.numeric", Span("m")),
    ("punctuation.accessor", Span("o")),
    ("keyword.other.package", Span("k")),
    ("keyword.other.import", Span("k")),
    ("variable", Plain),
    ("entity.name", Plain),
    ("storage.type", Plain),
];

/// `Rouge::Lexers::C` / `Cpp`: strings `s`, chars `sc`, whole preprocessor lines `cp`, floats
/// without exponent lexed as integers, names via the word rules.
const C: &[(&str, Rule)] = &[
    ("string.quoted.double", Span("s")),
    ("string.quoted.single", Span("sc")),
    ("constant.other.placeholder", Inherit),
    ("meta.preprocessor", Span("cp")),
    ("meta.preprocessor keyword", Span("cp")),
    ("meta.preprocessor entity", Span("cp")),
    ("meta.preprocessor constant", Span("cp")),
    ("constant.numeric.float", Span("mi")),
    ("punctuation.separator.decimal", Span("p")),
    ("constant.numeric.integer.hexadecimal storage.type.numeric", Span("mh")),
    ("punctuation.definition.numeric", Inherit),
    ("storage.type", Plain),
    ("storage.modifier", Plain),
    ("support.function", Plain),
    ("variable.function", Plain),
    ("entity.name.type", Plain),
    ("entity.name.namespace", Plain),
    ("punctuation.accessor.double-colon", Span("o")),
    ("punctuation.section.generic", Span("o")),
    ("punctuation.separator.initializer-list", Span("o")),
    ("punctuation.section.class", Span("nl")),
];

/// `Rouge::Lexers::Java`: all punctuation is `Operator`, strings `s`, chars `sc`, package paths
/// `nn`, annotations `nd`, names via the word rules.
const JAVA: &[(&str, Rule)] = &[
    ("punctuation", Span("o")),
    ("string.quoted.double", Span("s")),
    ("string.quoted.single", Span("sc")),
    ("meta.path support", Span("nn")),
    ("meta.path entity", Span("nn")),
    ("meta.path punctuation", Span("nn")),
    ("meta.annotation", Span("nd")),
    ("meta.annotation punctuation", Span("nd")),
    ("variable.annotation", Span("nd")),
    ("variable.function", Plain),
    ("storage.type", Plain),
    ("storage.modifier", Plain),
    ("support.class", Plain),
    ("entity.name.class", Span("nc")),
];

/// Per-Rouge-tag overlays ([`Variant::Improved`]): consulted before the shared table; they
/// encode how a particular Rouge lexer tokenizes where its classes differ systematically from
/// the shared mapping.
const OVERLAYS: &[(&str, &[(&str, Rule)])] = &[
    ("rust", RUST),
    ("python", PYTHON),
    ("javascript", JAVASCRIPT),
    ("typescript", JAVASCRIPT),
    ("go", GO),
    ("c", C),
    ("cpp", C),
    ("java", JAVA),
];

/// Class scheme variants.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Variant {
    /// Exactly `scope_map.py` (GL §8.4; the 50.5% / 75.8% baseline).
    Baseline,
    /// Baseline + [`IMPROVED_RULES`] + [`OVERLAYS`] + the Rouge word tables
    /// (`super::words`).
    Improved,
}

impl Variant {
    pub(crate) const ALL: [Variant; 2] = [Variant::Baseline, Variant::Improved];
}

/// The variant used by `highlight::gitlab`.
pub(crate) const DEFAULT_VARIANT: Variant = Variant::Improved;

/// A compiled class scheme.
pub(crate) struct Scheme {
    base: RuleSet<Rule>,
    overlays: HashMap<&'static str, RuleSet<Rule>>,
    /// Whether the Rouge word tables apply.
    pub(crate) words: bool,
}

/// Language-specific parts of a scheme for one block.
#[derive(Clone, Copy)]
pub(crate) struct ForLang<'s> {
    scheme: &'s Scheme,
    overlay: Option<&'s RuleSet<Rule>>,
}

impl Scheme {
    fn build(variant: Variant) -> Scheme {
        match variant {
            Variant::Baseline => Scheme {
                base: RuleSet::new(Precedence::AncestorFirst, BASELINE_RULES.iter().copied()),
                overlays: HashMap::new(),
                words: false,
            },
            Variant::Improved => {
                // Replacements keep the baseline position (tie order); new selectors append.
                let mut rules: Vec<(&'static str, Rule)> = BASELINE_RULES.to_vec();
                for &(selector, rule) in IMPROVED_RULES {
                    match rules.iter_mut().find(|(s, _)| *s == selector) {
                        Some(slot) => slot.1 = rule,
                        None => rules.push((selector, rule)),
                    }
                }
                Scheme {
                    base: RuleSet::new(Precedence::AncestorFirst, rules),
                    overlays: OVERLAYS
                        .iter()
                        .map(|&(tag, rules)| {
                            (tag, RuleSet::new(Precedence::AncestorFirst, rules.iter().copied()))
                        })
                        .collect(),
                    words: true,
                }
            }
        }
    }

    /// The scheme for `variant` (compiled once).
    pub(crate) fn get(variant: Variant) -> &'static Scheme {
        static SCHEMES: OnceLock<[Scheme; 2]> = OnceLock::new();
        let all = SCHEMES.get_or_init(|| Variant::ALL.map(Scheme::build));
        match variant {
            Variant::Baseline => &all[0],
            Variant::Improved => &all[1],
        }
    }

    /// The scheme specialised for Rouge lexer `tag`.
    pub(crate) fn for_lang(&self, tag: &str) -> ForLang<'_> {
        ForLang { scheme: self, overlay: self.overlays.get(tag) }
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

impl ForLang<'_> {
    /// The Rouge class for text under `stack` (outermost first); `None` = plain text.
    pub(crate) fn classify(&self, stack: &[Scope]) -> Option<&'static str> {
        for i in (0..stack.len()).rev() {
            let (scope, parents) = (stack[i], &stack[..i]);
            let rule = self
                .overlay
                .and_then(|o| o.lookup(scope, parents))
                .or_else(|| self.scheme.base.lookup(scope, parents));
            match rule {
                None | Some(Inherit) => continue,
                Some(Plain) => return None,
                Some(Span(class)) => return Some(class),
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stack(scopes: &str) -> Vec<Scope> {
        scopes.split(' ').map(|s| Scope::new(s).unwrap()).collect()
    }

    fn baseline(scopes: &str) -> Option<&'static str> {
        Scheme::get(Variant::Baseline).for_lang("x").classify(&stack(scopes))
    }

    #[test]
    fn all_selectors_compile() {
        assert_eq!(BASELINE_RULES.len(), 214);
        for v in Variant::ALL {
            assert!(Scheme::get(v).rejected().is_empty(), "{v:?}: {:?}", Scheme::get(v).rejected());
        }
    }

    #[test]
    fn baseline_examples() {
        assert_eq!(baseline("source.rust comment.line.double-slash.rust"), Some("c1"));
        assert_eq!(
            baseline("source.rust comment.line.documentation.rust punctuation.definition.comment.rust"),
            Some("cd"),
            "INHERIT is transparent"
        );
        assert_eq!(baseline("source.rust meta.function.rust storage.type.function.rust"), Some("kd"));
        assert_eq!(baseline("source.rust"), None);
        assert_eq!(baseline("source.rust meta.block.rust"), None);
        assert_eq!(
            baseline("source.json meta.mapping.key.json string.quoted.double.json"),
            Some("nl"),
            "ancestor rule beats the plain string rule"
        );
        assert_eq!(
            baseline("source.yaml meta.mapping.key.yaml string.unquoted.plain.out.yaml"),
            Some("na")
        );
        assert_eq!(baseline("source.json string.quoted.double.json"), Some("s2"));
        assert_eq!(baseline("text.html.basic meta.tag entity.name.tag.html"), Some("nt"));
    }
}
