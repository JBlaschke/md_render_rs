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

/// A table of `(selector, class)` rules.
type Table = &'static [(&'static str, Class)];

/// A table of text-conditioned `(selector, condition, class)` rules.
type TextTable = &'static [(&'static str, TextIs, Class)];

/// The text-conditioned alternatives of one selector, in table order (the first whose condition
/// holds decides).
pub(crate) type TextRules = [(TextIs, Class)];

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

/// The language-independent extra rules proposed in GH §7.6, layered on top of the map for
/// every language ([`Variant::Extended`]). Measured against github.com they lose to the
/// per-language overlays (they apply tree-sitter conventions to TextMate languages and vice
/// versa), so the default variant does not use them.
const EXTENDED: &[(&str, Class)] = &[
    ("entity.name.function", Some("pl-en")),
    ("support.function", Some("pl-en")),
    ("variable.function", Some("pl-en")),
    ("meta.function-call entity", Some("pl-en")),
    ("entity.name.type", Some("pl-smi")),
    ("entity.name.class", Some("pl-smi")),
    ("entity.name.struct", Some("pl-smi")),
    ("entity.name.enum", Some("pl-smi")),
    ("support.type", Some("pl-smi")),
    ("entity.other.attribute-name", Some("pl-e")),
    ("variable.parameter", Some("pl-v")),
    ("variable.other.member", Some("pl-c1")),
    ("meta.property", Some("pl-c1")),
    ("keyword.operator", Some("pl-k")),
    ("punctuation.separator", Some("pl-kos")),
    ("punctuation.terminator", Some("pl-kos")),
    ("punctuation.section", Some("pl-kos")),
    ("punctuation.section.embedded", Some("pl-pse")),
    ("constant.numeric", Some("pl-c1")),
    ("constant.language", Some("pl-c1")),
    ("constant.character.escape", Some("pl-cce")),
    ("meta.embedded", Some("pl-s1")),
    ("invalid.deprecated", Some("pl-bu")),
];

/// GitHub's Markdown grammar (`text.md`, wooorm/markdown-tm-language) against Sublime's
/// Markdown: list items, emphasis and fenced code are not colored as a whole, only their
/// markers; link/image punctuation is `pl-s`, URLs `pl-corl`; table pipes `pl-ml`; fenced code
/// is embedded without a wrapper class.
const MARKDOWN: &[(&str, Class)] = &[
    ("markup.list", None),
    ("punctuation.definition.list_item", Some("pl-v")),
    ("punctuation.separator.table-cell", Some("pl-ml")),
    ("punctuation.definition.blockquote", Some("pl-ent")),
    ("markup.bold", None),
    ("punctuation.definition.bold", Some("pl-s")),
    ("markup.italic", None),
    ("punctuation.definition.italic", Some("pl-s")),
    ("punctuation.definition.strikethrough", Some("pl-s")),
    ("punctuation.definition.link", Some("pl-s")),
    ("punctuation.definition.image", Some("pl-s")),
    ("punctuation.definition.metadata", Some("pl-s")),
    ("punctuation.definition.constant", Some("pl-s")),
    ("markup.underline.link", Some("pl-corl")),
    ("meta.link.inline.description", None),
    ("constant.other.reference.link", Some("pl-en")),
    (
        "meta.link.reference.def.footnote entity.name.reference.link",
        Some("pl-s"),
    ),
    ("string.other.link.description.title", Some("pl-s")),
    ("string.other.link punctuation.definition.string", None),
    ("meta.link.reference.literal.footnote-id", Some("pl-en")),
    ("constant.character.escape", Some("pl-c1")),
    ("meta.separator.thematic-break", None),
    ("punctuation.definition.thematic-break", Some("pl-s")),
    ("markup.raw.code-fence", None),
    ("punctuation.definition.raw", Some("pl-s")),
    ("constant.other.language-name", Some("pl-en")),
    ("source", None),
    ("meta.tag punctuation.definition.string", None),
];

/// JSON (TextMate on github.com): object keys are `pl-ent` as a whole, quotes included.
const JSON: &[(&str, Class)] = &[
    ("meta.mapping.key string", Some("pl-ent")),
    ("meta.mapping.key punctuation.definition.string", None),
];

/// YAML: keys `pl-ent` (the enclosing plain-scalar string is absorbed, see [`ABSORB`]),
/// anchors, aliases and block-scalar indicators are strings.
const YAML: &[(&str, Class)] = &[
    ("meta.property", Some("pl-s")),
    ("keyword.control.property", None),
    ("entity.name.other.anchor", None),
    ("keyword.control.flow.alias", Some("pl-s")),
    ("variable.other.alias", Some("pl-s")),
    ("keyword.control.flow.block-scalar", Some("pl-s")),
];

/// TOML: keys are `pl-smi`.
const TOML: &[(&str, Class)] = &[("entity.name.tag", Some("pl-smi"))];

/// Shell (atom/language-shellscript on github.com): command names, options and assignments are
/// plain; `$var`/`${…}` are `pl-smi`; `$(…)` is a string with `pl-pds` delimiters.
const SHELL: &[(&str, Class)] = &[
    ("variable.function", None),
    ("variable.parameter.option", None),
    ("string.unquoted", None),
    ("meta.group.expansion.parameter", Some("pl-smi")),
    (
        "meta.group.expansion.parameter variable.other.readwrite",
        None,
    ),
    ("variable.other.readwrite.assignment", None),
    ("keyword.operator.assignment", None),
    (
        "meta.group.expansion keyword.operator.assignment",
        Some("pl-k"),
    ),
    (
        "constant.numeric.integer.decimal.file-descriptor",
        Some("pl-k"),
    ),
    ("support.function.double-brace", None),
    ("meta.group.expansion.command", Some("pl-s")),
    (
        "meta.group.expansion.command punctuation.definition.variable",
        Some("pl-pds"),
    ),
    (
        "meta.group.expansion.command punctuation.section.parens",
        Some("pl-pds"),
    ),
];

/// Shared approximation of github.com's tree-sitter highlighting (GH §7.7: comments and strings
/// without nested marker spans, punctuation `pl-kos`, operators `pl-c1`, calls `pl-en`,
/// variables and parameters `pl-s1`, properties `pl-c1`) on Sublime scopes. Per-language
/// tables come first and override it.
const TREE_SITTER: &[(&str, Class)] = &[
    ("punctuation.definition.comment", None),
    ("punctuation.definition.string", None),
    ("punctuation", Some("pl-kos")),
    ("keyword.operator", Some("pl-c1")),
    ("variable.function", Some("pl-en")),
    ("support.function", Some("pl-en")),
    ("variable.parameter", Some("pl-s1")),
    ("variable.other.member", Some("pl-c1")),
    ("variable.other.property", Some("pl-c1")),
    ("constant.character.escape", Some("pl-cce")),
];

