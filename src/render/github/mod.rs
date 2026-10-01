//! GitHub flavor offline renderer (owner: render-github). Contract: docs/DESIGN.md §2 D2, §5;
//! .dev/research/github.md §9.

use super::{RenderOptions, Rendered};

/// Render GitHub-flavored output.
pub fn render(markdown: &str, opts: &RenderOptions) -> Rendered {
    super::placeholder_render(markdown, opts)
}
