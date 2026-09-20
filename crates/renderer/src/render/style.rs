//! The computed style model the layout engine consumes.
//!
//! Cascading and property parsing live in [`crate::render::stylo`] — Servo's engine.
//! This module holds only the small resolved-value types the box tree, Taffy
//! bridge, and paint step read, plus the initial values every mapping starts
//! from (`stylo_map`).

use crate::render::color::Color;
use crate::render::font::Weight;
use crate::render::geometry::Edges;

// ── Computed value types ────────────────────────────────────────────────────

/// `display`, with the box kinds this engine formats
/// (<https://drafts.csswg.org/css-display-3/#display-value-summary>).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Display {
    /// No box.
    None,
    /// Block-level block container.
    Block,
    /// Inline-level box participating in a line.
    Inline,
    /// Inline-level block container.
    InlineBlock,
    /// Block-level flex container.
    Flex,
    /// Inline-level flex container.
    InlineFlex,
    /// Block-level grid container.
    Grid,
    /// Inline-level grid container.
    InlineGrid,
    /// Block-level list item (markers are not generated yet).
    ListItem,
}

/// `position` (<https://drafts.csswg.org/css-position-3/#position-property>).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Position {
    /// In flow.
    Static,
    /// In flow, offset visually.
    Relative,
    /// Out of flow, against the nearest positioned ancestor.
    Absolute,
    /// Out of flow, against the viewport.
    Fixed,
}

/// A resolved length: Stylo computes font-relative units during the cascade,
/// so only absolute pixels and percentages reach layout.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Length {
    /// CSS pixels.
    Px(f32),
    /// Percent of the relevant basis.
    Percent(f32),
}

impl Length {
    /// Resolves to pixels given the percentage basis
    /// (<https://drafts.csswg.org/css-values-4/#lengths>).
    pub(crate) fn resolve(self, basis: f32) -> f32 {
        match self {
            Self::Px(value) => value,
            Self::Percent(value) => basis * value / 100.0,
        }
    }
}

/// A `width`/`height`/`margin` value: a length or `auto`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Dimension {
    /// `auto`.
    Auto,
    /// A length.
    Length(Length),
}

/// `box-sizing` (<https://drafts.csswg.org/css-sizing-3/#box-sizing>).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BoxSizing {
    /// Width covers the content box.
    ContentBox,
    /// Width covers the border box.
    BorderBox,
}

/// `overflow`, collapsed to whether painting clips.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Overflow {
    /// Paint escapes the box.
    Visible,
    /// Paint is clipped to the padding box.
    Hidden,
}

/// `float`, which blockifies the box and takes it out of flow
/// (<https://drafts.csswg.org/css-floats-3/#float-property>).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Float {
    /// In flow.
    None,
    /// Shifted left, content flows around it.
    Left,
    /// Shifted right, content flows around it.
    Right,
}

/// `clear`, which pushes the box below earlier floats
/// (<https://drafts.csswg.org/css-floats-3/#clear-property>).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Clear {
    /// No clearance.
    None,
    /// Below earlier left floats.
    Left,
    /// Below earlier right floats.
    Right,
    /// Below all earlier floats.
    Both,
}

/// `border-style`, with non-solid styles painted as solid for now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BorderStyle {
    /// No border.
    None,
    /// A solid line.
    Solid,
}

/// One computed border side.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct BorderSide {
    /// Line width in pixels.
    pub width: f32,
    /// Line style.
    pub style: BorderStyle,
    /// Line color.
    pub color: Color,
}

impl BorderSide {
    /// The initial `medium none currentColor`.
    pub(crate) const INITIAL: Self = Self {
        width: 3.0,
        style: BorderStyle::None,
        color: Color::BLACK,
    };

    /// An absent border: zero width, no style.
    pub(crate) const NONE: Self = Self {
        width: 0.0,
        style: BorderStyle::None,
        color: Color::BLACK,
    };

