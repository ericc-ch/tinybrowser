//! Rendered-text collection for `innerText`/`outerText`.
//!
//! Approximation of the spec's rendered text collection steps
//! (<https://html.spec.whatwg.org/multipage/dom.html#rendered-text-collection-steps>)
//! without a style engine: `display`, `white-space`, `visibility`,
//! `text-transform`, `float`, and `position` come from the `style`
//! attribute only, defaulting per tag. Stylesheet rules, pseudo-elements,
//! soft line breaks, and flex/grid `order` are not visible, so those
//! subtests fail honestly.

use blitz_dom::{BaseDocument, NodeData};

use crate::js::world::{BlitzId, NodeId};
use super::world_for_node;
use rquickjs::{Ctx, Result};

/// The `white-space` modes that change collection.
#[derive(Clone, Copy, PartialEq, Eq)]
enum WhiteSpace {
    /// `normal`, `nowrap`: collapse runs, trim at breaks.
    Normal,
    /// `pre`, `pre-wrap`: preserve everything (`\r` still becomes `\n`).
    Pre,
    /// `pre-line`: collapse spaces, preserve newlines.
    PreLine,
}

/// One `visibility` value, inherited until overridden.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Visibility {
    Visible,
    Hidden,
}

/// Block-level treatment of one element for break purposes.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Level {
    /// Inline content, no breaks of its own.
    Inline,
    /// Atomic inline (`img`, `inline-block`, ...): children trimmed, and
    /// surrounding spaces survive on both sides.
    Atomic,
    /// One line break around content.
    Block,
    /// A blank line around content (`p` only).
    Paragraph,
    /// Skipped entirely.
    Hidden,
    /// Children hoisted as if unwrapped (`display:contents`).
    Contents,
}

/// Effective `text-transform` with Turkish casing resolved.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Transform {
    None,
    Upper(bool),
    Lower(bool),
    Capitalize,
}

/// Reads one CSS property from a `style` attribute value: the last
/// declaration wins, `!important` is stripped, names are case-insensitive.
fn style_property<'a>(style: &'a str, property: &str) -> Option<&'a str> {
    let mut found = None;
    for declaration in style.split(';') {
        let (name, value) = match declaration.split_once(':') {
            Some(pair) => pair,
            None => continue,
        };
        if name.trim().eq_ignore_ascii_case(property) {
            let value = value.trim();
            found = Some(
                value
                    .strip_suffix("!important")
                    .map(str::trim)
                    .unwrap_or(value),
            );
        }
    }
    found
}

/// Whether `node` is an HTML element with one of these local names.
fn is_html_tag(base: &BaseDocument, node: BlitzId, tags: &[&str]) -> bool {
    base.get_node(node).is_some_and(|tree| {
        tree.data.downcast_element().is_some_and(|element| {
            element.name.ns == crate::js::world::html_namespace()
                && tags.contains(&element.name.local.as_ref())
        })
    })
}

/// The `white-space` mode of an element: the `style` attribute wins, then
/// `pre`-family tags, then inheritance.
fn white_space_of(base: &BaseDocument, node: BlitzId, inherited: WhiteSpace) -> WhiteSpace {
    if let Some(style) = crate::js::world::attr(base, node, "style")
        && let Some(value) = style_property(style, "white-space")
    {
        return match value.to_ascii_lowercase().as_str() {
            "pre" | "pre-wrap" | "break-spaces" => WhiteSpace::Pre,
            "pre-line" => WhiteSpace::PreLine,
            _ => WhiteSpace::Normal,
        };
    }
    if is_html_tag(base, node, &["pre", "listing", "xmp", "plaintext"]) {
        return WhiteSpace::Pre;
    }
    inherited
}

