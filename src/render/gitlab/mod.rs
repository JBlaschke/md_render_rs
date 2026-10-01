//! GitLab flavor offline renderer (owner: render-gitlab). Contract: docs/DESIGN.md §2 D3, §5;
//! .dev/research/gitlab.md §2-§6.

use super::{RenderOptions, Rendered};

/// Render GitLab-flavored output.
pub fn render(markdown: &str, opts: &RenderOptions) -> Rendered {
    super::placeholder_render(markdown, opts)
}
