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
type TextTable = &'static [(&'static str, TextIs, &'static str)];

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
    ("entity.name.class", Some("pl-v")),
    ("entity.other.inherited-class", Some("pl-v")),
    (
        "meta.expression.generator keyword.control.loop.for.in",
        Some("pl-c1"),
    ),
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

/// JSX in TSX: tag names `pl-s1`, tag brackets and attribute names `pl-c1`.
const TSX: &[(&str, Class)] = &[
    ("meta.tag entity.name.tag", Some("pl-s1")),
    ("meta.tag punctuation.definition.tag", Some("pl-c1")),
    ("entity.other.attribute-name", Some("pl-c1")),
    ("variable.other.constant entity.name.function", None),
];

/// Rust (tree-sitter on github.com): types `pl-smi`, constructors `pl-v`, macros and calls
/// `pl-en`, attributes `pl-c1` as a whole, lifetimes `pl-c1`; `=`/`=>`/`->` unclassified.
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
    ("storage.modifier.lifetime", Some("pl-c1")),
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

/// C (tree-sitter on github.com): no punctuation classes, types `pl-smi`, escapes and format
/// placeholders unclassified inside strings.
const C: &[(&str, Class)] = &[
    ("punctuation", None),
    ("storage.type", Some("pl-smi")),
    ("support.type", Some("pl-smi")),
    ("constant.other.placeholder", None),
    ("constant.character.escape", None),
    ("meta.preprocessor.macro constant.numeric", None),
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

/// Makefile (TextMate): assignments and plain words unclassified, `$(VAR)` a string, recipes
/// are shell without a wrapper class.
const MAKEFILE: &[(&str, Class)] = &[
    ("keyword.operator.assignment", None),
    ("string.unquoted", None),
    ("source", None),
    ("variable.parameter", Some("pl-s")),
    ("keyword.other.block", None),
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

/// PHP (tree-sitter on github.com): no punctuation classes, `$` of variables `pl-c1` inside
/// `pl-s1`, `<?php` `pl-ent`, namespaces `pl-v`.
const PHP: &[(&str, Class)] = &[
    ("source", None),
    ("punctuation", None),
    ("punctuation.section.embedded", Some("pl-ent")),
    ("punctuation.definition.variable", Some("pl-c1")),
    ("variable.other", Some("pl-s1")),
    ("storage.type", Some("pl-smi")),
    ("storage.type.function", Some("pl-k")),
    ("storage.type.class", Some("pl-k")),
    ("storage.modifier", Some("pl-k")),
    ("entity.name.namespace", Some("pl-v")),
    ("support.other.namespace", Some("pl-v")),
    ("support.class", Some("pl-smi")),
    ("meta.string-contents", Some("pl-s")),
    ("constant.character.escape", None),
    ("keyword.operator.string", None),
    ("keyword.operator.assignment", None),
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
}

impl TextIs {
    pub(crate) fn matches(self, text: &str) -> bool {
        match self {
            TextIs::Capitalized => text.starts_with(|c: char| c.is_ascii_uppercase()),
            TextIs::OneOf(words) => words.contains(&text),
        }
    }
}

use TextIs::{Capitalized, OneOf};

/// Text-conditioned classes per Linguist `tm_scope` ([`Variant::Hybrid`]): when a scope whose
/// class is not `None` matches the selector and its first text satisfies the condition, the
/// class given here is used instead.
const BY_TEXT: &[(&str, TextTable)] = &[
    (
        "source.python",
        &[
            ("meta.generic-name", Capitalized, "pl-v"),
            (
                "meta.function.parameters.annotation meta.generic-name",
                Capitalized,
                "pl-smi",
            ),
            ("variable.function", Capitalized, "pl-en"),
        ],
    ),
    ("source.js", JS_BY_TEXT),
    ("source.ts", JS_BY_TEXT),
    ("source.tsx", JS_BY_TEXT),
    ("source.rust", RUST_BY_TEXT),
    (
        "source.sql",
        &[(
            "constant.language",
            OneOf(&["NULL", "null", "Null"]),
            "pl-k",
        )],
    ),
    (
        "source.java",
        &[(
            "storage.type",
            OneOf(&["class", "interface", "enum", "record"]),
            "pl-k",
        )],
    ),
];

/// tree-sitter-javascript: `((identifier) @constructor (#match? @constructor "^[A-Z]"))`.
const JS_BY_TEXT: &[(&str, TextIs, &str)] = &[
    ("entity.name.function", Capitalized, "pl-v"),
    ("variable.other.constant", Capitalized, "pl-v"),
    ("variable.other.readwrite", Capitalized, "pl-v"),
];

/// Sublime's Rust scopes keywords like `let` and primitive types alike (`storage.type`), and
/// constructors like `Some` like std types (`support.type`).
const RUST_BY_TEXT: &[(&str, TextIs, &str)] = &[
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
        "pl-k",
    ),
    (
        "support.type",
        OneOf(&["Some", "None", "Ok", "Err"]),
        "pl-v",
    ),
    ("keyword.operator", OneOf(&["as"]), "pl-k"),
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
    by_text: RuleSet<(TextIs, &'static str)>,
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
                .chain(BY_TEXT.iter().map(|(l, _)| *l));
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
                let by_text = BY_TEXT
                    .iter()
                    .filter(|(l, _)| *l == lang)
                    .flat_map(|(_, t)| *t);
                overlays.entry(lang).or_insert_with(|| Overlay {
                    rules: RuleSet::new(Precedence::LongestFirst, rules),
                    absorb: RuleSet::new(Precedence::LongestFirst, absorb.map(|s| (*s, ()))),
                    by_text: RuleSet::new(
                        Precedence::LongestFirst,
                        by_text.map(|&(s, cond, class)| (s, (cond, class))),
                    ),
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
    pub(crate) fn classify(
        &self,
        overlay: Option<&Overlay>,
        scope: Scope,
        parents: &[Scope],
        previous: Option<Scope>,
    ) -> (Class, Option<(TextIs, &'static str)>) {
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
        (class, by_text.copied())
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
}
