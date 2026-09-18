//! The render pipeline: cascade styles over the DOM, build the box tree, lay
//! it out, and paint one image.

use std::collections::HashMap;

use dom::{Dom, NodeId};

use crate::font::Fonts;
use crate::geometry::Rect;
use crate::layout::{LayoutBox, PaintItem};
use crate::paint::Painter;
use crate::style::{
    Decl, Declared, Origin, Rule, Style, UA_STYLESHEET, parse_inline_style, parse_stylesheet,
};
use crate::text::FontStyle;
use crate::tree;
use crate::{RenderError, RenderOptions, RgbaImage};

/// One declaration collected for a specific element, with its cascade key.
struct Cascaded {
    /// Sort key, ascending: later entries win.
    key: DeclKey,
    /// The declaration.
    declared: Declared,
}

/// Cascade order within one element
/// (<https://drafts.csswg.org/css-cascade-5/#cascade-sort>, origins reduced
/// to UA and author).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct DeclKey {
    /// Important declarations sort after normal ones within an origin.
    important: u8,
    /// Origin rank: for normal declarations the UA loses; for important
    /// declarations the UA wins (the accessibility override).
    origin: u8,
    /// Selector specificity; `style` attributes rank above selectors.
    specificity: u32,
    /// Rule order across sheets.
    order: u32,
    /// Declaration order inside its rule.
    index: u32,
}

/// Renders `dom` into one image.
///
/// # Errors
///
/// See [`RenderError`].
pub(crate) fn render(
    dom: &Dom,
    stylesheets: &[String],
    options: &RenderOptions,
) -> Result<RgbaImage, RenderError> {
    if !valid_dimension(options.width)
        || !valid_dimension(options.height)
        || !valid_dimension(options.scale)
    {
        return Err(RenderError::InvalidViewport);
    }
    let viewport_width = options.width;
    let viewport_height = options.height;

    // 1. Parse the UA sheet and every author sheet in order.
    let mut order = 0u32;
    let mut rules = parse_stylesheet(
        dom,
        UA_STYLESHEET,
        Origin::UserAgent,
        &mut order,
        viewport_width,
    );
    for sheet in stylesheets {
        rules.extend(parse_stylesheet(
            dom,
            sheet,
            Origin::Author,
            &mut order,
            viewport_width,
        ));
    }

    // 2. Compute one style per element in tree order.
    let fonts = Fonts::load()?;
    let styles = compute_styles(dom, &rules);

    // 3. Build and lay out the box tree through Taffy.
    let root = tree::build(dom, &styles);
    let layout = crate::boxes::layout_root(&root, &fonts, viewport_width, viewport_height);

    // 4. Paint once, then encode from the caller.
    let width = crate::device_pixels((viewport_width * options.scale).round().max(1.0));
    let height = crate::device_pixels((viewport_height * options.scale).round().max(1.0));
    let mut painter = Painter::new(width, height, fonts)?;
    paint(&mut painter, &layout);
    Ok(painter.into_image())
}

/// Walks elements in document order and computes each style.
fn compute_styles(dom: &Dom, rules: &[Rule]) -> HashMap<NodeId, Style> {
    let mut styles = HashMap::new();
    let document = dom.document();
    let mut root_font_size = 16.0_f32;
    for node in dom.descendants(document) {
        if !matches!(dom.kind(node), Some(dom::NodeKind::Element { .. })) {
            continue;
        }
        let parent = dom.parent(node).filter(|&parent| parent != document);
        let parent_style = parent.and_then(|parent| styles.get(&parent).cloned());
        let is_root = parent.is_none();
        let style = compute_style(
            dom,
            rules,
            node,
            parent_style.as_ref(),
            if is_root { 16.0 } else { root_font_size },
        );
        if is_root {
            root_font_size = style.font_size;
        }
        styles.insert(node, style);
    }
    styles
}