/// Python (tree-sitter on github.com).
const PYTHON: &[(&str, Class)] = &[
    // Python's tree-sitter queries capture no punctuation except interpolation braces.
    ("punctuation", None),
    ("punctuation.section.interpolation", Some("pl-kos")),
    ("meta.generic-name", Some("pl-s1")),
    ("meta.import-name", Some("pl-s1")),
    ("punctuation.accessor + meta.generic-name", Some("pl-c1")),
    ("punctuation.accessor + variable.function", Some("pl-c1")),
    ("variable.function meta.generic-name", None),
    ("variable.function support.function", None),
    ("entity.name meta.generic-name", None),
    ("meta.string.interpolated", Some("pl-s")),
    ("meta.string.interpolated string", None),
    ("meta.string.interpolated storage.type.string", None),
    ("meta.interpolation string.quoted", Some("pl-s")),
    ("source.regexp constant", None),
    ("source.regexp keyword.operator", None),
    ("variable.language", Some("pl-s1")),
    ("support.type", Some("pl-smi")),
    ("support.variable.magic", Some("pl-s1")),
    ("support.function.magic", None),
    (
        "meta.function.parameters.annotation meta.generic-name",
        Some("pl-smi"),
    ),
    ("punctuation.separator.annotation.return", Some("pl-c1")),
    ("storage.type keyword.declaration", None),
    ("storage.type.string", Some("pl-s")),
    ("comment.block.documentation", Some("pl-s")),
    ("meta.interpolation", Some("pl-s1")),
    ("source.python.embedded", None),
    ("storage.modifier.conversion", None),
    ("meta.annotation", Some("pl-en")),
    ("variable.annotation", None),
    // `@dataclass(frozen=True)`: the decorator is a call.
    (
        "variable.annotation.function meta.generic-name",
        Some("pl-en"),
    ),
    // `ValueError(…)`: a call, not a type.
    ("variable.function support.type", None),
    ("entity.name.class", Some("pl-v")),
    ("entity.other.inherited-class", Some("pl-v")),
    // `"in" @operator`, in `for` statements and comprehensions alike; `@` is not an operator
    // capture (it is also the decorator marker).
    ("keyword.control.loop.for.in", Some("pl-c1")),
    ("keyword.operator.matrix", None),
    // tree-sitter-python does not look into `%s` placeholders or format specs.
    ("constant.other.placeholder", None),
    ("constant.other.format-spec", None),
    ("source.regexp", None),
];

/// JavaScript / TypeScript / TSX (tree-sitter on github.com).
const JAVASCRIPT: &[(&str, Class)] = &[
    ("variable.other.readwrite", Some("pl-s1")),
    ("variable.other.constant", Some("pl-s1")),
    ("variable.language", Some("pl-smi")),
    ("support.type.object", Some("pl-smi")),
    ("meta.property.object", Some("pl-c1")),
    ("entity.name.function variable.other.readwrite", None),
    ("entity.name.function variable.other.constant", None),
    ("entity.name.class", Some("pl-v")),
    ("entity.other.inherited-class", Some("pl-v")),
    ("storage.type.function.arrow", Some("pl-c1")),
    ("keyword.operator.ternary", None),
    // tree-sitter-javascript captures no `...` (spread and rest alike).
    ("keyword.operator.spread", None),
    ("keyword.operator.rest", None),
    ("meta.string", Some("pl-s")),
    ("meta.string string", None),
    ("string.regexp", Some("pl-pds")),
    ("string.regexp punctuation.definition.string", Some("pl-c1")),
    ("string.regexp keyword.operator", Some("pl-c1")),
    ("string.regexp keyword.other", None),
    ("meta.interpolation", Some("pl-s1")),
    ("punctuation.section.interpolation", Some("pl-kos")),
    ("source.js.embedded", None),
    ("source.ts.embedded", None),
    ("source.tsx.embedded", None),
];

/// TypeScript / TSX on top of [`JAVASCRIPT`] (two-face's TypeScript grammar uses the vscode
/// scope names).
const TYPESCRIPT: &[(&str, Class)] = &[
    ("meta.brace", Some("pl-kos")),
    ("keyword.operator.type.annotation", None),
    ("keyword.operator.optional", None),
    ("keyword.operator.expression", Some("pl-k")),
    ("support.type.primitive", Some("pl-smi")),
    ("entity.name.type", Some("pl-smi")),
    ("entity.name.type.enum", Some("pl-v")),
    ("variable.object.property", Some("pl-c1")),
    ("variable.other.enummember", Some("pl-c1")),
    ("variable.other.object", Some("pl-s1")),
    ("support.variable.dom", Some("pl-smi")),
    ("punctuation.definition.typeparameters", Some("pl-c1")),
    ("meta.template.expression", Some("pl-s1")),
    ("punctuation.definition.template-expression", Some("pl-kos")),
    ("meta.embedded.line", None),
];

/// JSX in TSX (and in JavaScript containing JSX, which is tokenized with the TSX grammar): tag
/// names `pl-s1` (components `pl-v`, [`JSX_BY_TEXT`]), `<` and `>` `pl-c1` (tree-sitter captures
/// them as operators), `</` and `/>` `pl-kos`, attribute names `pl-c1`, text and character
/// references unclassified.
const TSX: &[(&str, Class)] = &[
    ("meta.tag entity.name.tag", Some("pl-s1")),
    ("meta.tag support.class.component", None),
    ("meta.tag punctuation.definition.tag", Some("pl-c1")),
    ("meta.tag constant.character.entity", None),
    ("entity.other.attribute-name", Some("pl-c1")),
    ("variable.other.constant entity.name.function", None),
];