/// The display treatment of an element: the `style` attribute wins, with
/// `float` (non-`none`) and absolute positioning blockifying, then tag
/// defaults. Stylesheet classes are invisible, so class-driven layouts
/// keep their tag default and fail honestly.
fn level_of(base: &BaseDocument, node: BlitzId) -> Level {
    let Some(tree) = base.get_node(node) else {
        return Level::Hidden;
    };
    let Some(element) = tree.data.downcast_element() else {
        return Level::Inline;
    };
    let html = element.name.ns == crate::js::world::html_namespace();
    let local = element.name.local.as_ref();
    // `template` and `noscript` never render, regardless of styling: their
    // content is not elements (noscript) or lives in a separate fragment.
    if html && (local == "template" || local == "noscript") {
        return Level::Hidden;
    }
    let style = crate::js::world::attr(base, node, "style");
    let display = style.and_then(|style| style_property(style, "display"));
    // `float` and absolute positioning blockify.
    let blockified = style.is_some_and(|style| {
        style_property(style, "float")
            .is_some_and(|value| !value.is_empty() && !value.eq_ignore_ascii_case("none"))
            || style_property(style, "position").is_some_and(|value| {
                value.eq_ignore_ascii_case("absolute") || value.eq_ignore_ascii_case("fixed")
            })
    });
    if let Some(display) = display {
        let display = display.to_ascii_lowercase();
        let display = display.as_str();
        if display == "none" {
            return Level::Hidden;
        }
        if display == "contents" {
            return Level::Contents;
        }
        // `p` keeps its blank lines under any rendering display, even
        // `inline-block` (margins, not display, drive them).
        if html && local == "p" {
            return Level::Paragraph;
        }
        if blockified
            && !matches!(
                display,
                "block"
                    | "flex"
                    | "grid"
                    | "table"
                    | "list-item"
                    | "inline-block"
                    | "inline-flex"
                    | "inline-grid"
                    | "inline-table"
            )
        {
            return Level::Block;
        }
        return match display {
            "block" | "flex" | "grid" | "table" | "list-item" => Level::Block,
            "inline-block" | "inline-flex" | "inline-grid" | "inline-table" => Level::Atomic,
            "table-row" | "table-cell" | "table-caption" | "table-column"
            | "table-column-group" | "table-row-group" | "table-header-group"
            | "table-footer-group" => Level::Hidden,
            _ => Level::Inline,
        };
    }
    if blockified {
        return Level::Block;
    }
    // `p` without an explicit display (handled above) is blank-lined.
    if html && local == "p" {
        return Level::Paragraph;
    }
    if !html {
        // Foreign content without styling: known never-rendered SVG
        // containers are skipped, everything else is inline. SVG local
        // names are case-sensitive (`clipPath`, not `clippath`).
        return if matches!(
            local,
            "defs" | "desc" | "metadata" | "stop" | "title" | "mask" | "clipPath" | "pattern"
            | "symbol" | "use" | "script" | "style"
        ) {
            Level::Hidden
        } else {
            Level::Inline
        };
    }
    match local {
        // Never rendered (without an overriding `style` attribute, which
        // returned above for rendering displays; `source`/`track` join
        // `script`/`style` here; `template`/`noscript` returned earlier).
        "head" | "meta" | "link" | "base" | "title" | "script" | "style" | "source"
        | "track" | "frame" | "frameset" | "noframes" | "noembed" | "datalist" => {
            Level::Hidden
        }
        // `rp` never renders (its parens are dropped); an explicit rendering
        // display above still wins.
        "rp" => Level::Hidden,
        // Transparent containers.
        "span" | "a" | "b" | "i" | "em" | "strong" | "code" | "tt" | "u" | "s" | "small"
        | "big" | "cite" | "q" | "dfn" | "abbr" | "kbd" | "samp" | "var" | "sub" | "sup"
        | "mark" | "ruby" | "rt" | "bdi" | "bdo" | "font" | "label" | "button" | "fieldset"
        | "legend" | "output" | "nobr" | "wbr" | "slot" | "picture" | "li" | "dt" | "dd"
        | "colgroup" | "col" | "thead" | "tbody" | "tfoot" | "tr" | "td" | "th" | "caption"
        | "marquee" | "center" | "isindex" => Level::Inline,
        // Block containers.
        "address" | "article" | "aside" | "blockquote" | "details" | "dialog" | "div"
        | "dl" | "figcaption" | "figure" | "footer" | "form" | "h1" | "h2" | "h3" | "h4"
        | "h5" | "h6" | "header" | "hgroup" | "main" | "nav" | "ol" | "section" | "summary"
        | "ul" | "dir" | "menu" | "pre" | "listing" | "xmp" | "plaintext" | "table" => {
            Level::Block
        }
        // Atomic inline: replaced elements and controls with ignored content.
        "img" | "input" | "textarea" | "iframe" | "audio" | "video" | "canvas" | "object"
        | "embed" | "param" | "map" | "area" | "applet" => Level::Atomic,
        "select" | "option" | "optgroup" | "p" => Level::Paragraph,
        // Unknown and custom elements are inline by default.
        _ => Level::Inline,
    }
}

/// Whether the element's subtree contributes no rendered text: replaced
/// and content-ignored elements. `source`/`track`/`script`/`style` render
/// when given an explicit rendering display instead (handled by their
/// level), so they are not listed here.
fn subtree_skipped(base: &BaseDocument, node: BlitzId) -> bool {
    is_html_tag(
        base,
        node,
        &[
            "input", "textarea", "iframe", "audio", "video", "canvas", "object", "embed",
            "img", "param", "map", "area",
        ],
    )
}

/// The effective `visibility` of an element: an explicit `visible` or
/// hiding value wins, anything else (including invalid values) inherits.
fn visibility_of(base: &BaseDocument, node: BlitzId, inherited: Visibility) -> Visibility {
    if let Some(style) = crate::js::world::attr(base, node, "style")
        && let Some(value) = style_property(style, "visibility")
    {
        if value.eq_ignore_ascii_case("visible") {
            return Visibility::Visible;
        }
        if value.eq_ignore_ascii_case("hidden") || value.eq_ignore_ascii_case("collapse") {
            return Visibility::Hidden;
        }
    }
    inherited
}