    /// Whether anything paints.
    pub(crate) fn paints(self) -> bool {
        self.style != BorderStyle::None && self.width > 0.0
    }
}

/// `text-align` (<https://drafts.csswg.org/css-text-3/#text-align-property>).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TextAlign {
    /// Left edges align.
    Left,
    /// Right edges align.
    Right,
    /// Centers align.
    Center,
}

/// `white-space` (<https://drafts.csswg.org/css-text-3/#white-space-property>).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WhiteSpace {
    /// Collapse, wrap.
    Normal,
    /// Collapse, no wrap.
    Nowrap,
    /// Preserve, break at newlines.
    Pre,
    /// Preserve, wrap.
    PreWrap,
}

/// `text-decoration-line`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TextDecoration {
    /// No line.
    None,
    /// Under the baseline.
    Underline,
    /// Through the middle.
    LineThrough,
}

/// `visibility`, without `collapse`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Visibility {
    /// Painted.
    Visible,
    /// Laid out, not painted.
    Hidden,
}

/// `text-transform`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TextTransform {
    /// As authored.
    None,
    /// Uppercased.
    Uppercase,
    /// Lowercased.
    Lowercase,
}

/// `line-height`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum LineHeight {
    /// The face's own line metrics.
    Normal,
    /// A multiple of the font size.
    Number(f32),
    /// A length.
    Px(f32),
}

/// `vertical-align`, reduced to the values line layout distinguishes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum VerticalAlign {
    /// Alphabetic baseline.
    Baseline,
    /// Top of the line box.
    Top,
    /// Middle of the line box.
    Middle,
    /// Bottom of the line box.
    Bottom,
}

/// `flex-direction` (<https://drafts.csswg.org/css-flexbox-1/#flex-direction-property>).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FlexDirection {
    /// Main axis left to right.
    Row,
    /// Main axis right to left.
    RowReverse,
    /// Main axis top to bottom.
    Column,
    /// Main axis bottom to top.
    ColumnReverse,
}

/// `flex-wrap`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FlexWrap {
    /// One line.
    Nowrap,
    /// Wrap in main-axis order.
    Wrap,
    /// Wrap in reverse order.
    WrapReverse,
}

/// `justify-content`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum JustifyContent {
    /// Stretch auto-sized grid tracks to fill the container.
    Stretch,
    /// Pack at the start.
    FlexStart,
    /// Pack at the end.
    FlexEnd,
    /// Pack in the middle.
    Center,
    /// First and last flush, equal gaps between.
    SpaceBetween,
    /// Equal gaps around every item.
    SpaceAround,
    /// Equal gaps, including the edges.
    SpaceEvenly,
}

/// `align-items`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AlignItems {
    /// Stretch to the line.
    Stretch,
    /// Pack at the cross start.
    FlexStart,
    /// Pack at the cross end.
    FlexEnd,
    /// Center in the line.
    Center,
    /// Align baselines.
    Baseline,
}

/// `align-self`, where `auto` defers to the container.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AlignSelf {
    /// Use the container's `align-items`.
    Auto,
    /// Stretch to the line.
    Stretch,
    /// Pack at the cross start.
    FlexStart,
    /// Pack at the cross end.
    FlexEnd,
    /// Center in the line.
    Center,
    /// Align baselines.
    Baseline,
}

/// `align-content`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AlignContent {
    /// Stretch lines to fill the container.
    Stretch,
    /// Pack lines at the start.
    FlexStart,
    /// Pack lines at the end.
    FlexEnd,
    /// Center the lines.
    Center,
    /// Equal gaps between lines.
    SpaceBetween,
    /// Equal gaps around lines.
    SpaceAround,
}

/// One grid track sizing function, without line names
/// (<https://drafts.csswg.org/css-grid-1/#track-sizing>).
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum TrackSize {
    /// `auto`.
    Auto,
    /// A fixed length or percentage.
    Length(Length),
    /// A flexible fraction.
    Flex(f32),
}