/// Rust (tree-sitter on github.com): types `pl-smi`, constructors `pl-v`, macros and calls
/// `pl-en`, attributes `pl-c1` as a whole, lifetimes `'` `pl-c1` + name `pl-ent`
/// ([`RUST_WORDS`]); `=`/`=>`/`->` unclassified.
const RUST: &[(&str, Class)] = &[
    ("storage.type", Some("pl-smi")),
    ("storage.type.function", Some("pl-k")),
    ("storage.type.struct", Some("pl-k")),
    ("storage.type.enum", Some("pl-k")),
    ("storage.type.trait", Some("pl-k")),
    ("storage.type.impl", Some("pl-k")),
    ("storage.type.type", Some("pl-k")),
    ("storage.type.module", Some("pl-k")),
    ("storage.type.numeric", Some("pl-c1")),
    ("storage.type.string", None),
    ("storage.modifier.lifetime", None),
    ("support.type", Some("pl-smi")),
    ("support.macro", Some("pl-en")),
    ("variable.language", Some("pl-smi")),
    ("entity.name.struct", Some("pl-smi")),
    ("entity.name.impl", Some("pl-smi")),
    ("entity.name.type", Some("pl-smi")),
    ("entity.name.enum", Some("pl-smi")),
    ("entity.name.trait", Some("pl-smi")),
    ("entity.name.label", Some("pl-c1")),
    ("entity.name.macro", None),
    ("meta.macro support.function", Some("pl-k")),
    ("constant.other.placeholder", None),
    ("keyword.operator.assignment", None),
    ("meta.annotation", Some("pl-c1")),
    ("meta.annotation.parameters", None),
    ("variable.annotation", None),
    ("punctuation.definition.annotation", None),
    ("meta.function.return-type punctuation.separator", None),
];

/// C (tree-sitter on github.com): no punctuation classes, types `pl-smi` (`typedef`, `struct`,
/// `union`, `enum` are keywords, [`BY_TEXT`]), `sizeof` a keyword, number suffixes part of the
/// number, escapes and format placeholders unclassified inside strings, macro bodies
/// (tree-sitter's opaque `preproc_arg`) unclassified.
const C: &[(&str, Class)] = &[
    ("punctuation", None),
    ("storage.type", Some("pl-smi")),
    ("support.type", Some("pl-smi")),
    ("entity.name.type", Some("pl-smi")),
    ("keyword.operator.word", Some("pl-k")),
    ("constant.numeric storage.type.numeric", None),
    ("constant.other.placeholder", None),
    ("constant.character.escape", None),
    ("meta.preprocessor.macro constant.numeric", None),
    ("meta.preprocessor.macro keyword.operator", None),
];

/// Go (tree-sitter on github.com): no punctuation classes, types `pl-smi`, identifiers `pl-s1`,
/// selector calls and fields `pl-c1`.
const GO: &[(&str, Class)] = &[
    ("punctuation", None),
    ("storage.type", Some("pl-smi")),
    ("storage.type.keyword", Some("pl-k")),
    ("storage.type support", None),
    ("entity.name.type", Some("pl-smi")),
    ("variable.other", Some("pl-s1")),
    ("variable.declaration", Some("pl-s1")),
    ("variable.other.member", Some("pl-c1")),
    ("variable.function", Some("pl-c1")),
    ("variable.function support.function", None),
    ("constant.other.placeholder", None),
    ("entity.name.function", Some("pl-s1")),
    (
        "meta.function.declaration entity.name.function",
        Some("pl-c1"),
    ),
    ("keyword.other.package + variable.other", None),
];

/// CSS (tree-sitter on github.com): property names, selector names and pseudo-classes
/// `pl-c1`, units `pl-smi`, functions `pl-en`, colors `pl-pds`, custom properties `pl-s1`,
/// keyword values unclassified.
const CSS: &[(&str, Class)] = &[
    ("punctuation", None),
    ("punctuation.separator.key-value", Some("pl-kos")),
    (
        "entity.other.attribute-name.id punctuation.definition.entity",
        Some("pl-kos"),
    ),
    (
        "entity.other.pseudo-class punctuation.definition.entity",
        Some("pl-kos"),
    ),
    ("meta.property-name", None),
    ("support.type.property-name", Some("pl-c1")),
    ("keyword.other.unit", Some("pl-smi")),
    ("support.constant.property-value", None),
    ("support.constant.font-name", None),
    ("entity.other.attribute-name", Some("pl-c1")),
    ("entity.other.pseudo-class", Some("pl-c1")),
    ("entity.other.pseudo-element", Some("pl-c1")),
    ("punctuation.definition.entity", None),
    ("punctuation.separator.combinator", Some("pl-c1")),
    ("constant.other.color", Some("pl-pds")),
    (
        "constant.other.color punctuation.definition.constant",
        Some("pl-kos"),
    ),
    ("support.type.custom-property", Some("pl-s1")),
    ("support.type.custom-property punctuation", None),
    ("support.type.custom-property.name", None),
    ("meta.group string.unquoted", None),
];

/// HTML (tree-sitter on github.com): tag brackets `pl-kos`, attribute names `pl-c1`, the
/// doctype `pl-c1` as a whole, entities unclassified; embedded CSS/JS without wrapper classes.
const HTML: &[(&str, Class)] = &[
    ("punctuation", None),
    ("punctuation.definition.tag", Some("pl-kos")),
    ("entity.other.attribute-name", Some("pl-c1")),
    ("meta.tag.sgml.doctype", Some("pl-c1")),
    ("meta.tag.sgml.doctype keyword", None),
    (
        "meta.tag.sgml.doctype punctuation.definition.tag.begin",
        None,
    ),
    ("constant.character.entity", None),
    ("source", None),
];

/// Makefile (TextMate): assignments and plain words unclassified, `$(VAR)` and `$(func …)`
/// strings (the name inside `pl-smi`, [`MAKEFILE_WORDS`]), recipes are shell without a wrapper
/// class and without command classes (GitHub's grammar does not embed shell), `@`
/// unclassified; special targets (`.PHONY`, ...) `pl-c1` ([`BY_TEXT`]). Sublime's "expected a
/// tab" marker on space-indented lines is not an error on github.com.
const MAKEFILE: &[(&str, Class)] = &[
    ("keyword.operator.assignment", None),
    ("string.unquoted", None),
    ("source", None),
    ("variable.parameter", Some("pl-s")),
    ("meta.function-call.makefile", Some("pl-s")),
    ("meta.function-call.arguments.makefile", Some("pl-s")),
    // Sublime closes the call scope before its `)`.
    (
        "meta.function-call.arguments + keyword.other.block.end",
        Some("pl-s"),
    ),
    ("keyword.other.block", None),
    ("constant.language", None),
    ("source.shell support.function", None),
    ("invalid.illegal.inconsistent", None),
];

/// C++ (TextMate on github.com): operators, calls and parameters unclassified, member calls
/// `pl-c1`, access specifiers (with their colon) `pl-k`.
const CPP: &[(&str, Class)] = &[
    ("keyword.operator", None),
    ("variable.function", None),
    ("variable.function.member", Some("pl-c1")),
    ("variable.parameter", None),
    ("variable.other.readwrite.member", None),
    ("punctuation.section.class", Some("pl-k")),
];