/// The effective `text-transform` of an element: explicit style wins, else
/// inherited. Turkish casing resolves `lang` up the ancestor chain.
fn transform_of(base: &BaseDocument, node: BlitzId, inherited: Transform) -> Transform {
    if let Some(style) = crate::js::world::attr(base, node, "style")
        && let Some(value) = style_property(style, "text-transform")
    {
        let mut turkish = false;
        let mut cursor = Some(node);
        while let Some(current) = cursor {
            if let Some(lang) = crate::js::world::attr(base, current, "lang") {
                let lang = lang.to_ascii_lowercase();
                turkish = lang == "tr" || lang.starts_with("tr-");
                break;
            }
            cursor = base.get_node(current).and_then(|tree| tree.parent);
        }
        return match value.to_ascii_lowercase().as_str() {
            "uppercase" => Transform::Upper(turkish),
            "lowercase" => Transform::Lower(turkish),
            "capitalize" => Transform::Capitalize,
            _ => Transform::None,
        };
    }
    inherited
}

/// Applies an effective transform to already-whitespace-processed text.
fn apply_transform(text: &str, transform: Transform) -> String {
    match transform {
        Transform::None => text.to_owned(),
        Transform::Upper(turkish) => {
            if turkish {
                text.chars()
                    .map(|char| match char {
                        'i' => 'İ',
                        'ı' => 'I',
                        _ => char,
                    })
                    .collect::<String>()
                    .to_uppercase()
            } else {
                text.to_uppercase()
            }
        }
        Transform::Lower(turkish) => {
            if turkish {
                text.chars()
                    .map(|char| match char {
                        'I' => 'ı',
                        'İ' => 'i',
                        _ => char,
                    })
                    .collect::<String>()
                    .to_lowercase()
            } else {
                text.to_lowercase()
            }
        }
        Transform::Capitalize => {
            let mut out = String::with_capacity(text.len());
            let mut word_start = true;
            for char in text.chars() {
                if char.is_alphabetic() {
                    if word_start {
                        out.extend(char.to_uppercase());
                    } else {
                        out.push(char);
                    }
                    word_start = false;
                } else {
                    out.push(char);
                    if matches!(char, ' ' | '\t' | '\n' | '\u{c}' | '\r' | '\u{a0}') {
                        word_start = true;
                    }
                }
            }
            out
        }
    }
}

/// Processes one text node under `white-space` `mode`: `\r\n` and lone `\r`
/// become `\n` in every mode, then spaces collapse unless preserved
/// (`&nbsp;` never collapses).
fn process_text(text: &str, mode: WhiteSpace) -> String {
    let mut normalized = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(char) = chars.next() {
        if char == '\r' {
            if chars.peek() == Some(&'\n') {
                chars.next();
            }
            normalized.push('\n');
        } else {
            normalized.push(char);
        }
    }
    if mode == WhiteSpace::Pre {
        return normalized;
    }
    let collapse_newlines = mode == WhiteSpace::Normal;
    let mut out = String::with_capacity(normalized.len());
    let mut pending_space = false;
    // After a preserved newline, collapsible spaces open no run: they are
    // invisible at the line start.
    let mut after_newline = false;
    for char in normalized.chars() {
        if char == '\n' && !collapse_newlines {
            pending_space = false;
            after_newline = true;
            out.push('\n');
        } else if matches!(char, ' ' | '\t' | '\u{c}' | '\r')
            || (collapse_newlines && char == '\n')
        {
            if after_newline {
                continue;
            }
            pending_space = true;
        } else {
            if pending_space {
                out.push(' ');
                pending_space = false;
            }
            after_newline = false;
            out.push(char);
        }
    }
    if pending_space {
        out.push(' ');
    }
    out
}

/// Collector state for one rendered-text run.
struct Collector<'a> {
    base: &'a BaseDocument,
    out: String,
    /// The buffer ends with a collapsible space ready to merge.
    pending_space: bool,
    /// The next space run emits even after trailing content (atomic inline
    /// on both sides keeps its spaces).
    force_space: bool,
    /// The last item collected was atomic content with nothing after it
    /// yet: a trailing space materializes at the very end.
    trailing_atomic: bool,
    /// Whether the last pushed text preserved spaces (`pre`).
    last_preserve: bool,
    /// Trailing boundary breaks issued since the last content: content
    /// newlines (e.g. inside `pre`) never count, so a following block adds
    /// rather than collapses.
    trailing_breaks: usize,
    /// Whether the trailing break (if any) came from a forced break (`br`).
    trailing_forced: bool,
}