/// One `grid-template` track: a single size, a minmax, or a repeat.
/// Nested `repeat()` is rejected; `minmax()` may appear inside `repeat()`.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum GridTrack {
    /// One track.
    Single(TrackSize),
    /// `minmax(min, max)`.
    MinMax(TrackSize, TrackSize),
    /// `repeat(count, tracks)`.
    Repeat(u16, Vec<GridTrack>),
}

/// One grid line placement
/// (<https://drafts.csswg.org/css-grid-1/#line-placement>). Named lines are
/// not supported.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GridPlacement {
    /// Automatic placement.
    Auto,
    /// An explicit line number (negative counts from the end).
    Line(i32),
    /// A span of tracks.
    Span(u16),
}

/// Grid placement on one axis: `start / end`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct GridLine {
    /// Start line.
    pub start: GridPlacement,
    /// End line.
    pub end: GridPlacement,
}

impl GridLine {
    /// Automatic placement on both sides.
    pub(crate) const AUTO: Self = Self {
        start: GridPlacement::Auto,
        end: GridPlacement::Auto,
    };
}

/// One computed style.
///
/// Lengths are resolved except percentages, which need layout-time bases
/// (<https://drafts.csswg.org/css-cascade-5/#computed-value>).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Style {
    /// Box generation.
    pub display: Display,
    /// Positioning scheme.
    pub position: Position,
    /// Used when `position` is not static.
    pub inset_top: Dimension,
    /// Used when `position` is not static.
    pub inset_right: Dimension,
    /// Used when `position` is not static.
    pub inset_bottom: Dimension,
    /// Used when `position` is not static.
    pub inset_left: Dimension,
    /// Preferred content width.
    pub width: Dimension,
    /// Preferred content height.
    pub height: Dimension,
    /// Preferred width divided by height for replaced content.
    pub aspect_ratio: Option<f32>,
    /// Lower width bound.
    pub min_width: Dimension,
    /// Lower height bound.
    pub min_height: Dimension,
    /// Upper width bound.
    pub max_width: Dimension,
    /// Upper height bound.
    pub max_height: Dimension,
    /// Outer margins.
    pub margin: Edges<Dimension>,
    /// Inner padding.
    pub padding: Edges<Length>,
    /// Border sides.
    pub border: Edges<BorderSide>,
    /// Corner radii in top-left, top-right, bottom-right, bottom-left order.
    pub border_radius: [Length; 4],
    /// Width interpretation.
    pub box_sizing: BoxSizing,
    /// Clipping.
    pub overflow: Overflow,
    /// Float placement.
    pub float: Float,
    /// Float clearance.
    pub clear: Clear,
    /// Text color.
    pub color: Color,
    /// Background fill.
    pub background: Color,
    /// Font size in pixels.
    pub font_size: f32,
    /// Font weight.
    pub font_weight: Weight,
    /// Line height.
    pub line_height: LineHeight,
    /// Horizontal alignment of lines.
    pub text_align: TextAlign,
    /// Whitespace processing.
    pub white_space: WhiteSpace,
    /// Decoration line.
    pub text_decoration: TextDecoration,
    /// Text case mapping.
    pub text_transform: TextTransform,
    /// Extra advance per character.
    pub letter_spacing: f32,
    /// Painted or not.
    pub visibility: Visibility,
    /// Group opacity. Zero suppresses the element's complete paint subtree.
    pub opacity: f32,
    /// Baseline alignment for inline-level boxes.
    pub vertical_align: VerticalAlign,
    /// Flex main axis.
    pub flex_direction: FlexDirection,
    /// Flex wrapping.
    pub flex_wrap: FlexWrap,
    /// Main-axis distribution.
    pub justify_content: JustifyContent,
    /// Cross-axis default.
    pub align_items: AlignItems,
    /// Cross-axis override.
    pub align_self: AlignSelf,
    /// Multi-line distribution.
    pub align_content: AlignContent,
    /// Flex grow factor.
    pub flex_grow: f32,
    /// Flex shrink factor.
    pub flex_shrink: f32,
    /// Flex base size.
    pub flex_basis: Dimension,
    /// Paint/order override.
    pub order: i32,
    /// Main-axis gap.
    pub row_gap: Length,
    /// Cross-axis gap.
    pub column_gap: Length,
    /// Grid column tracks (`none` is the empty list).
    pub grid_template_columns: Vec<GridTrack>,
    /// Grid row tracks (`none` is the empty list).
    pub grid_template_rows: Vec<GridTrack>,
    /// Grid column placement.
    pub grid_column: GridLine,
    /// Grid row placement.
    pub grid_row: GridLine,
    /// Default cross-axis alignment for grid items.
    pub justify_items: AlignItems,
}