/// Dockerfile (TextMate): strings without delimiter spans; stage names unclassified.
const DOCKERFILE: &[(&str, Class)] = &[
    ("punctuation.definition.string", None),
    ("variable.stage-name", None),
    ("entity.name.enum.tag-digest", None),
];

/// SQL (TextMate): `*` is a keyword.
const SQL: &[(&str, Class)] = &[("variable.language.star", Some("pl-k"))];

/// INI (TextMate): `[section]` is `pl-en` as a whole, values unclassified.
const INI: &[(&str, Class)] = &[
    ("meta.tag.section", Some("pl-en")),
    ("entity.section", None),
    ("punctuation.definition.section", None),
    ("string.name.value", None),
];

/// Kotlin (TextMate): template expressions `pl-e`, local names unclassified.
const KOTLIN: &[(&str, Class)] = &[
    ("entity.name.variable", None),
    ("keyword.operator.dot", None),
    ("variable.parameter.template", Some("pl-e")),
    ("storage.type.buildin", Some("pl-c1")),
];

/// GraphQL (TextMate): fields and variables `pl-v`, type references `pl-c1`.
const GRAPHQL: &[(&str, Class)] = &[
    ("entity.name.field", Some("pl-v")),
    ("punctuation.definition.variable", Some("pl-v")),
    ("variable.other", Some("pl-v")),
    ("variable.type", Some("pl-c1")),
    ("variable.type support.type", None),
];

/// Nix (tree-sitter on github.com).
const NIX: &[(&str, Class)] = &[
    ("keyword.operator", Some("pl-kos")),
    ("keyword.operator.nix", None),
    ("variable.parameter", Some("pl-s1")),
    ("entity.other.attribute-name", Some("pl-c1")),
    ("string.unquoted.spath", Some("pl-pds")),
    ("markup.italic", None),
    ("punctuation.section.embedded", Some("pl-kos")),
];

/// Regular expressions (tree-sitter on github.com).
const REGEXP: &[(&str, Class)] = &[
    ("keyword.control.group", Some("pl-kos")),
    ("keyword.control.anchors", Some("pl-cce")),
    ("keyword.control.set", Some("pl-kos")),
    ("keyword.control.character-class", Some("pl-cce")),
    ("keyword.operator.quantifier", Some("pl-c1")),
    ("keyword.operator.alternation", Some("pl-c1")),
    ("keyword.other.named-capture-group", Some("pl-kos")),
    ("punctuation.definition.capture-group-name", None),
    ("entity.name.capture-group", Some("pl-c1")),
    ("constant.other.assertion", Some("pl-kos")),
    ("meta.literal", Some("pl-s")),
];

/// SCSS (TextMate): `$variables` `pl-v`, `&` `pl-ent`, mixin names unclassified.
const SCSS: &[(&str, Class)] = &[
    ("variable.other", Some("pl-v")),
    ("variable.other punctuation.definition.variable", None),
    ("keyword.operator.ampersand", Some("pl-ent")),
    ("variable.function", None),
];

/// Perl (TextMate).
const PERL: &[(&str, Class)] = &[
    ("keyword.operator.assignment", None),
    ("storage.type keyword.declaration", None),
    ("punctuation.separator.key-value", Some("pl-k")),
];

/// Lua (TextMate): table keys `pl-smi`, their `=` a keyword.
const LUA: &[(&str, Class)] = &[
    ("meta.mapping punctuation.separator.key-value", Some("pl-k")),
    ("meta.mapping.key string.unquoted.key", Some("pl-smi")),
];

/// LaTeX (TextMate): `\begin`/`\end` one span, inline math a string.
const LATEX: &[(&str, Class)] = &[
    ("support.function keyword.control", None),
    ("support.function punctuation.definition.backslash", None),
    ("meta.environment.math", Some("pl-s")),
    ("meta.environment.math variable", None),
    ("meta.environment.math keyword.operator", None),
];

/// XML (TextMate): CDATA sections are strings with `pl-pds` delimiters.
const XML: &[(&str, Class)] = &[
    ("meta.tag.sgml.cdata", Some("pl-s")),
    (
        "meta.tag.sgml.cdata punctuation.definition.tag",
        Some("pl-pds"),
    ),
    ("meta.tag.sgml.cdata keyword", Some("pl-pds")),
];

/// PHP (tree-sitter on github.com): no punctuation or operator classes, `$` of variables
/// `pl-c1` inside `pl-s1` (`$this` too), `<?php` `pl-ent`, namespace segments `pl-v`
/// ([`PHP_WORDS`]), declared and extended class names unclassified, instantiated classes
/// `pl-v`.
const PHP: &[(&str, Class)] = &[
    ("source", None),
    ("punctuation", None),
    ("keyword.operator", None),
    ("punctuation.section.embedded", Some("pl-ent")),
    ("punctuation.definition.variable", Some("pl-c1")),
    ("variable.other", Some("pl-s1")),
    ("variable.language", Some("pl-s1")),
    ("storage.type", Some("pl-smi")),
    ("storage.type.function", Some("pl-k")),
    ("storage.type.class", Some("pl-k")),
    ("storage.type.nullable", None),
    ("storage.modifier", Some("pl-k")),
    ("entity.name.namespace", None),
    ("support.other.namespace", Some("pl-v")),
    ("entity.name.class", None),
    ("entity.other.inherited-class", None),
    ("support.class", Some("pl-smi")),
    ("keyword.other.new + support.class", Some("pl-v")),
    ("entity.name.function support.function", None),
    ("meta.string-contents", Some("pl-s")),
    ("constant.character.escape", None),
];

/// C# (tree-sitter on github.com).
const CSHARP: &[(&str, Class)] = &[
    ("punctuation.definition.generic", Some("pl-c1")),
    ("punctuation.separator.type", Some("pl-c1")),
    ("meta.path", Some("pl-s1")),
    ("entity.name.namespace", Some("pl-v")),
    ("entity.name.class", Some("pl-smi")),
    ("entity.other.inherited-class", Some("pl-smi")),
    ("variable.annotation", Some("pl-c1")),
    ("storage.type", Some("pl-smi")),
    ("storage.type.function", Some("pl-c1")),
    ("storage.type.function.accessor", Some("pl-k")),
    ("storage.type.namespace", Some("pl-k")),
    ("storage.type.class", Some("pl-k")),
    ("storage.type.struct", Some("pl-k")),
    ("storage.type.interface", Some("pl-k")),
    ("storage.type.enum", Some("pl-k")),
    ("keyword.operator.new", Some("pl-k")),
    ("support.type", Some("pl-smi")),
    ("variable.other", Some("pl-s1")),
    ("variable.other.member", Some("pl-s1")),
    ("meta.string.interpolated", Some("pl-s")),
    ("meta.string.interpolated string", None),
    ("source.cs", None),
];