impl<'a> Collector<'a> {
    /// Pushes mode-processed text, merging boundary spaces unless preserved.
    fn push_text(&mut self, text: &str, preserve: bool) {
        if text.is_empty() {
            return;
        }
        if preserve {
            self.pending_space = false;
            self.out.push_str(text);
            self.force_space = false;
            self.last_preserve = true;
            self.trailing_breaks = 0;
            self.trailing_forced = false;
            self.trailing_atomic = false;
            return;
        }
        self.last_preserve = false;
        // A space-only run merges without disturbing the break count, and
        // vanishes entirely against a preceding break: it is already
        // represented there.
        if text.trim_matches(' ').is_empty() {
            if !(self.out.is_empty() || self.out.ends_with('\n')) {
                self.pending_space = true;
            }
            return;
        }
        self.trailing_breaks = 0;
        self.trailing_forced = false;
        let mut text = text;
        if text.starts_with(' ') {
            if self.force_space || self.pending_space {
                // Atomic boundaries keep their space; an owed space
                // materializes once against the run.
                self.out.push(' ');
            } else if !(self.out.is_empty() || self.out.ends_with([' ', '\n', '\t']))
            {
                self.out.push(' ');
            }
            self.pending_space = false;
            self.force_space = false;
            text = text.trim_start_matches(' ');
            if text.is_empty() {
                self.pending_space = true;
                return;
            }
        } else if self.pending_space {
            // An owed space materializes before content.
            if !(self.out.is_empty() || self.out.ends_with('\n')) {
                self.out.push(' ');
            }
            self.pending_space = false;
            self.force_space = false;
        } else {
            self.force_space = false;
        }
        if text.ends_with(' ') {
            self.pending_space = true;
            text = text.trim_end_matches(' ');
        } else {
            self.pending_space = false;
        }
        self.out.push_str(text);
        self.trailing_atomic = false;
    }

    /// Pushes atomic content: trimmed; a pending space flushes first and a
    /// following space run survives.
    fn push_atomic(&mut self, text: &str) {
        if self.pending_space {
            self.out.push(' ');
            self.pending_space = false;
        }
        let trimmed = text.trim_matches([' ', '\n', '\t']);
        self.out.push_str(trimmed);
        self.force_space = true;
        self.last_preserve = false;
        self.trailing_breaks = 0;
        self.trailing_forced = false;
        self.trailing_atomic = true;
    }

    /// A forced break (`br`): always exactly one newline.
    fn push_forced_break(&mut self) {
        self.pending_space = false;
        self.force_space = false;
        if !self.last_preserve {
            while self.out.ends_with(' ') {
                self.out.pop();
            }
        }
        self.last_preserve = false;
        self.out.push('\n');
        self.trailing_breaks += 1;
        self.trailing_forced = true;
        self.trailing_atomic = false;
    }

    /// Ensures `count` block-boundary newlines when the buffer has content,
    /// absorbing a pending space and trailing spaces (unless preserved).
    /// Only breaks issued here count toward collapsing; content newlines
    /// (e.g. inside `pre`) never satisfy a following block. A forced break
    /// survives (a following block still collapses against it via the
    /// count).
    fn ensure_breaks(&mut self, count: usize) {
        if self.out.is_empty() {
            self.pending_space = false;
            self.force_space = false;
            self.trailing_atomic = false;
            return;
        }
        self.pending_space = false;
        self.force_space = false;
        self.trailing_atomic = false;
        if !self.last_preserve {
            while self.out.ends_with(' ') {
                self.out.pop();
            }
        }
        self.last_preserve = false;
        for _ in self.trailing_breaks..count {
            self.out.push('\n');
        }
        if self.trailing_breaks < count {
            self.trailing_breaks = count;
        }
    }
}

/// Whether the element establishes a flex/grid container (children blockify).
fn is_flex(base: &BaseDocument, node: BlitzId) -> bool {
    crate::js::world::attr(base, node, "style").is_some_and(|style| {
        style_property(style, "display").is_some_and(|value| {
            value.eq_ignore_ascii_case("flex")
                || value.eq_ignore_ascii_case("grid")
                || value.eq_ignore_ascii_case("inline-flex")
                || value.eq_ignore_ascii_case("inline-grid")
        })
    })
}

/// Collects rendered text for `node`'s children.
#[allow(clippy::too_many_arguments)]
fn collect_children(
    collector: &mut Collector<'_>,
    node: BlitzId,
    mode: WhiteSpace,
    visibility: Visibility,
    transform: Transform,
    flex_parent: bool,
) {
    let Some(tree) = collector.base.get_node(node) else {
        return;
    };
    let children: Vec<BlitzId> = tree.children.to_vec();
    for child in children {
        collect_node(collector, child, mode, visibility, transform, flex_parent);
    }
}