/// Computes one element's style: inherited base, matching declarations in
/// cascade order, then the `style` attribute last.
fn compute_style(
    dom: &Dom,
    rules: &[Rule],
    element: NodeId,
    parent: Option<&Style>,
    root_font_size: f32,
) -> Style {
    let mut collected: Vec<Cascaded> = Vec::new();

    for rule in rules {
        let Some(specificity) = rule.selectors.matching_specificity(dom, element) else {
            continue;
        };
        for (index, declared) in rule.declarations.iter().enumerate() {
            collected.push(Cascaded {
                key: DeclKey {
                    important: u8::from(declared.important),
                    origin: origin_rank(rule.origin, declared.important),
                    specificity,
                    order: rule.order,
                    index: u32::try_from(index).unwrap_or(u32::MAX),
                },
                declared: declared.clone(),
            });
        }
    }

    // The `style` attribute is author origin and outranks author selectors
    // (<https://drafts.csswg.org/css-cascade-5/#style-attribute>).
    if let Some(attribute) = dom.attribute(element, "style") {
        for (index, declared) in parse_inline_style(&attribute).into_iter().enumerate() {
            collected.push(Cascaded {
                key: DeclKey {
                    important: u8::from(declared.important),
                    origin: origin_rank(Origin::Author, declared.important),
                    specificity: u32::MAX,
                    order: u32::MAX,
                    index: u32::try_from(index).unwrap_or(u32::MAX),
                },
                declared,
            });
        }
    }

    collected.sort_by_key(|entry| entry.key);

    let mut style = match parent {
        Some(parent) => Style::inherited_from(parent),
        None => Style::initial(),
    };

    // font-size resolves before other properties so `em` is final
    // (<https://drafts.csswg.org/css-cascade-5/#computed-value>).
    let parent_font_size = parent.map_or(style.font_size, |parent| parent.font_size);
    if let Some(entry) = collected
        .iter()
        .rev()
        .find(|entry| matches!(entry.declared.decl, Decl::FontSize(_)))
        && let Decl::FontSize(length) = &entry.declared.decl
    {
        style.font_size = length
            .resolve(parent_font_size, parent_font_size, root_font_size)
            .max(0.0);
    }

    for entry in &collected {
        crate::style::apply(entry.declared.decl.clone(), &mut style, root_font_size);
    }

    // `border-width` computes to zero when the style is none
    // (<https://drafts.csswg.org/css-backgrounds-3/#border-width>).
    style.border = style.border.map(|mut side| {
        if side.style == crate::style::BorderStyle::None {
            side.width = 0.0;
        }
        side
    });

    style
}

/// Origin ranking for cascade sorting. The UA sheet here is presentational,
/// so important author declarations win; revisit when a user origin exists.
fn origin_rank(origin: Origin, important: bool) -> u8 {
    match (origin, important) {
        // UA defaults lose to everything; important author declarations win
        // over every normal one. Important UA declarations share the middle
        // rank with normal author declarations.
        (Origin::UserAgent, false) | (Origin::Author, true) => 0,
        (Origin::Author, false) | (Origin::UserAgent, true) => 1,
    }
}

/// Whether a viewport or scale value is usable.
fn valid_dimension(value: f32) -> bool {
    value.is_finite() && value > 0.0
}

/// Paints a laid-out tree in CSS paint order: background, border, then
/// content (<https://drafts.csswg.org/css2/#painting-order>).
fn paint(painter: &mut Painter, layout: &LayoutBox) {
    if layout.style.visibility != crate::style::Visibility::Visible {
        return;
    }
    let style = &layout.style;
    let rect = layout.rect;
    if style.background.a > 0 {
        painter.fill_rect(rect, style.background);
    }

    let border = style.border;
    if border.top.paints() {
        painter.fill_rect(
            Rect::new(rect.x, rect.y, rect.width, border.top.width),
            border.top.color,
        );
    }
    if border.right.paints() {
        painter.fill_rect(
            Rect::new(
                rect.right() - border.right.width,
                rect.y + border.top.width,
                border.right.width,
                rect.height - border.top.width - border.bottom.width,
            ),
            border.right.color,
        );
    }
    if border.bottom.paints() {
        painter.fill_rect(
            Rect::new(
                rect.x,
                rect.bottom() - border.bottom.width,
                rect.width,
                border.bottom.width,
            ),
            border.bottom.color,
        );
    }
    if border.left.paints() {
        painter.fill_rect(
            Rect::new(
                rect.x,
                rect.y + border.top.width,
                border.left.width,
                rect.height - border.top.width - border.bottom.width,
            ),
            border.left.color,
        );
    }

    let clipped = style.overflow == crate::style::Overflow::Hidden;
    if clipped {
        painter.push_clip(layout.padding_box());
    }
    for item in &layout.items {
        match item {
            PaintItem::Box(child) => paint(painter, child),
            PaintItem::Text(run) => {
                painter.draw_text(
                    &run.text,
                    run.x,
                    run.baseline,
                    FontStyle {
                        size: run.font_size,
                        weight: run.weight,
                    },
                    run.color,
                );
                match run.decoration {
                    crate::style::TextDecoration::None => {}
                    crate::style::TextDecoration::Underline => {
                        painter.fill_rect(
                            Rect::new(
                                run.x,
                                run.baseline + run.font_size * 0.1,
                                run.width,
                                (run.font_size * 0.06).max(1.0),
                            ),
                            run.color,
                        );
                    }
                    crate::style::TextDecoration::LineThrough => {
                        painter.fill_rect(
                            Rect::new(
                                run.x,
                                run.baseline - run.font_size * 0.3,
                                run.width,
                                (run.font_size * 0.06).max(1.0),
                            ),
                            run.color,
                        );
                    }
                }
            }
        }
    }
    if clipped {
        painter.pop_clip();
    }
}