/// Elixir (tree-sitter on github.com): modules `pl-v`, atoms and keyword keys `pl-pds`.
const ELIXIR: &[(&str, Class)] = &[
    ("entity.name.class", Some("pl-v")),
    ("constant.other.keywords", Some("pl-pds")),
    ("constant.other.symbol", Some("pl-pds")),
    ("punctuation.definition.constant", None),
];

/// Java (tree-sitter on github.com): no punctuation or operator classes, types `pl-smi`,
/// package path segments `pl-s1`, annotations `pl-c1`.
const JAVA: &[(&str, Class)] = &[
    ("punctuation", None),
    ("keyword.operator", None),
    ("entity.name.namespace", Some("pl-s1")),
    ("support.type.package", Some("pl-s1")),
    ("support.class", Some("pl-smi")),
    ("support.class.import", Some("pl-s1")),
    ("meta.annotation", Some("pl-c1")),
    ("variable.annotation", None),
    ("storage.type", Some("pl-smi")),
    ("storage.modifier.array", None),
    ("entity.name.class", Some("pl-smi")),
    ("entity.other.inherited-class", Some("pl-smi")),
    ("constant.numeric storage.type.numeric", None),
];

/// Ruby (tree-sitter on github.com): classes and modules `pl-v`, symbols `pl-pds`, instance and
/// class variables `pl-c1`, `require`/`attr_*` `pl-en`, operators unclassified.
const RUBY: &[(&str, Class)] = &[
    ("keyword.operator", None),
    ("storage.type keyword.declaration", None),
    ("entity.name.namespace", Some("pl-v")),
    ("entity.name.class", Some("pl-v")),
    ("entity.other.inherited-class", Some("pl-v")),
    ("punctuation.separator.inheritance", None),
    ("keyword.control.import", Some("pl-en")),
    ("keyword.other.special-method", Some("pl-en")),
    ("constant.other.symbol", Some("pl-pds")),
    ("constant.other.symbol punctuation", None),
    ("variable.other.readwrite.instance", Some("pl-c1")),
    ("variable.other.readwrite.class", Some("pl-c1")),
    ("variable.other.readwrite.global", None),
    (
        "variable.other.readwrite punctuation.definition.variable",
        None,
    ),
    ("variable.language", Some("pl-smi")),
    ("meta.interpolation", Some("pl-s1")),
    ("source.ruby.embedded", None),
    ("string.regexp keyword.other", None),
    (
        "string.quoted.other.literal punctuation.definition.string",
        Some("pl-kos"),
    ),
    ("meta.block.parameters punctuation", None),
];

/// Swift (tree-sitter on github.com).
const SWIFT: &[(&str, Class)] = &[
    ("support.type", Some("pl-smi")),
    ("keyword.operator.function-result", Some("pl-c1")),
];

/// A condition on the first text of a scope (tree-sitter queries test token text, TextMate
/// scopes cannot: GH §7.7).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TextIs {
    /// Starts with an ASCII uppercase letter (tree-sitter's `^[A-Z]` constructor/type rules).
    Capitalized,
    /// Is exactly one of these words.
    OneOf(&'static [&'static str]),
    /// Any text (a fallback after more specific alternatives of the same selector).
    Any,
    /// Starts with an ASCII letter or `_` (an identifier, for [`WORDS`]).
    Identifier,
    /// Starts with an ASCII lowercase letter or `_`.
    Lowercase,
}

impl TextIs {
    pub(crate) fn matches(self, text: &str) -> bool {
        match self {
            TextIs::Capitalized => text.starts_with(|c: char| c.is_ascii_uppercase()),
            TextIs::OneOf(words) => words.contains(&text),
            TextIs::Any => true,
            TextIs::Identifier => text.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_'),
            TextIs::Lowercase => text.starts_with(|c: char| c.is_ascii_lowercase() || c == '_'),
        }
    }
}

use TextIs::{Any, Capitalized, Identifier, Lowercase, OneOf};

/// A condition on the code around a word ([`WORDS`]): tree-sitter rules depend on the syntax
/// node a name is in (`call_expression function:`, `scoped_identifier path:`); the token that
/// follows or precedes the name is the lexical trace of that node.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Near {
    /// No condition.
    Anywhere,
    /// The code right after the word starts with one of these.
    Before(&'static [&'static str]),
    /// The code right before the word ends with one of these.
    After(&'static [&'static str]),
    /// The word's line, without its indentation, starts with one of these.
    InLine(&'static [&'static str]),
}

impl Near {
    /// Whether the condition holds for the word at `start..end` of `code`.
    pub(crate) fn holds(self, code: &str, start: usize, end: usize) -> bool {
        match self {
            Near::Anywhere => true,
            Near::Before(next) => code
                .get(end..)
                .is_some_and(|rest| next.iter().any(|n| rest.starts_with(n))),
            Near::After(prev) => code
                .get(..start)
                .is_some_and(|head| prev.iter().any(|p| head.ends_with(p))),
            Near::InLine(prefixes) => code.get(..start).is_some_and(|head| {
                let line = head.rsplit('\n').next().unwrap_or(head).trim_start();
                prefixes.iter().any(|p| line.starts_with(p))
            }),
        }
    }
}

use Near::{After, Anywhere, Before, InLine};

/// A table of word rules: `(selector, word condition, context condition, class)`.
type WordTable = &'static [(&'static str, TextIs, Near, Class)];

/// The word rules of one selector, in table order (the first that holds decides, also when its
/// class is `None`).
pub(crate) type WordRules = [(TextIs, Near, Class)];