/// Collects one node.
#[allow(clippy::too_many_arguments)]
fn collect_node(
    collector: &mut Collector<'_>,
    node: BlitzId,
    inherited_mode: WhiteSpace,
    inherited_visibility: Visibility,
    inherited_transform: Transform,
    flex_parent: bool,
) {
    let base = collector.base;
    let Some(tree) = base.get_node(node) else {
        return;
    };
    match &tree.data {
        NodeData::Text(text) => {
            if inherited_visibility == Visibility::Hidden {
                return;
            }
            let processed = process_text(&text.content, inherited_mode);
            let transformed = apply_transform(&processed, inherited_transform);
            collector.push_text(&transformed, inherited_mode == WhiteSpace::Pre);
        }
        NodeData::Element(element) => {
            let local = element.name.local.as_ref().to_owned();
            let html = element.name.ns == crate::js::world::html_namespace();
            let visibility = visibility_of(base, node, inherited_visibility);
            // `visibility:hidden` still lays out: recurse so explicitly
            // visible descendants show, but hide this level's own text.
            // (The text arm skips hidden text; breaks still apply.)
            if html && crate::js::world::attr(base, node, "hidden").is_some_and(|value| {
                !value.eq_ignore_ascii_case("until-found")
            }) {
                return;
            }
            if html && local == "br" {
                collector.push_forced_break();
                return;
            }
            let mode = white_space_of(base, node, inherited_mode);
            let transform = transform_of(base, node, inherited_transform);
            // A `tr` target (or nested row) collects its cells directly.
            if html && local == "tr" {
                collect_row(collector, node, mode, visibility, transform);
                return;
            }
            if html && local == "hr" {
                collector.ensure_breaks(1);
                collect_children(
                    collector,
                    node,
                    inherited_mode,
                    visibility,
                    inherited_transform,
                    false,
                );
                collector.ensure_breaks(1);
                return;
            }
            let mode = white_space_of(base, node, inherited_mode);
            let transform = transform_of(base, node, inherited_transform);
            let mut level = level_of(base, node);
            if flex_parent && level == Level::Inline {
                level = Level::Block;
            }
            match level {
                Level::Hidden => {}
                Level::Contents => {
                    collect_children(collector, node, mode, visibility, transform, false);
                }
                Level::Atomic => {
                    if subtree_skipped(base, node) {
                        collector.push_atomic("");
                        return;
                    }
                    let mut inner = Collector {
                        base,
                        out: String::new(),
                        pending_space: false,
                        force_space: false,
                        last_preserve: false,
                        trailing_breaks: 0,
                        trailing_forced: false,
                        trailing_atomic: false,
                    };
                    collect_children(&mut inner, node, mode, visibility, transform, false);
                    collector.push_atomic(&inner.out);
                }
                Level::Block | Level::Paragraph => {
                    if html && local == "table" {
                        collect_table(collector, node, mode, visibility, transform);
                        return;
                    }
                    if html && local == "select" {
                        collect_select(collector, node, mode, visibility, transform);
                        return;
                    }
                    if html && local == "details" {
                        collect_details(collector, node, mode, visibility, transform);
                        return;
                    }
                    let breaks = if level == Level::Paragraph { 2 } else { 1 };
                    // Inside `visibility:hidden` subtrees blocks recurse
                    // transparently (only explicitly visible descendants
                    // show, without their usual blank lines).
                    let transparent = visibility == Visibility::Hidden;
                    // `option`/`optgroup` force breaks even when empty.
                    if html && (local == "option" || local == "optgroup") {
                        // Nested `optgroup` in `optgroup` renders nothing.
                        if local == "optgroup" && is_optgroup_ancestor(base, node) {
                            if !transparent {
                                collector.ensure_breaks(1);
                            }
                            return;
                        }
                        if !transparent {
                            collector.ensure_breaks(1);
                        }
                        collect_children(
                            collector,
                            node,
                            mode,
                            visibility,
                            transform,
                            is_flex(base, node),
                        );
                        if !transparent {
                            collector.ensure_breaks(1);
                        }
                        return;
                    }
                    if !transparent {
                        collector.ensure_breaks(breaks);
                    }
                    // A display-overridden replaced element (e.g. block-level
                    // `img`) breaks without rendering its contents.
                    if !subtree_skipped(base, node) {
                        collect_children(
                            collector,
                            node,
                            mode,
                            visibility,
                            transform,
                            is_flex(base, node),
                        );
                    }
                    // A trailing atomic keeps its space instead of a break.
                    if !transparent {
                        if collector.trailing_atomic {
                            collector.trailing_atomic = false;
                        } else {
                            collector.ensure_breaks(breaks);
                        }
                    }
                }
                Level::Inline => {
                    if subtree_skipped(base, node) {
                        return;
                    }
                    collect_children(collector, node, mode, visibility, transform, false);
                }
            }
        }
        _ => {}
    }
}

/// Whether `node` (`optgroup`) sits inside another `optgroup`.
fn is_optgroup_ancestor(base: &BaseDocument, node: BlitzId) -> bool {
    let mut cursor = base.get_node(node).and_then(|tree| tree.parent);
    while let Some(parent) = cursor {
        if is_html_tag(base, parent, &["optgroup"]) {
            return true;
        }
        if is_html_tag(base, parent, &["select"]) {
            return false;
        }
        cursor = base.get_node(parent).and_then(|tree| tree.parent);
    }
    false
}