impl Style {
    /// The initial value of every property
    /// (<https://drafts.csswg.org/css-cascade-5/#initial-values>).
    pub(crate) fn initial() -> Self {
        Self {
            display: Display::Inline,
            position: Position::Static,
            inset_top: Dimension::Auto,
            inset_right: Dimension::Auto,
            inset_bottom: Dimension::Auto,
            inset_left: Dimension::Auto,
            width: Dimension::Auto,
            height: Dimension::Auto,
            aspect_ratio: None,
            min_width: Dimension::Auto,
            min_height: Dimension::Auto,
            max_width: Dimension::Auto,
            max_height: Dimension::Auto,
            margin: Edges::new(
                Dimension::Length(Length::Px(0.0)),
                Dimension::Length(Length::Px(0.0)),
                Dimension::Length(Length::Px(0.0)),
                Dimension::Length(Length::Px(0.0)),
            ),
            padding: Edges::new(
                Length::Px(0.0),
                Length::Px(0.0),
                Length::Px(0.0),
                Length::Px(0.0),
            ),
            border: Edges::new(
                BorderSide::INITIAL,
                BorderSide::INITIAL,
                BorderSide::INITIAL,
                BorderSide::INITIAL,
            ),
            border_radius: [Length::Px(0.0); 4],
            box_sizing: BoxSizing::ContentBox,
            overflow: Overflow::Visible,
            float: Float::None,
            clear: Clear::None,
            color: Color::BLACK,
            background: Color::TRANSPARENT,
            font_size: 16.0,
            font_weight: Weight::Normal,
            line_height: LineHeight::Normal,
            text_align: TextAlign::Left,
            white_space: WhiteSpace::Normal,
            text_decoration: TextDecoration::None,
            text_transform: TextTransform::None,
            letter_spacing: 0.0,
            visibility: Visibility::Visible,
            opacity: 1.0,
            vertical_align: VerticalAlign::Baseline,
            flex_direction: FlexDirection::Row,
            flex_wrap: FlexWrap::Nowrap,
            justify_content: JustifyContent::FlexStart,
            align_items: AlignItems::Stretch,
            align_self: AlignSelf::Auto,
            align_content: AlignContent::Stretch,
            flex_grow: 0.0,
            flex_shrink: 1.0,
            flex_basis: Dimension::Auto,
            order: 0,
            row_gap: Length::Px(0.0),
            column_gap: Length::Px(0.0),
            grid_template_columns: Vec::new(),
            grid_template_rows: Vec::new(),
            grid_column: GridLine::AUTO,
            grid_row: GridLine::AUTO,
            justify_items: AlignItems::Stretch,
        }
    }

    /// A style that inherits every inherited property from `parent` and
    /// resets the rest to initial values
    /// (<https://drafts.csswg.org/css-cascade-5/#inheritance>).
    pub(crate) fn inherited_from(parent: &Self) -> Self {
        Self {
            color: parent.color,
            font_size: parent.font_size,
            font_weight: parent.font_weight,
            line_height: parent.line_height,
            text_align: parent.text_align,
            white_space: parent.white_space,
            text_transform: parent.text_transform,
            letter_spacing: parent.letter_spacing,
            visibility: parent.visibility,
            ..Self::initial()
        }
    }
}