/// Text-conditioned classes per Linguist `tm_scope` ([`Variant::Hybrid`]): when a scope whose
/// class is not `None` matches the selector and its first text satisfies the condition, the
/// class given here (`None`: no span) is used instead. Several rules with the same selector are
/// alternatives, tried in table order.
const BY_TEXT: &[(&str, TextTable)] = &[
    (
        "source.python",
        &[
            ("meta.generic-name", Capitalized, Some("pl-v")),
            (
                "meta.function.parameters.annotation meta.generic-name",
                Capitalized,
                Some("pl-smi"),
            ),
            ("variable.function", Capitalized, Some("pl-en")),
            // `Optional[...]`: a subscripted name is not a `type` node itself, so the
            // constructor rule applies.
            (
                "meta.function.parameters.annotation meta.item-access meta.generic-name",
                Capitalized,
                Some("pl-v"),
            ),
        ],
    ),
    ("source.js", JS_BY_TEXT),
    ("source.ts", JS_BY_TEXT),
    ("source.tsx", JS_BY_TEXT),
    ("source.tsx", JSX_BY_TEXT),
    ("source.rust", RUST_BY_TEXT),
    (
        "text.html.php",
        &[
            // Keywords the (pre-PHP 7.4/8.0) Sublime grammar scopes as functions or types.
            (
                "support.function.construct",
                OneOf(&["echo", "print"]),
                Some("pl-k"),
            ),
            ("variable.function", OneOf(&["fn"]), Some("pl-k")),
            (
                "support.class",
                OneOf(&["public", "protected", "private", "readonly"]),
                Some("pl-k"),
            ),
            // `declare(strict_types=1)` directives.
            (
                "constant.other",
                OneOf(&["strict_types", "ticks", "encoding"]),
                None,
            ),
        ],
    ),
    // Kotlin: a parameter's default value `=` is plain, its type `:` a keyword.
    (
        "source.kotlin",
        &[("keyword.operator.declaration", OneOf(&["="]), None)],
    ),
    (
        "source.makefile",
        &[(
            "entity.name.function",
            OneOf(&[
                ".PHONY",
                ".SUFFIXES",
                ".DEFAULT",
                ".PRECIOUS",
                ".INTERMEDIATE",
                ".SECONDARY",
                ".SECONDEXPANSION",
                ".DELETE_ON_ERROR",
                ".IGNORE",
                ".LOW_RESOLUTION_TIME",
                ".SILENT",
                ".EXPORT_ALL_VARIABLES",
                ".NOTPARALLEL",
                ".ONESHELL",
                ".POSIX",
            ]),
            Some("pl-c1"),
        )],
    ),
    (
        "source.c",
        &[(
            "storage.type",
            OneOf(&["typedef", "struct", "union", "enum"]),
            Some("pl-k"),
        )],
    ),
    (
        "source.sql",
        &[(
            "constant.language",
            OneOf(&["NULL", "null", "Null"]),
            Some("pl-k"),
        )],
    ),
    (
        "source.java",
        &[(
            "storage.type",
            OneOf(&["class", "interface", "enum", "record"]),
            Some("pl-k"),
        )],
    ),
];

/// tree-sitter-javascript: `((identifier) @constructor (#match? @constructor "^[A-Z]"))`.
const JS_BY_TEXT: TextTable = &[
    ("entity.name.function", Capitalized, Some("pl-v")),
    ("variable.other.constant", Capitalized, Some("pl-v")),
    ("variable.other.readwrite", Capitalized, Some("pl-v")),
];

/// tree-sitter-javascript's JSX rules: capitalized tag names are constructors, `</` and `/>`
/// are brackets (`<` and `>` are operators, as everywhere).
const JSX_BY_TEXT: TextTable = &[
    ("meta.tag entity.name.tag", Capitalized, Some("pl-v")),
    (
        "meta.tag punctuation.definition.tag",
        OneOf(&["</", "/>"]),
        Some("pl-kos"),
    ),
];

/// Sublime's Rust scopes keywords like `let` and primitive types alike (`storage.type`), and
/// constructors like `Some` like std types (`support.type`).
const RUST_BY_TEXT: TextTable = &[
    (
        "storage.type",
        OneOf(&[
            "let",
            "const",
            "static",
            "fn",
            "struct",
            "enum",
            "trait",
            "impl",
            "type",
            "mod",
            "union",
            "macro_rules!",
            "extern",
            "crate",
            "dyn",
        ]),
        Some("pl-k"),
    ),
    (
        "support.type",
        OneOf(&["Some", "None", "Ok", "Err"]),
        Some("pl-v"),
    ),
    // `(self) @variable.builtin`, also as a parameter.
    ("variable.parameter", OneOf(&["self"]), Some("pl-smi")),
    // tree-sitter-rust captures only `*`, `&` and `'` as operators; `as` and `in` are keywords.
    ("keyword.operator", OneOf(&["as", "in"]), Some("pl-k")),
    ("keyword.operator", OneOf(&["*", "&"]), Some("pl-c1")),
    ("keyword.operator", Any, None),
];

/// Word rules per Linguist `tm_scope` ([`Variant::Hybrid`]) for text that no scope classifies:
/// tree-sitter captures names the Sublime grammars leave unscoped (`dyn`, derive names, path
/// calls). The text is split into words (identifier and number runs, single punctuation
/// characters); a word gets the class of the first rule that holds, looking from the innermost
/// scope with word rules outwards, but never past a scope that has a class of its own (words in
/// strings and comments are left alone).
const WORDS: &[(&str, WordTable)] = &[
    ("source.rust", RUST_WORDS),
    ("source.makefile", MAKEFILE_WORDS),
    ("text.html.php", PHP_WORDS),
    ("source.kotlin", KOTLIN_WORDS),
];

/// tree-sitter-rust's `highlights.scm`, for the names Sublime's Rust leaves unscoped.
const RUST_WORDS: WordTable = &[
    // `(lifetime "'" (identifier) @label)`, `"'" @operator` (the lifetime scope has no class).
    (
        "storage.modifier.lifetime",
        OneOf(&["'"]),
        Anywhere,
        Some("pl-c1"),
    ),
    (
        "storage.modifier.lifetime",
        Identifier,
        Anywhere,
        Some("pl-ent"),
    ),
    // `#[derive(Debug, Clone)]`: identifiers `^[A-Z]` are constructors.
    ("meta.annotation", Capitalized, Anywhere, Some("pl-v")),
    // Type arguments are `type_identifier`s.
    ("meta.generic", Capitalized, Anywhere, Some("pl-smi")),
    // Enum variants are constructors, the fields of struct variants properties, the types of
    // their fields types.
    (
        "meta.enum meta.group",
        Capitalized,
        Anywhere,
        Some("pl-smi"),
    ),
    (
        "meta.enum meta.block",
        Capitalized,
        After(&[": "]),
        Some("pl-smi"),
    ),
    ("meta.enum meta.block", Capitalized, Anywhere, Some("pl-v")),
    (
        "meta.enum meta.block",
        Lowercase,
        Before(&[": "]),
        Some("pl-c1"),
    ),
    ("source.rust", OneOf(&["dyn"]), Anywhere, Some("pl-k")),
    // Unscoped lifetimes (`Formatter<'_>`).
    ("source.rust", OneOf(&["'"]), Anywhere, Some("pl-c1")),
    ("source.rust", Identifier, After(&["'"]), Some("pl-ent")),
    // "Assume that uppercase names in paths are types."
    ("source.rust", Capitalized, Before(&["::"]), Some("pl-smi")),
    // The names a `use` declaration imports are plain identifiers: constructors.
    (
        "source.rust",
        Capitalized,
        InLine(&["use ", "pub use ", "pub(crate) use "]),
        Some("pl-v"),
    ),
    // `call_expression function:` and `generic_function function:` (also `scoped_identifier
    // name:` and method calls).
    (
        "source.rust",
        Lowercase,
        Before(&["(", "::<"]),
        Some("pl-en"),
    ),
    ("source.rust", OneOf(&[","]), Anywhere, Some("pl-kos")),
];