/// Whether the element itself opts out of rendering with `display:none`.
/// Unlike `visibility`, descendants cannot re-show through this.
fn display_none(base: &BaseDocument, node: BlitzId) -> bool {
    crate::js::world::attr(base, node, "style").is_some_and(|style| {
        style_property(style, "display").is_some_and(|value| value.eq_ignore_ascii_case("none"))
    })
}

/// Collects a `table` subtree: rows newline-separated, cells tab-separated
/// (including trailing empty cells), captions broken. Whitespace directly
/// under table structure is ignored.
#[allow(clippy::too_many_arguments)]
fn collect_table(
    collector: &mut Collector<'_>,
    node: BlitzId,
    mode: WhiteSpace,
    visibility: Visibility,
    transform: Transform,
) {
    collector.ensure_breaks(1);
    // Rows in tree order (`tfoot` is not reordered); captions are direct
    // children only. Row groups pass their `visibility` down so collapsed
    // sections hide their rows.
    let top_children: Vec<BlitzId> = collector
        .base
        .get_node(node)
        .map(|tree| tree.children.to_vec())
        .unwrap_or_default();
    let mut rows: Vec<(BlitzId, Visibility)> = Vec::new();
    let mut captions: Vec<BlitzId> = Vec::new();
    let mut stack: Vec<(BlitzId, Visibility)> = top_children
        .into_iter()
        .rev()
        .map(|child| (child, visibility))
        .collect();
    while let Some((current, inherited)) = stack.pop() {
        // `display:none` subtrees never render, so they contribute no rows.
        if display_none(collector.base, current) {
            continue;
        }
        let Some(tree) = collector.base.get_node(current) else {
            continue;
        };
        let kind = tree.data.downcast_element().and_then(|element| {
            (element.name.ns == crate::js::world::html_namespace())
                .then(|| element.name.local.as_ref().to_owned())
        });
        match kind.as_deref() {
            Some("tr") => {
                rows.push((current, inherited));
            }
            Some("td") | Some("th") => {}
            Some("table") => {}
            _ => {
                let item_visibility = visibility_of(collector.base, current, inherited);
                let children = tree.children.to_vec();
                for child in children.into_iter().rev() {
                    stack.push((child, item_visibility));
                }
            }
        }
    }
    for child in collector
        .base
        .get_node(node)
        .map(|tree| tree.children.to_vec())
        .unwrap_or_default()
    {
        let is_caption = collector.base.get_node(child).is_some_and(|tree| {
            tree.data.downcast_element().is_some_and(|element| {
                element.name.ns == crate::js::world::html_namespace()
                    && element.name.local.as_ref() == "caption"
            })
        });
        if is_caption {
            captions.push(child);
        }
    }
    let mut first = true;
    for (row, row_inherited) in rows {
        if !first {
            collector.ensure_breaks(1);
        }
        first = false;
        // Rows resolve their own box properties so `tbody`/`tr` styles
        // (e.g. `visibility:collapse`) apply to their cells.
        let row_visibility = visibility_of(collector.base, row, row_inherited);
        let row_mode = white_space_of(collector.base, row, mode);
        let row_transform = transform_of(collector.base, row, transform);
        collect_row(
            collector,
            row,
            row_mode,
            row_visibility,
            row_transform,
        );
    }
    for caption in captions {
        collector.ensure_breaks(1);
        let caption_visibility = visibility_of(collector.base, caption, visibility);
        let caption_mode = white_space_of(collector.base, caption, mode);
        let caption_transform = transform_of(collector.base, caption, transform);
        collect_children(
            collector,
            caption,
            caption_mode,
            caption_visibility,
            caption_transform,
            false,
        );
    }
    collector.ensure_breaks(1);
}

/// Collects one table row: cells tab-separated (including trailing empty
/// cells). An empty row still breaks: its own line survives the shared
/// separator.
#[allow(clippy::too_many_arguments)]
fn collect_row(
    collector: &mut Collector<'_>,
    row: BlitzId,
    mode: WhiteSpace,
    visibility: Visibility,
    transform: Transform,
) {
    let before = collector.out.len();
    let Some(tree) = collector.base.get_node(row) else {
        return;
    };
    let mut cell_first = true;
    for cell in tree.children.to_vec() {
        let is_cell = collector.base.get_node(cell).is_some_and(|tree| {
            tree.data.downcast_element().is_some_and(|element| {
                element.name.ns == crate::js::world::html_namespace()
                    && matches!(element.name.local.as_ref(), "td" | "th")
            })
        });
        if !is_cell {
            continue;
        }
        if !cell_first {
            collector.out.push('\t');
            collector.pending_space = false;
            collector.force_space = false;
            collector.trailing_breaks = 0;
            collector.trailing_atomic = false;
        }
        cell_first = false;
        let cell_visibility = visibility_of(collector.base, cell, visibility);
        let cell_mode = white_space_of(collector.base, cell, mode);
        let cell_transform = transform_of(collector.base, cell, transform);
        collect_children(
            collector,
            cell,
            cell_mode,
            cell_visibility,
            cell_transform,
            false,
        );
    }
    if collector.out.len() == before {
        collector.out.push('\n');
        collector.trailing_breaks += 1;
    }
}

/// Collects a `select` subtree: each `option`/`optgroup` on its own line,
/// direct text ignored.
#[allow(clippy::too_many_arguments)]
fn collect_select(
    collector: &mut Collector<'_>,
    node: BlitzId,
    mode: WhiteSpace,
    visibility: Visibility,
    transform: Transform,
) {
    let Some(tree) = collector.base.get_node(node) else {
        return;
    };
    for child in tree.children.to_vec() {
        // `display:none` options never render.
        if display_none(collector.base, child) {
            continue;
        }
        let kind = collector.base.get_node(child).and_then(|tree| {
            tree.data.downcast_element().and_then(|element| {
                (element.name.ns == crate::js::world::html_namespace())
                    .then(|| element.name.local.as_ref().to_owned())
            })
        });
        match kind.as_deref() {
            // Text directly under `select` is ignored.
            None => {}
            Some("option") | Some("optgroup") => {
                if kind.as_deref() == Some("optgroup")
                    && is_optgroup_ancestor(collector.base, child)
                {
                    continue;
                }
                collector.ensure_breaks(1);
                let item_visibility = visibility_of(collector.base, child, visibility);
                let item_mode = white_space_of(collector.base, child, mode);
                let item_transform = transform_of(collector.base, child, transform);
                collect_children(
                    collector,
                    child,
                    item_mode,
                    item_visibility,
                    item_transform,
                    false,
                );
                collector.ensure_breaks(1);
            }
            // Invalid elements inside `select` still render.
            _ => {
                collect_node(collector, child, mode, visibility, transform, false);
            }
        }
    }
}

/// Collects a `details` subtree: without `open`, only `summary` shows.
#[allow(clippy::too_many_arguments)]
fn collect_details(
    collector: &mut Collector<'_>,
    node: BlitzId,
    mode: WhiteSpace,
    visibility: Visibility,
    transform: Transform,
) {
    collector.ensure_breaks(1);
    if crate::js::world::attr(collector.base, node, "open").is_none() {
        let Some(tree) = collector.base.get_node(node) else {
            return;
        };
        for child in tree.children.to_vec() {
            if is_html_tag(collector.base, child, &["summary"]) {
                collect_node(collector, child, mode, visibility, transform, false);
            }
        }
        collector.ensure_breaks(1);
        return;
    }
    collect_children(collector, node, mode, visibility, transform, false);
    collector.ensure_breaks(1);
}

/// The rendered text of `id`'s subtree: `textContent` when `id` is not being
/// rendered, otherwise the collected text with block-boundary trailing
/// breaks trimmed (a single forced break survives).
pub(crate) fn rendered_text(ctx: &Ctx<'_>, id: NodeId) -> Result<String> {
    let world = world_for_node(ctx, id)?;
    let world = world.borrow();
    let Some(parsed) = world.document(id) else {
        return Ok(String::new());
    };
    let base = &parsed.document.base;
    let Some(tree) = base.get_node(id.node) else {
        return Ok(String::new());
    };
    let is_html_element = tree.data.downcast_element().is_some_and(|element| {
        element.name.ns == crate::js::world::html_namespace()
    });
    if !is_html_element {
        return Ok(String::new());
    }
    // Content-ignored elements and `noscript` render nothing themselves.
    // (Checked before the not-rendered path, which would otherwise answer
    // `textContent`.)
    if subtree_skipped(base, id.node) || is_html_tag(base, id.node, &["noscript"]) {
        return Ok(String::new());
    }
    if !being_rendered(base, id.node) {
        return Ok(text_content(base, id.node));
    }
    // `br` itself renders nothing.
    if is_html_tag(base, id.node, &["br"]) {
        return Ok(String::new());
    }
    let mut collector = Collector {
        base,
        out: String::new(),
        pending_space: false,
        force_space: false,
        last_preserve: false,
        trailing_breaks: 0,
        trailing_forced: false,
        trailing_atomic: false,
    };
    // The target's own properties seed inheritance, and its own element
    // dispatch applies (tables, selects, and details collect structurally
    // even at the top level): collect the target itself rather than only
    // its children. Leading breaks never arise; trailing ones trim below.
    let (top_mode, top_visibility, top_transform) = initial_styles(base, id.node);
    collect_node(
        &mut collector,
        id.node,
        top_mode,
        top_visibility,
        top_transform,
        is_flex(base, id.node),
    );
    let mut out = collector.out;
    // Leading block breaks never arise (ensures skip empty buffers), but a
    // leading forced break (`<br>` first) survives.
    if collector.trailing_forced {
        // A forced break survives at the end, collapsed to one.
        while out.ends_with("\n\n") {
            out.pop();
        }
    } else {
        // Block-boundary breaks at the end are dropped; content newlines
        // (e.g. inside `pre`) survive.
        for _ in 0..collector.trailing_breaks {
            if out.ends_with('\n') {
                out.pop();
            }
        }
    }
    // A trailing collapsible space vanishes at the end; a trailing atomic
    // (nothing after a replaced element) keeps one final space.
    collector.pending_space = false;
    collector.force_space = false;
    if collector.trailing_atomic && !out.is_empty() && !out.ends_with([' ', '\n', '\t']) {
        out.push(' ');
    }
    Ok(out)
}