/// GitHub's Makefile grammar: the name inside `$(…)` is a variable (`pl-smi` inside the `pl-s`
/// reference), the delimiters are not.
const MAKEFILE_WORDS: WordTable = &[
    (
        "variable.parameter.makefile keyword.other.block",
        Any,
        Anywhere,
        None,
    ),
    ("variable.parameter.makefile", Any, Anywhere, Some("pl-smi")),
];

/// tree-sitter-php: namespace names are constructors segment by segment, `$this` is
/// `variable.builtin`.
const PHP_WORDS: WordTable = &[
    ("entity.name.namespace", Identifier, Anywhere, Some("pl-v")),
    (
        "variable.language",
        OneOf(&["this"]),
        Anywhere,
        Some("pl-smi"),
    ),
];

/// GitHub's Kotlin grammar (as observed on github.com): calls of capitalized names
/// (constructors) are entity names, the standard-library builtins `println` and `listOf`
/// support functions.
const KOTLIN_WORDS: WordTable = &[
    ("source", Capitalized, Before(&["("]), Some("pl-en")),
    (
        "source",
        OneOf(&["println", "listOf"]),
        Before(&["("]),
        Some("pl-c1"),
    ),
];

/// Per-language overlays ([`Variant::Hybrid`]), keyed by Linguist `tm_scope`; each is a list of
/// rule tables, earlier tables winning ties. For languages github.com highlights with
/// tree-sitter (GH §7.7) they approximate the tree-sitter capture → class mapping; for TextMate
/// languages they bridge grammar differences.
const OVERLAYS: &[(&str, &[Table])] = &[
    ("text.md", &[MARKDOWN]),
    ("source.json", &[JSON]),
    ("source.json.comments", &[JSON]),
    ("source.yaml", &[YAML]),
    ("source.toml", &[TOML]),
    ("source.shell", &[SHELL]),
    ("text.shell-session", &[SHELL]),
    ("source.python", &[PYTHON, TREE_SITTER]),
    ("source.js", &[JAVASCRIPT, TREE_SITTER]),
    ("source.ts", &[TYPESCRIPT, JAVASCRIPT, TREE_SITTER]),
    ("source.tsx", &[TSX, TYPESCRIPT, JAVASCRIPT, TREE_SITTER]),
    ("source.rust", &[RUST, TREE_SITTER]),
    ("source.c", &[C, TREE_SITTER]),
    ("source.go", &[GO, TREE_SITTER]),
    ("source.css", &[CSS, TREE_SITTER]),
    ("text.html.basic", &[HTML, TREE_SITTER]),
    ("source.makefile", &[MAKEFILE, SHELL]),
    ("source.c++", &[CPP]),
    ("source.dockerfile", &[DOCKERFILE, SHELL]),
    ("source.sql", &[SQL]),
    ("source.ini", &[INI]),
    ("source.kotlin", &[KOTLIN]),
    ("source.graphql", &[GRAPHQL]),
    ("source.nix", &[NIX, TREE_SITTER]),
    ("source.regexp", &[REGEXP]),
    ("source.css.scss", &[SCSS]),
    ("source.perl", &[PERL]),
    ("source.lua", &[LUA]),
    ("text.tex.latex", &[LATEX]),
    ("text.xml", &[XML]),
    ("text.html.php", &[PHP, TREE_SITTER]),
    ("source.cs", &[CSHARP, TREE_SITTER]),
    ("source.elixir", &[ELIXIR, TREE_SITTER]),
    ("source.java", &[JAVA, TREE_SITTER]),
    ("source.swift", &[SWIFT, TREE_SITTER]),
    ("source.ruby", &[RUBY, TREE_SITTER]),
];

/// Selectors (per Linguist `tm_scope`, [`Variant::Hybrid`]) whose span replaces the span of the
/// enclosing scope when both start at the same position: Sublime wraps some tokens in an extra
/// scope that GitHub's grammar does not have (YAML keys are tags *inside* a plain scalar).
const ABSORB: &[(&str, &[&str])] = &[
    ("source.yaml", &["entity.name.tag"]),
    ("source.js", &["string.regexp"]),
    ("source.ts", &["string.regexp"]),
    ("source.tsx", &["string.regexp"]),
];

/// Class scheme variants (see the module docs). Grammar ports (shell-session, diff) are used
/// by all of them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Variant {
    /// The PrettyLights map alone (GH §7.5, "minimum viable").
    Map,
    /// Map + the language-independent GH §7.6 rules ([`EXTENDED`]).
    Extended,
    /// Map + the per-language overlays ([`OVERLAYS`], [`ABSORB`], [`BY_TEXT`]): TextMate
    /// grammar bridges for TextMate languages, tree-sitter approximations (GH §7.7) for the
    /// others.
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
    overlays: HashMap<&'static str, Overlay>,
}

/// The language-specific part of a scheme.
pub(crate) struct Overlay {
    rules: RuleSet<Class>,
    absorb: RuleSet<()>,
    by_text: RuleSet<Vec<(TextIs, Class)>>,
    words: Option<RuleSet<Vec<(TextIs, Near, Class)>>>,
}