/// Resolves the inherited `white-space`, `visibility`, and
/// `text-transform` at `node` by walking up to the root: the nearest
/// explicit style wins, `pre`-family tags imply `pre`, otherwise defaults.
fn initial_styles(base: &BaseDocument, node: BlitzId) -> (WhiteSpace, Visibility, Transform) {
    let mut mode = None;
    let mut visibility = None;
    let mut transform = None;
    let mut cursor = Some(node);
    while let Some(current) = cursor
        && (mode.is_none() || visibility.is_none() || transform.is_none())
    {
        if mode.is_none() {
            if let Some(style) = crate::js::world::attr(base, current, "style")
                && let Some(value) = style_property(style, "white-space")
            {
                mode = Some(match value.to_ascii_lowercase().as_str() {
                    "pre" | "pre-wrap" => WhiteSpace::Pre,
                    "pre-line" => WhiteSpace::PreLine,
                    _ => WhiteSpace::Normal,
                });
            } else if is_html_tag(base, current, &["pre", "listing", "xmp", "plaintext"]) {
                mode = Some(WhiteSpace::Pre);
            }
        }
        if visibility.is_none()
            && let Some(style) = crate::js::world::attr(base, current, "style")
            && let Some(value) = style_property(style, "visibility")
        {
            visibility = Some(if value.eq_ignore_ascii_case("visible") {
                Visibility::Visible
            } else {
                Visibility::Hidden
            });
        }
        if transform.is_none()
            && let Some(style) = crate::js::world::attr(base, current, "style")
            && let Some(value) = style_property(style, "text-transform")
        {
            let mut turkish = false;
            let mut lang_cursor = Some(current);
            while let Some(lang_node) = lang_cursor {
                if let Some(lang) = crate::js::world::attr(base, lang_node, "lang") {
                    let lang = lang.to_ascii_lowercase();
                    turkish = lang == "tr" || lang.starts_with("tr-");
                    break;
                }
                lang_cursor = base.get_node(lang_node).and_then(|tree| tree.parent);
            }
            transform = Some(match value.to_ascii_lowercase().as_str() {
                "uppercase" => Transform::Upper(turkish),
                "lowercase" => Transform::Lower(turkish),
                "capitalize" => Transform::Capitalize,
                _ => Transform::None,
            });
        }
        cursor = base.get_node(current).and_then(|tree| tree.parent);
    }
    (
        mode.unwrap_or(WhiteSpace::Normal),
        visibility.unwrap_or(Visibility::Visible),
        transform.unwrap_or(Transform::None),
    )
}

/// Whether the node is being rendered: no `display:none`, no `hidden`
/// (except `hidden=until-found`, which renders), and no never-rendered tag
/// on the path to the root.
fn being_rendered(base: &BaseDocument, node: BlitzId) -> bool {
    let mut cursor = Some(node);
    while let Some(current) = cursor {
        let Some(tree) = base.get_node(current) else {
            return false;
        };
        if let Some(element) = tree.data.downcast_element() {
            let html = element.name.ns == crate::js::world::html_namespace();
            let local = element.name.local.as_ref();
            if html
                && crate::js::world::attr(base, current, "hidden").is_some_and(|value| {
                    !value.eq_ignore_ascii_case("until-found")
                })
            {
                return false;
            }
            if let Some(style) = crate::js::world::attr(base, current, "style")
                && let Some(display) = style_property(style, "display")
                && display.eq_ignore_ascii_case("none")
            {
                return false;
            }
            if html
                && matches!(
                    local,
                    "template" | "noscript" | "head" | "meta" | "link" | "base" | "title"
                    | "script" | "style"
                )
            {
                return false;
            }
        }
        cursor = tree.parent;
    }
    true
}

/// Descendant text without any processing (the not-rendered path).
fn text_content(base: &BaseDocument, id: BlitzId) -> String {    let mut text = String::new();
    let mut stack = base
        .get_node(id)
        .map(|node| node.children.iter().rev().copied().collect::<Vec<_>>())
        .unwrap_or_default();
    while let Some(current) = stack.pop() {
        let Some(node) = base.get_node(current) else {
            continue;
        };
        match &node.data {
            NodeData::Text(data) => text.push_str(&data.content),
            NodeData::Element(_) | NodeData::Fragment { .. } | NodeData::AnonymousBlock(_) => {
                let mut children = node.children.to_vec();
                children.reverse();
                stack.extend(children);
            }
            _ => {}
        }
    }
    text
}