/// Group `(selector, alternative…)` rows by selector, keeping table order: rules with the same
/// selector are alternatives of one rule.
fn alternatives<A>(rows: impl Iterator<Item = (&'static str, A)>) -> Vec<(&'static str, Vec<A>)> {
    let mut out: Vec<(&'static str, Vec<A>)> = Vec::new();
    for (selector, alternative) in rows {
        match out.iter_mut().find(|(s, _)| *s == selector) {
            Some((_, alternatives)) => alternatives.push(alternative),
            None => out.push((selector, vec![alternative])),
        }
    }
    out
}

impl Scheme {
    fn build(variant: Variant) -> Scheme {
        let map = PRETTYLIGHTS_MAP.iter().map(|&(s, c)| (s, Some(c)));
        let extended = EXTENDED.iter().copied();
        let base = match variant {
            Variant::Map | Variant::Hybrid => RuleSet::new(Precedence::LongestFirst, map),
            Variant::Extended => RuleSet::new(Precedence::LongestFirst, map.chain(extended)),
        };
        let mut overlays = HashMap::new();
        if variant == Variant::Hybrid {
            let langs = OVERLAYS
                .iter()
                .map(|(l, _)| *l)
                .chain(ABSORB.iter().map(|(l, _)| *l))
                .chain(BY_TEXT.iter().map(|(l, _)| *l))
                .chain(WORDS.iter().map(|(l, _)| *l));
            for lang in langs {
                let tables = OVERLAYS
                    .iter()
                    .filter(|(l, _)| *l == lang)
                    .flat_map(|(_, t)| *t);
                let rules = tables.flat_map(|t| t.iter().copied());
                let absorb = ABSORB
                    .iter()
                    .filter(|(l, _)| *l == lang)
                    .flat_map(|(_, s)| *s);
                let by_text = alternatives(
                    BY_TEXT
                        .iter()
                        .filter(|(l, _)| *l == lang)
                        .flat_map(|(_, t)| *t)
                        .map(|&(selector, cond, class)| (selector, (cond, class))),
                );
                let words = alternatives(
                    WORDS
                        .iter()
                        .filter(|(l, _)| *l == lang)
                        .flat_map(|(_, t)| *t)
                        .map(|&(selector, word, near, class)| (selector, (word, near, class))),
                );
                overlays.entry(lang).or_insert_with(|| Overlay {
                    rules: RuleSet::new(Precedence::LongestFirst, rules),
                    absorb: RuleSet::new(Precedence::LongestFirst, absorb.map(|s| (*s, ()))),
                    by_text: RuleSet::new(Precedence::LongestFirst, by_text),
                    words: (!words.is_empty())
                        .then(|| RuleSet::new(Precedence::LongestFirst, words)),
                });
            }
        }
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
    pub(crate) fn overlay(&self, tm_scope: &str) -> Option<&Overlay> {
        self.overlays.get(tm_scope)
    }

    /// Class for a pushed `scope` with `parents` further out and `previous` the innermost scope
    /// of the previous non-blank text: the overlay's verdict if it has a matching rule, else the
    /// base table's; plus a text-conditioned replacement class ([`BY_TEXT`]).
    pub(crate) fn classify<'o>(
        &self,
        overlay: Option<&'o Overlay>,
        scope: Scope,
        parents: &[Scope],
        previous: Option<Scope>,
    ) -> (Class, Option<&'o TextRules>) {
        let class = match overlay.and_then(|o| o.rules.lookup_after(scope, parents, previous)) {
            Some(class) => *class,
            None => self
                .base
                .lookup_after(scope, parents, previous)
                .copied()
                .flatten(),
        };
        let by_text = match class {
            Some(_) => overlay.and_then(|o| o.by_text.lookup_after(scope, parents, previous)),
            None => None,
        };
        (class, by_text.map(Vec::as_slice))
    }

    /// Whether a span for `scope` replaces its parent's span when both start together.
    pub(crate) fn absorbs(overlay: Option<&Overlay>, scope: Scope, parents: &[Scope]) -> bool {
        overlay.is_some_and(|o| o.absorb.lookup(scope, parents).is_some())
    }

    #[cfg(test)]
    pub(crate) fn rejected(&self) -> Vec<&'static str> {
        let mut out = self.base.rejected().to_vec();
        for o in self.overlays.values() {
            out.extend_from_slice(o.rules.rejected());
            out.extend_from_slice(o.absorb.rejected());
            out.extend_from_slice(o.by_text.rejected());
            if let Some(words) = &o.words {
                out.extend_from_slice(words.rejected());
            }
        }
        out
    }
}

impl Overlay {
    /// Whether the language has word rules ([`WORDS`]).
    pub(crate) fn has_words(&self) -> bool {
        self.words.is_some()
    }

    /// The word rules for text directly inside `scope` (with `parents` further out), if any.
    pub(crate) fn words(&self, scope: Scope, parents: &[Scope]) -> Option<&WordRules> {
        let rules = self.words.as_ref()?.lookup(scope, parents)?;
        Some(rules.as_slice())
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
            assert!(
                Scheme::get(v).rejected().is_empty(),
                "{v:?}: {:?}",
                Scheme::get(v).rejected()
            );
        }
        assert_eq!(PRETTYLIGHTS_MAP.len(), 75);
    }

    #[test]
    fn map_longest_prefix() {
        let m = Scheme::get(Variant::Map);
        let c = |x: &str| m.classify(None, s(x), &[], None).0;
        assert_eq!(c("comment.line.double-slash.rust"), Some("pl-c"));
        assert_eq!(c("entity.name.function.rust"), Some("pl-en"));
        assert_eq!(c("entity.name.tag.html"), Some("pl-ent"));
        assert_eq!(c("entity.other.attribute-name.html"), Some("pl-e"));
        assert_eq!(c("string.quoted.double.rust"), Some("pl-s"));
        assert_eq!(
            c("punctuation.definition.string.begin.rust"),
            Some("pl-pds")
        );
        assert_eq!(c("constant.character.escape.rust"), Some("pl-cce"));
        assert_eq!(c("storage.type.java"), Some("pl-smi"));
        assert_eq!(c("storage.type.function.rust"), Some("pl-k"));
        assert_eq!(c("meta.function.rust"), None);
        assert_eq!(c("punctuation.terminator.rust"), None);
    }

    #[test]
    fn word_conditions() {
        let code = "use a::B;\n  x::new(1)";
        let at = |w: &str| {
            let i = code.rfind(w).unwrap();
            (i, i + w.len())
        };
        let (s, e) = at("new");
        assert!(Before(&["("]).holds(code, s, e));
        assert!(!Before(&["::"]).holds(code, s, e));
        assert!(After(&["::"]).holds(code, s, e));
        assert!(InLine(&["x::"]).holds(code, s, e));
        assert!(!InLine(&["use "]).holds(code, s, e));
        let (s, e) = at("B");
        assert!(InLine(&["use "]).holds(code, s, e));
        assert!(Before(&[";\n"]).holds(code, s, e));
        assert!(Anywhere.holds(code, s, e));
        // At the edges of the code.
        assert!(!Before(&["("]).holds(code, code.len(), code.len()));
        assert!(!After(&["x"]).holds(code, 0, 3));
        assert!(Identifier.matches("_x") && !Identifier.matches("1x") && !Lowercase.matches("X"));
    }
}
