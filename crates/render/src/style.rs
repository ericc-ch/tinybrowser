//! The CSS subset: parsed declarations, the computed style model, stylesheet
//! parsing, and the UA stylesheet.
//!
//! Property syntax follows CSS Values and Units
//! (<https://drafts.csswg.org/css-values-4/>); the supported property set is
//! the subset the layout engine implements. Unknown properties and invalid
//! values are dropped, exactly as a browser drops them
//! (<https://drafts.csswg.org/css-syntax-3/#error-handling>).

use cssparser::{
    AtRuleParser, CowRcStr, DeclarationParser, ParseError, Parser, ParserInput, ParserState,
    QualifiedRuleParser, RuleBodyItemParser, RuleBodyParser, StyleSheetParser, Token,
};
use dom::{CompiledSelectors, Dom};

use crate::color::Color;
use crate::font::Weight;
use crate::geometry::Edges;

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

/// A length, with font-relative units still unresolved.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Length {
    /// CSS pixels.
    Px(f32),
    /// Percent of the relevant basis.
    Percent(f32),
    /// Times the element's own font size.
    Em(f32),
    /// Times the root element's font size.
    Rem(f32),
}

impl Length {
    /// Resolves to pixels given the percentage basis, the element's font
    /// size, and the root font size
    /// (<https://drafts.csswg.org/css-values-4/#lengths>).
    pub(crate) fn resolve(self, basis: f32, font_size: f32, root_font_size: f32) -> f32 {
        match self {
            Self::Px(value) => value,
            Self::Percent(value) => basis * value / 100.0,
            Self::Em(value) => font_size * value,
            Self::Rem(value) => root_font_size * value,
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

    /// Applies one text declaration to a draft style.
    fn apply_text(text: &Decl, style: &mut Style, font_size: f32, root_font_size: f32) {
        match text {
            Decl::TextAlign(align) => style.text_align = *align,
            Decl::WhiteSpace(space) => style.white_space = *space,
            Decl::TextDecoration(decoration) => style.text_decoration = *decoration,
            Decl::TextTransform(transform) => style.text_transform = *transform,
            Decl::LetterSpacing(length) => {
                style.letter_spacing = length.resolve(font_size, font_size, root_font_size);
            }
            Decl::Visibility(visibility) => style.visibility = *visibility,
            Decl::VerticalAlign(align) => style.vertical_align = *align,
            _ => {}
        }
    }

    /// Applies one flex declaration to a draft style.
    fn apply_flex(flex: &Decl, style: &mut Style) {
        match flex {
            Decl::FlexGrow(grow) => style.flex_grow = *grow,
            Decl::FlexShrink(shrink) => style.flex_shrink = *shrink,
            Decl::FlexBasis(basis) => style.flex_basis = *basis,
            Decl::Order(order) => style.order = *order,
            Decl::RowGap(gap) => style.row_gap = *gap,
            Decl::ColumnGap(gap) => style.column_gap = *gap,
            _ => {}
        }
    }

    /// Applies one grid declaration to a draft style.
    fn apply_grid(grid: &Decl, style: &mut Style) {
        match grid {
            Decl::GridTemplateColumns(tracks) => style.grid_template_columns.clone_from(tracks),
            Decl::GridTemplateRows(tracks) => style.grid_template_rows.clone_from(tracks),
            Decl::GridColumn(line) => style.grid_column = *line,
            Decl::GridRow(line) => style.grid_row = *line,
            Decl::JustifyItems(align) => style.justify_items = *align,
            _ => {}
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

// ── Declaration model ───────────────────────────────────────────────────────

/// A color that may be `currentColor`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum SpecColor {
    /// `currentColor`, resolved against the element's `color`.
    Current,
    /// A concrete color.
    Value(Color),
}

impl SpecColor {
    /// Resolves against the computed `color`.
    pub(crate) fn resolve(self, current: Color) -> Color {
        match self {
            Self::Current => current,
            Self::Value(color) => color,
        }
    }
}

/// One parsed declaration in cascade order.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Decl {
    /// `display`.
    Display(Display),
    /// `position`.
    Position(Position),
    /// `top`.
    InsetTop(Dimension),
    /// `right`.
    InsetRight(Dimension),
    /// `bottom`.
    InsetBottom(Dimension),
    /// `left`.
    InsetLeft(Dimension),
    /// `width`.
    Width(Dimension),
    /// `height`.
    Height(Dimension),
    /// `min-width`.
    MinWidth(Dimension),
    /// `min-height`.
    MinHeight(Dimension),
    /// `max-width`.
    MaxWidth(Dimension),
    /// `max-height`.
    MaxHeight(Dimension),
    /// One margin side.
    MarginSide(Side, Dimension),
    /// One padding side.
    PaddingSide(Side, Length),
    /// One border width side.
    BorderWidth(Side, f32),
    /// One border style side.
    BorderStyleValue(Side, BorderStyle),
    /// One border color side.
    BorderColor(Side, SpecColor),
    /// `box-sizing`.
    BoxSizing(BoxSizing),
    /// `overflow`.
    Overflow(Overflow),
    /// `float`.
    Float(Float),
    /// `clear`.
    Clear(Clear),
    /// `color`.
    Color(Color),
    /// `background-color`.
    Background(Color),
    /// `font-size`, still in declared units.
    FontSize(Length),
    /// `font-weight`.
    FontWeight(Weight),
    /// `line-height`.
    LineHeightValue(LineHeightSpec),
    /// `text-align`.
    TextAlign(TextAlign),
    /// `white-space`.
    WhiteSpace(WhiteSpace),
    /// `text-decoration`.
    TextDecoration(TextDecoration),
    /// `text-transform`.
    TextTransform(TextTransform),
    /// `letter-spacing`.
    LetterSpacing(Length),
    /// `visibility`.
    Visibility(Visibility),
    /// `vertical-align`.
    VerticalAlign(VerticalAlign),
    /// `flex-direction`.
    FlexDirection(FlexDirection),
    /// `flex-wrap`.
    FlexWrap(FlexWrap),
    /// `justify-content`.
    JustifyContent(JustifyContent),
    /// `align-items`.
    AlignItems(AlignItems),
    /// `align-self`.
    AlignSelf(AlignSelf),
    /// `align-content`.
    AlignContent(AlignContent),
    /// `flex-grow`.
    FlexGrow(f32),
    /// `flex-shrink`.
    FlexShrink(f32),
    /// `flex-basis`.
    FlexBasis(Dimension),
    /// `order`.
    Order(i32),
    /// `row-gap`.
    RowGap(Length),
    /// `column-gap`.
    ColumnGap(Length),
    /// `grid-template-columns`.
    GridTemplateColumns(Vec<GridTrack>),
    /// `grid-template-rows`.
    GridTemplateRows(Vec<GridTrack>),
    /// `grid-column`.
    GridColumn(GridLine),
    /// `grid-row`.
    GridRow(GridLine),
    /// `justify-items`.
    JustifyItems(AlignItems),
}

/// A box side for per-side declarations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Side {
    /// Top side.
    Top,
    /// Right side.
    Right,
    /// Bottom side.
    Bottom,
    /// Left side.
    Left,
}

/// `line-height` before resolution against `font-size`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum LineHeightSpec {
    /// Normal.
    Normal,
    /// A multiple.
    Number(f32),
    /// A length or percentage.
    Length(Length),
}

/// `font-size` keywords, against the 16px medium
/// (<https://drafts.csswg.org/css-fonts-4/#absolute-size-mapping>).
fn font_size_keyword(name: &str) -> Option<f32> {
    Some(match name {
        "xx-small" => 9.0,
        "x-small" => 10.0,
        "small" => 13.0,
        "medium" => 16.0,
        "large" => 18.0,
        "x-large" => 24.0,
        "xx-large" => 32.0,
        "xxx-large" => 48.0,
        _ => return None,
    })
}

// ── Value parsing ───────────────────────────────────────────────────────────

/// Runs `parse` over `value` and requires the whole value to be consumed.
fn with_value<T>(
    value: &str,
    parse: impl for<'i, 't> FnOnce(&mut Parser<'i, 't>) -> Result<T, ParseError<'i, ()>>,
) -> Option<T> {
    let mut input = ParserInput::new(value);
    let mut parser = Parser::new(&mut input);
    parser.parse_entirely(parse).ok()
}

/// `<length>`: a number with a unit, percentages allowed
/// (<https://drafts.csswg.org/css-values-4/#length-value>).
fn parse_length<'i>(input: &mut Parser<'i, '_>) -> Result<Length, ParseError<'i, ()>> {
    if let Ok(percent) = input.try_parse(|input: &mut Parser<'_, '_>| input.expect_percentage()) {
        return Ok(Length::Percent(percent * 100.0));
    }
    let location = input.current_source_location();
    match input.next()?.clone() {
        Token::Number { value: 0.0, .. } => Ok(Length::Px(0.0)),
        Token::Dimension { value, unit, .. } => match unit.to_ascii_lowercase().as_str() {
            "px" => Ok(Length::Px(value)),
            "em" => Ok(Length::Em(value)),
            "rem" => Ok(Length::Rem(value)),
            _ => Err(location.new_custom_error(())),
        },
        token => Err(location.new_unexpected_token_error(token)),
    }
}

/// `<length> | auto`.
fn parse_dimension<'i>(input: &mut Parser<'i, '_>) -> Result<Dimension, ParseError<'i, ()>> {
    if input.try_parse(|input| input.expect_ident_matching("auto")).is_ok() {
        return Ok(Dimension::Auto);
    }
    Ok(Dimension::Length(parse_length(input)?))
}

/// One to four values expanded to the sides, following the CSS shorthand
/// rules (<https://drafts.csswg.org/css-values-4/#component-multipliers>).
fn parse_sides<'i, 't, T: Copy, F>(
    input: &mut Parser<'i, 't>,
    parse: F,
) -> Result<Edges<T>, ParseError<'i, ()>>
where
    F: Fn(&mut Parser<'i, 't>) -> Result<T, ParseError<'i, ()>>,
{
    let first = parse(input)?;
    let second = input.try_parse(&parse).unwrap_or(first);
    let third = input.try_parse(&parse).unwrap_or(first);
    let fourth = input.try_parse(&parse).unwrap_or(second);
    Ok(Edges::new(first, second, third, fourth))
}

/// A single color token: named, hex, or a `rgb()`/`rgba()` function.
fn parse_color<'i>(input: &mut Parser<'i, '_>) -> Result<SpecColor, ParseError<'i, ()>> {
    let location = input.current_source_location();
    match input.next()?.clone() {
        Token::Ident(name) => match name.to_ascii_lowercase().as_str() {
            "currentcolor" => Ok(SpecColor::Current),
            _ => Color::parse(&name).map(SpecColor::Value).ok_or_else(|| location.new_custom_error(())),
        },
        Token::Hash(value) | Token::IDHash(value) => Color::parse(&format!("#{value}"))
            .map(SpecColor::Value)
            .ok_or_else(|| location.new_custom_error(())),
        Token::Function(name) if name.eq_ignore_ascii_case("rgb") || name.eq_ignore_ascii_case("rgba") => {
            let name = name.to_string();
            let arguments = input.parse_nested_block(|input| {
                let start = input.position();
                while input.next_including_whitespace_and_comments().is_ok() {}
                Ok::<_, ParseError<'i, ()>>(input.slice_from(start).to_owned())
            })?;
            Color::parse(&format!("{name}({arguments})"))
                .map(SpecColor::Value)
                .ok_or_else(|| location.new_custom_error(()))
        }
        token => Err(location.new_unexpected_token_error(token)),
    }
}

/// `border-width`: a keyword or length.
fn parse_border_width<'i>(input: &mut Parser<'i, '_>) -> Result<f32, ParseError<'i, ()>> {
    if let Ok(name) =
        input.try_parse(|input: &mut Parser<'_, '_>| input.expect_ident_cloned())
    {
        return match name.to_ascii_lowercase().as_str() {
            "thin" => Ok(1.0),
            "medium" => Ok(3.0),
            "thick" => Ok(5.0),
            _ => Err(input.new_custom_error(())),
        };
    }
    let Length::Px(width) = parse_length(input)? else {
        return Err(input.new_custom_error(()));
    };
    Ok(width.max(0.0))
}

/// `border-style`: any non-`none` style paints as solid for now.
fn parse_border_style<'i>(input: &mut Parser<'i, '_>) -> Result<BorderStyle, ParseError<'i, ()>> {
    let name = input.expect_ident_cloned()?;
    Ok(match name.to_ascii_lowercase().as_str() {
        "none" | "hidden" => BorderStyle::None,
        "solid" | "dashed" | "dotted" | "double" | "groove" | "ridge" | "inset" | "outset" => BorderStyle::Solid,
        _ => return Err(input.new_custom_error(())),
    })
}

/// A `border`/`border-top` shorthand: any of width, style, color in any
/// order (<https://drafts.csswg.org/css-backgrounds-3/#border-shorthands>).
#[derive(Clone, Copy, Debug, Default)]
struct BorderShorthand {    width: Option<f32>,
    style: Option<BorderStyle>,
    color: Option<SpecColor>,
}

fn parse_border_shorthand<'i>(input: &mut Parser<'i, '_>) -> Result<BorderShorthand, ParseError<'i, ()>> {
    let mut out = BorderShorthand::default();
    while !input.is_exhausted() {
        if out.width.is_none()
            && let Ok(width) = input.try_parse(parse_border_width)
        {
            out.width = Some(width);
            continue;
        }
        if out.style.is_none()
            && let Ok(style) = input.try_parse(parse_border_style)
        {
            out.style = Some(style);
            continue;
        }
        if out.color.is_none()
            && let Ok(color) = input.try_parse(parse_color)
        {
            out.color = Some(color);
            continue;
        }
        return Err(input.new_custom_error(()));
    }
    Ok(out)
}

/// `background` shorthand, reduced to the background color.
fn parse_background<'i>(input: &mut Parser<'i, '_>) -> Result<Color, ParseError<'i, ()>> {
    let mut color = Color::TRANSPARENT;
    while !input.is_exhausted() {
        if let Ok(spec) = input.try_parse(parse_color) {
            color = spec.resolve(Color::BLACK);
            continue;
        }
        let location = input.current_source_location();
        if input.next().is_err() {
            return Err(location.new_custom_error(()));
        }
    }
    Ok(color)
}

/// One `margin`/`padding` length list.
fn parse_margins<'i>(input: &mut Parser<'i, '_>) -> Result<Edges<Dimension>, ParseError<'i, ()>> {
    parse_sides(input, parse_dimension)
}

fn parse_paddings<'i>(input: &mut Parser<'i, '_>) -> Result<Edges<Length>, ParseError<'i, ()>> {
    parse_sides(input, parse_length)
}

fn parse_border_widths<'i>(input: &mut Parser<'i, '_>) -> Result<Edges<f32>, ParseError<'i, ()>> {
    parse_sides(input, parse_border_width)
}

fn parse_border_styles<'i>(input: &mut Parser<'i, '_>) -> Result<Edges<BorderStyle>, ParseError<'i, ()>> {
    parse_sides(input, parse_border_style)
}

fn parse_border_colors<'i>(input: &mut Parser<'i, '_>) -> Result<Edges<SpecColor>, ParseError<'i, ()>> {
    parse_sides(input, parse_color)
}

/// `line-height`: `normal`, a number, or a length.
fn parse_line_height<'i>(input: &mut Parser<'i, '_>) -> Result<LineHeightSpec, ParseError<'i, ()>> {
    if let Ok(name) = input.try_parse(Parser::expect_ident_cloned) {
        return match name.to_ascii_lowercase().as_str() {
            "normal" => Ok(LineHeightSpec::Normal),
            _ => Err(input.new_custom_error(())),
        };
    }
    if let Ok(number) = input.try_parse(Parser::expect_number) {
        return Ok(LineHeightSpec::Number(number.max(0.0)));
    }
    Ok(LineHeightSpec::Length(parse_length(input)?))
}

/// `<integer>`, as `Token::Number.int_value`
/// (<https://drafts.csswg.org/css-values-4/#integer-value>).
fn parse_integer<'i>(input: &mut Parser<'i, '_>) -> Result<i32, ParseError<'i, ()>> {
    let location = input.current_source_location();
    match input.next()?.clone() {
        Token::Number {
            int_value: Some(value),
            ..
        } => Ok(value),
        token => Err(location.new_unexpected_token_error(token)),
    }
}

/// One track size: `auto`, a length, or a flexible fraction
/// (<https://drafts.csswg.org/css-grid-1/#track-sizing>).
fn parse_track_size<'i>(input: &mut Parser<'i, '_>) -> Result<TrackSize, ParseError<'i, ()>> {
    if let Ok(name) = input.try_parse(|input: &mut Parser<'i, '_>| input.expect_ident_cloned()) {
        if name.eq_ignore_ascii_case("auto") {
            return Ok(TrackSize::Auto);
        }
        return Err(input.new_custom_error(()));
    }
    if let Ok(flex) = input.try_parse(
        |input: &mut Parser<'i, '_>| -> Result<f32, ParseError<'i, ()>> {
            match input.next()?.clone() {
                Token::Dimension { value, ref unit, .. }
                    if unit.eq_ignore_ascii_case("fr") =>
                {
                    Ok(value)
                }
                _ => Err(input.new_custom_error(())),
            }
        },
    ) {
        return Ok(TrackSize::Flex(flex.max(0.0)));
    }
    Ok(TrackSize::Length(parse_length(input)?))
}

/// One `grid-template` track: a single size, `minmax()`, or `repeat()`.
/// Nested `repeat()` is rejected, as the spec forbids it.
fn parse_grid_track<'i>(input: &mut Parser<'i, '_>) -> Result<GridTrack, ParseError<'i, ()>> {
    if let Ok(track) = input.try_parse(|input: &mut Parser<'i, '_>| {
        let location = input.current_source_location();
        let name = match input.next()?.clone() {
            Token::Function(name) => name.to_string(),
            token => return Err(location.new_unexpected_token_error(token)),
        };
        if name.eq_ignore_ascii_case("minmax") {
            let (min, max) = input.parse_nested_block(|input| {
                let min = parse_track_size(input)?;
                input.expect_comma()?;
                let max = parse_track_size(input)?;
                Ok((min, max))
            })?;
            return Ok(GridTrack::MinMax(min, max));
        }
        if name.eq_ignore_ascii_case("repeat") {
            let (count, tracks) = input.parse_nested_block(|input| {
                let count = parse_integer(input)?;
                input.expect_comma()?;
                let mut tracks = Vec::new();
                while !input.is_exhausted() {
                    tracks.push(parse_repeat_track(input)?);
                }
                Ok((count, tracks))
            })?;
            let count = u16::try_from(count).map_err(|_| input.new_custom_error(()))?;
            if count == 0 || tracks.is_empty() {
                return Err(input.new_custom_error(()));
            }
            return Ok(GridTrack::Repeat(count, tracks));
        }
        Err(input.new_custom_error(()))
    }) {
        return Ok(track);
    }
    Ok(GridTrack::Single(parse_track_size(input)?))
}

/// One track inside `repeat()`: a single size or `minmax()`.
fn parse_repeat_track<'i>(input: &mut Parser<'i, '_>) -> Result<GridTrack, ParseError<'i, ()>> {
    if let Ok((min, max)) = input.try_parse(|input: &mut Parser<'i, '_>| {
        let location = input.current_source_location();
        match input.next()?.clone() {
            Token::Function(name) if name.eq_ignore_ascii_case("minmax") => {
                input.parse_nested_block(|input| {
                    let min = parse_track_size(input)?;
                    input.expect_comma()?;
                    let max = parse_track_size(input)?;
                    Ok((min, max))
                })
            }
            token => Err(location.new_unexpected_token_error(token)),
        }
    }) {
        return Ok(GridTrack::MinMax(min, max));
    }
    Ok(GridTrack::Single(parse_track_size(input)?))
}

/// A `grid-template-columns/rows` track list, or `none` for no tracks.
fn parse_track_list<'i>(input: &mut Parser<'i, '_>) -> Result<Vec<GridTrack>, ParseError<'i, ()>> {
    if let Ok(name) = input.try_parse(|input: &mut Parser<'i, '_>| input.expect_ident_cloned()) {
        if name.eq_ignore_ascii_case("none") {
            return Ok(Vec::new());
        }
        return Err(input.new_custom_error(()));
    }
    let mut tracks = Vec::new();
    while !input.is_exhausted() {
        tracks.push(parse_grid_track(input)?);
    }
    if tracks.is_empty() {
        return Err(input.new_custom_error(()));
    }
    Ok(tracks)
}

/// One grid line placement: `auto`, a line number, or `span N`.
fn parse_grid_placement<'i>(
    input: &mut Parser<'i, '_>,
) -> Result<GridPlacement, ParseError<'i, ()>> {
    if let Ok(name) = input.try_parse(|input: &mut Parser<'i, '_>| input.expect_ident_cloned()) {
        if name.eq_ignore_ascii_case("auto") {
            return Ok(GridPlacement::Auto);
        }
        if name.eq_ignore_ascii_case("span") {
            let count = parse_integer(input)?;
            return u16::try_from(count)
                .map(GridPlacement::Span)
                .map_err(|_| input.new_custom_error(()));
        }
        return Err(input.new_custom_error(()));
    }
    parse_integer(input).map(GridPlacement::Line)
}

/// A `grid-column/row` value: `start / end`, with a lone `span` belonging to
/// the end (<https://drafts.csswg.org/css-grid-1/#line-placement>).
fn parse_grid_line<'i>(input: &mut Parser<'i, '_>) -> Result<GridLine, ParseError<'i, ()>> {
    let first = parse_grid_placement(input)?;
    if input
        .try_parse(|input: &mut Parser<'i, '_>| input.expect_delim('/'))
        .is_ok()
    {
        return Ok(GridLine {
            start: first,
            end: parse_grid_placement(input)?,
        });
    }
    if matches!(first, GridPlacement::Span(_)) {
        Ok(GridLine {
            start: GridPlacement::Auto,
            end: first,
        })
    } else {
        Ok(GridLine {
            start: first,
            end: GridPlacement::Auto,
        })
    }
}

/// `font-size`: keywords, lengths, and percentages.
fn parse_font_size<'i>(input: &mut Parser<'i, '_>) -> Result<Length, ParseError<'i, ()>> {    if let Ok(name) = input.try_parse(Parser::expect_ident_cloned) {
        if let Some(px) = font_size_keyword(&name) {
            return Ok(Length::Px(px));
        }
        if name.eq_ignore_ascii_case("larger") {
            return Ok(Length::Em(1.2));
        }
        if name.eq_ignore_ascii_case("smaller") {
            return Ok(Length::Em(0.833));
        }
        return Err(input.new_custom_error(()));
    }
    parse_length(input)
}

/// `font-weight`: keywords or the numeric scale.
fn parse_font_weight<'i>(input: &mut Parser<'i, '_>) -> Result<Weight, ParseError<'i, ()>> {
    if let Ok(name) = input.try_parse(Parser::expect_ident_cloned) {
        return match name.to_ascii_lowercase().as_str() {
            "normal" | "lighter" => Ok(Weight::Normal),
            "bold" | "bolder" => Ok(Weight::Bold),
            _ => Err(input.new_custom_error(())),
        };
    }
    let weight = parse_integer(input)?;
    Ok(if weight >= 600 { Weight::Bold } else { Weight::Normal })
}

/// Parses one declaration into zero or more [`Decl`]s. The `!important`
/// flag was already removed by the caller.
#[expect(
    clippy::too_many_lines,
    reason = "the property dispatch table: one arm per supported CSS property keeps the supported set in one place"
)]
pub(crate) fn parse_declaration(name: &str, value: &str) -> Vec<Decl> {
    let mut out = Vec::new();
    match name {
        "display" => {
            if let Some(display) = with_value(value, |input| {
                let name = input.expect_ident_cloned()?;
                Ok(match name.to_ascii_lowercase().as_str() {
                    "none" => Display::None,
                    "block" | "flow-root" | "table" | "table-row" | "table-cell"
                    | "table-row-group" | "table-header-group" | "table-footer-group"
                    | "table-caption" => Display::Block,
                    "inline" => Display::Inline,
                    "inline-block" => Display::InlineBlock,
                    "flex" => Display::Flex,
                    "inline-flex" => Display::InlineFlex,
                    "grid" => Display::Grid,
                    "inline-grid" => Display::InlineGrid,
                    "list-item" => Display::ListItem,
                    _ => return Err(input.new_custom_error(())),
                })
            }) {
                out.push(Decl::Display(display));
            }
        }
        "position" => {
            if let Some(position) = with_value(value, |input| {
                let name = input.expect_ident_cloned()?;
                Ok(match name.to_ascii_lowercase().as_str() {
                    "relative" => Position::Relative,
                    "absolute" => Position::Absolute,
                    "fixed" => Position::Fixed,
                    "static" | "sticky" => Position::Static,
                    _ => return Err(input.new_custom_error(())),
                })
            }) {
                out.push(Decl::Position(position));
            }
        }
        "top" | "right" | "bottom" | "left" => {
            push_inset_declaration(&mut out, name, value);
        }
        "width" => push_dimension(&mut out, value, Decl::Width),
        "height" => push_dimension(&mut out, value, Decl::Height),
        "min-width" => push_dimension(&mut out, value, Decl::MinWidth),
        "min-height" => push_dimension(&mut out, value, Decl::MinHeight),
        "max-width" => push_dimension(&mut out, value, Decl::MaxWidth),
        "max-height" => push_dimension(&mut out, value, Decl::MaxHeight),
        "margin" | "margin-top" | "margin-right" | "margin-bottom" | "margin-left" | "padding" | "padding-top" | "padding-right" | "padding-bottom" | "padding-left" => {
            push_spacing_declaration(&mut out, name, value);
        }
        "border" | "border-top" | "border-right" | "border-bottom" | "border-left" | "border-width" | "border-style" | "border-color" | "border-top-width" | "border-right-width" | "border-bottom-width" | "border-left-width" | "border-top-style" | "border-right-style" | "border-bottom-style" | "border-left-style" | "border-top-color" | "border-right-color" | "border-bottom-color" | "border-left-color" => {
            push_border_declaration(&mut out, name, value);
        }
        "box-sizing" => {
            if let Some(sizing) = with_value(value, |input| {
                let name = input.expect_ident_cloned()?;
                Ok(match name.to_ascii_lowercase().as_str() {
                    "content-box" => BoxSizing::ContentBox,
                    "border-box" => BoxSizing::BorderBox,
                    _ => return Err(input.new_custom_error(())),
                })
            }) {
                out.push(Decl::BoxSizing(sizing));
            }
        }
        "overflow" | "overflow-x" | "overflow-y" => {
            if let Some(overflow) = with_value(value, |input| {
                let name = input.expect_ident_cloned()?;
                Ok(match name.to_ascii_lowercase().as_str() {
                    "visible" => Overflow::Visible,
                    _ => Overflow::Hidden,
                })
            }) {
                out.push(Decl::Overflow(overflow));
            }
        }
        "float" | "clear" => {
            push_float_declaration(&mut out, name, value);
        }
        "color" => {
            if let Some(color) = with_value(value, parse_color) {
                out.push(Decl::Color(color.resolve(Color::BLACK)));
            }
        }
        "background" => {
            if let Some(color) = with_value(value, parse_background) {
                out.push(Decl::Background(color));
            }
        }
        "background-color" => {
            if let Some(color) = with_value(value, parse_color) {
                out.push(Decl::Background(color.resolve(Color::BLACK)));
            }
        }
        "font-size" => {
            if let Some(size) = with_value(value, parse_font_size) {
                out.push(Decl::FontSize(size));
            }
        }
        "font-weight" => {
            if let Some(weight) = with_value(value, parse_font_weight) {
                out.push(Decl::FontWeight(weight));
            }
        }
        "line-height" => {
            if let Some(height) = with_value(value, parse_line_height) {
                out.push(Decl::LineHeightValue(height));
            }
        }
        "text-align" => {
            if let Some(align) = with_value(value, |input| {
                let name = input.expect_ident_cloned()?;
                Ok(match name.to_ascii_lowercase().as_str() {
                    "left" | "start" | "justify" | "-webkit-left" | "-webkit-start" => TextAlign::Left,
                    "right" | "end" | "-webkit-right" | "-webkit-end" => TextAlign::Right,
                    "center" | "-webkit-center" => TextAlign::Center,
                    _ => return Err(input.new_custom_error(())),
                })
            }) {
                out.push(Decl::TextAlign(align));
            }
        }
        "white-space" => {
            if let Some(space) = with_value(value, |input| {
                let name = input.expect_ident_cloned()?;
                Ok(match name.to_ascii_lowercase().as_str() {
                    "normal" => WhiteSpace::Normal,
                    "nowrap" => WhiteSpace::Nowrap,
                    "pre" => WhiteSpace::Pre,
                    "pre-wrap" | "break-spaces" | "pre-line" => WhiteSpace::PreWrap,
                    _ => return Err(input.new_custom_error(())),
                })
            }) {
                out.push(Decl::WhiteSpace(space));
            }
        }
        "text-decoration" | "text-decoration-line" => {
            if let Some(decoration) = with_value(value, |input| {
                let mut decoration = TextDecoration::None;
                while !input.is_exhausted() {
                    let name = input.expect_ident_cloned()?;
                    match name.to_ascii_lowercase().as_str() {
                        "underline" => decoration = TextDecoration::Underline,
                        "line-through" => decoration = TextDecoration::LineThrough,
                        "none" | "overline" | "blink" => {}
                        _ => return Err(input.new_custom_error(())),
                    }
                }
                Ok(decoration)
            }) {
                out.push(Decl::TextDecoration(decoration));
            }
        }
        "text-transform" => {
            if let Some(transform) = with_value(value, |input| {
                let name = input.expect_ident_cloned()?;
                Ok(match name.to_ascii_lowercase().as_str() {
                    "uppercase" => TextTransform::Uppercase,
                    "lowercase" => TextTransform::Lowercase,
                    "none" | "capitalize" => TextTransform::None,
                    _ => return Err(input.new_custom_error(())),
                })
            }) {
                out.push(Decl::TextTransform(transform));
            }
        }
        "letter-spacing" => {
            if let Some(Length::Px(spacing)) = with_value(value, parse_length) {
                out.push(Decl::LetterSpacing(Length::Px(spacing)));
            }
        }
        "visibility" => {
            if let Some(visibility) = with_value(value, |input| {
                let name = input.expect_ident_cloned()?;
                Ok(match name.to_ascii_lowercase().as_str() {
                    "visible" => Visibility::Visible,
                    _ => Visibility::Hidden,
                })
            }) {
                out.push(Decl::Visibility(visibility));
            }
        }
        "vertical-align" => {
            if let Some(align) = with_value(value, |input| {
                let name = input.expect_ident_cloned()?;
                Ok(match name.to_ascii_lowercase().as_str() {
                    "top" => VerticalAlign::Top,
                    "middle" => VerticalAlign::Middle,
                    "bottom" => VerticalAlign::Bottom,
                    "baseline" | "sub" | "super" | "text-top" | "text-bottom" => VerticalAlign::Baseline,
                    _ => return Err(input.new_custom_error(())),
                })
            }) {
                out.push(Decl::VerticalAlign(align));
            }
        }
        "flex-direction" => {
            if let Some(direction) = with_value(value, |input| {
                let name = input.expect_ident_cloned()?;
                Ok(match name.to_ascii_lowercase().as_str() {
                    "row" => FlexDirection::Row,
                    "row-reverse" => FlexDirection::RowReverse,
                    "column" => FlexDirection::Column,
                    "column-reverse" => FlexDirection::ColumnReverse,
                    _ => return Err(input.new_custom_error(())),
                })
            }) {
                out.push(Decl::FlexDirection(direction));
            }
        }
        "flex-wrap" => {
            if let Some(wrap) = with_value(value, |input| {
                let name = input.expect_ident_cloned()?;
                Ok(match name.to_ascii_lowercase().as_str() {
                    "nowrap" => FlexWrap::Nowrap,
                    "wrap" => FlexWrap::Wrap,
                    "wrap-reverse" => FlexWrap::WrapReverse,
                    _ => return Err(input.new_custom_error(())),
                })
            }) {
                out.push(Decl::FlexWrap(wrap));
            }
        }
        "justify-content" => {
            if let Some(justify) = with_value(value, |input| {
                let name = input.expect_ident_cloned()?;
                Ok(match name.to_ascii_lowercase().as_str() {
                    "flex-start" | "start" | "left" => JustifyContent::FlexStart,
                    "flex-end" | "end" | "right" => JustifyContent::FlexEnd,
                    "center" => JustifyContent::Center,
                    "space-between" => JustifyContent::SpaceBetween,
                    "space-around" => JustifyContent::SpaceAround,
                    "space-evenly" => JustifyContent::SpaceEvenly,
                    _ => return Err(input.new_custom_error(())),
                })
            }) {
                out.push(Decl::JustifyContent(justify));
            }
        }
        "align-items" => {
            if let Some(align) = with_value(value, parse_align_items) {
                out.push(Decl::AlignItems(align));
            }
        }
        "justify-items" => {
            if let Some(align) = with_value(value, parse_align_items) {
                out.push(Decl::JustifyItems(align));
            }
        }
        "align-self" => {
            if let Some(align) = with_value(value, |input| {
                let name = input.expect_ident_cloned()?;
                Ok(match name.to_ascii_lowercase().as_str() {
                    "auto" => AlignSelf::Auto,
                    "stretch" => AlignSelf::Stretch,
                    "flex-start" | "start" => AlignSelf::FlexStart,
                    "flex-end" | "end" => AlignSelf::FlexEnd,
                    "center" => AlignSelf::Center,
                    "baseline" => AlignSelf::Baseline,
                    _ => return Err(input.new_custom_error(())),
                })
            }) {
                out.push(Decl::AlignSelf(align));
            }
        }
        "align-content" => {
            if let Some(align) = with_value(value, |input| {
                let name = input.expect_ident_cloned()?;
                Ok(match name.to_ascii_lowercase().as_str() {
                    "stretch" | "flex-start" | "start" => AlignContent::FlexStart,
                    "flex-end" | "end" => AlignContent::FlexEnd,
                    "center" => AlignContent::Center,
                    "space-between" => AlignContent::SpaceBetween,
                    "space-around" | "space-evenly" => AlignContent::SpaceAround,
                    _ => return Err(input.new_custom_error(())),
                })
            }) {
                out.push(Decl::AlignContent(align));
            }
        }
        "flex-grow" => {
            if let Some(grow) = with_value(value, |input| Ok(input.expect_number()?.max(0.0))) {
                out.push(Decl::FlexGrow(grow));
            }
        }
        "flex-shrink" => {
            if let Some(shrink) = with_value(value, |input| Ok(input.expect_number()?.max(0.0))) {
                out.push(Decl::FlexShrink(shrink));
            }
        }
        "flex-basis" => {
            if let Some(basis) = with_value(value, parse_dimension) {
                out.push(Decl::FlexBasis(basis));
            }
        }
        "flex" => {
            out.extend(parse_flex_shorthand(value));
        }
        "order" => {
            if let Some(order) = with_value(value, parse_integer) {
                out.push(Decl::Order(order));
            }
        }
        "gap" => {
            if let Some(edges) =
                with_value(value, |input| parse_sides(input, parse_length))
            {
                out.push(Decl::RowGap(edges.top));
                out.push(Decl::ColumnGap(edges.left));
            }
        }
        "grid-template-columns" | "grid-template-rows" | "grid-column" | "grid-row" => {
            push_grid_declaration(&mut out, name, value);
        }
        "row-gap" => push_length(&mut out, value, Decl::RowGap),
        "column-gap" => push_length(&mut out, value, Decl::ColumnGap),
        _ => {}
    }
    out
}

/// Parses any `margin*`/`padding*` property into declarations.
fn push_spacing_declaration(out: &mut Vec<Decl>, name: &str, value: &str) {
    match name {
        "margin" => {
            if let Some(edges) = with_value(value, parse_margins) {
                for (side, dimension) in side_values(edges) {
                    out.push(Decl::MarginSide(side, dimension));
                }
            }
        }
        "margin-top" => push_dimension(out, value, |d| Decl::MarginSide(Side::Top, d)),
        "margin-right" => push_dimension(out, value, |d| Decl::MarginSide(Side::Right, d)),
        "margin-bottom" => push_dimension(out, value, |d| Decl::MarginSide(Side::Bottom, d)),
        "margin-left" => push_dimension(out, value, |d| Decl::MarginSide(Side::Left, d)),
        "padding" => {
            if let Some(edges) = with_value(value, parse_paddings) {
                for (side, length) in side_values(edges) {
                    out.push(Decl::PaddingSide(side, length));
                }
            }
        }
        "padding-top" => push_length(out, value, |l| Decl::PaddingSide(Side::Top, l)),
        "padding-right" => push_length(out, value, |l| Decl::PaddingSide(Side::Right, l)),
        "padding-bottom" => push_length(out, value, |l| Decl::PaddingSide(Side::Bottom, l)),
        "padding-left" => push_length(out, value, |l| Decl::PaddingSide(Side::Left, l)),
        _ => {}
    }
}

/// Parses any `border*` property into declarations.
fn push_border_declaration(out: &mut Vec<Decl>, name: &str, value: &str) {
    match name {
        "border" => {
            if let Some(shorthand) = with_value(value, parse_border_shorthand) {
                for side in [Side::Top, Side::Right, Side::Bottom, Side::Left] {
                    push_border_shorthand(out, side, shorthand);
                }
            }
        }
        "border-top" | "border-right" | "border-bottom" | "border-left" => {
            if let Some(side) = side_from_name(name)
                && let Some(shorthand) = with_value(value, parse_border_shorthand)
            {
                push_border_shorthand(out, side, shorthand);
            }
        }
        "border-width" => {
            if let Some(edges) = with_value(value, parse_border_widths) {
                for (side, width) in side_values(edges) {
                    out.push(Decl::BorderWidth(side, width));
                }
            }
        }
        "border-style" => {
            if let Some(edges) = with_value(value, parse_border_styles) {
                for (side, style) in side_values(edges) {
                    out.push(Decl::BorderStyleValue(side, style));
                }
            }
        }
        "border-color" => {
            if let Some(edges) = with_value(value, parse_border_colors) {
                for (side, color) in side_values(edges) {
                    out.push(Decl::BorderColor(side, color));
                }
            }
        }
        "border-top-width" => push_border_width(out, value, Side::Top),
        "border-right-width" => push_border_width(out, value, Side::Right),
        "border-bottom-width" => push_border_width(out, value, Side::Bottom),
        "border-left-width" => push_border_width(out, value, Side::Left),
        "border-top-style" => push_border_style(out, value, Side::Top),
        "border-right-style" => push_border_style(out, value, Side::Right),
        "border-bottom-style" => push_border_style(out, value, Side::Bottom),
        "border-left-style" => push_border_style(out, value, Side::Left),
        "border-top-color" => push_border_color(out, value, Side::Top),
        "border-right-color" => push_border_color(out, value, Side::Right),
        "border-bottom-color" => push_border_color(out, value, Side::Bottom),
        "border-left-color" => push_border_color(out, value, Side::Left),
        _ => {}
    }
}

/// Parses `top`/`right`/`bottom`/`left` into a declaration.
fn push_inset_declaration(out: &mut Vec<Decl>, name: &str, value: &str) {
    match name {
        "top" => push_dimension(out, value, Decl::InsetTop),
        "right" => push_dimension(out, value, Decl::InsetRight),
        "bottom" => push_dimension(out, value, Decl::InsetBottom),
        "left" => push_dimension(out, value, Decl::InsetLeft),
        _ => {}
    }
}

/// Parses `float`/`clear` into a declaration.
fn push_float_declaration(out: &mut Vec<Decl>, name: &str, value: &str) {
    match name {
        "float" => {
            if let Some(float) = with_value(value, |input| {
                let name = input.expect_ident_cloned()?;
                Ok(match name.to_ascii_lowercase().as_str() {
                    "left" => Float::Left,
                    "right" => Float::Right,
                    _ => Float::None,
                })
            }) {
                out.push(Decl::Float(float));
            }
        }
        "clear" => {
            if let Some(clear) = with_value(value, |input| {
                let name = input.expect_ident_cloned()?;
                Ok(match name.to_ascii_lowercase().as_str() {
                    "left" => Clear::Left,
                    "right" => Clear::Right,
                    "both" => Clear::Both,
                    _ => Clear::None,
                })
            }) {
                out.push(Decl::Clear(clear));
            }
        }
        _ => {}
    }
}

/// Parses one grid longhand into a declaration.
fn push_grid_declaration(out: &mut Vec<Decl>, name: &str, value: &str) {
    match name {
        "grid-template-columns" => {
            if let Some(tracks) = with_value(value, parse_track_list) {
                out.push(Decl::GridTemplateColumns(tracks));
            }
        }
        "grid-template-rows" => {
            if let Some(tracks) = with_value(value, parse_track_list) {
                out.push(Decl::GridTemplateRows(tracks));
            }
        }
        "grid-column" => {
            if let Some(line) = with_value(value, parse_grid_line) {
                out.push(Decl::GridColumn(line));
            }
        }
        "grid-row" => {
            if let Some(line) = with_value(value, parse_grid_line) {
                out.push(Decl::GridRow(line));
            }
        }
        _ => {}
    }
}

/// `align-items` parser shared by `align-items`.
fn parse_align_items<'i>(input: &mut Parser<'i, '_>) -> Result<AlignItems, ParseError<'i, ()>> {
    let name = input.expect_ident_cloned()?;
    Ok(match name.to_ascii_lowercase().as_str() {
        "stretch" | "normal" => AlignItems::Stretch,
        "flex-start" | "start" => AlignItems::FlexStart,
        "flex-end" | "end" => AlignItems::FlexEnd,
        "center" => AlignItems::Center,
        "baseline" => AlignItems::Baseline,
        _ => return Err(input.new_custom_error(())),
    })
}

fn side_from_name(name: &str) -> Option<Side> {
    match name {
        "border-top" => Some(Side::Top),
        "border-right" => Some(Side::Right),
        "border-bottom" => Some(Side::Bottom),
        "border-left" => Some(Side::Left),
        _ => None,
    }
}

fn push_border_shorthand(out: &mut Vec<Decl>, side: Side, shorthand: BorderShorthand) {
    if let Some(width) = shorthand.width {
        out.push(Decl::BorderWidth(side, width));
    }
    if let Some(style) = shorthand.style {
        out.push(Decl::BorderStyleValue(side, style));
    }
    if let Some(color) = shorthand.color {
        out.push(Decl::BorderColor(side, color));
    }
}

/// Maps the four-edge helper onto sides in declaration order.
fn side_values<T: Copy>(edges: Edges<T>) -> [(Side, T); 4] {
    [
        (Side::Top, edges.top),
        (Side::Right, edges.right),
        (Side::Bottom, edges.bottom),
        (Side::Left, edges.left),
    ]
}

fn push_dimension(out: &mut Vec<Decl>, value: &str, make: impl FnOnce(Dimension) -> Decl) {
    if let Some(dimension) = with_value(value, parse_dimension) {
        out.push(make(dimension));
    }
}

fn push_length(out: &mut Vec<Decl>, value: &str, make: impl FnOnce(Length) -> Decl) {
    if let Some(length) = with_value(value, parse_length) {
        out.push(make(length));
    }
}

fn push_border_width(out: &mut Vec<Decl>, value: &str, side: Side) {
    if let Some(width) = with_value(value, parse_border_width) {
        out.push(Decl::BorderWidth(side, width));
    }
}

fn push_border_style(out: &mut Vec<Decl>, value: &str, side: Side) {
    if let Some(style) = with_value(value, parse_border_style) {
        out.push(Decl::BorderStyleValue(side, style));
    }
}

fn push_border_color(out: &mut Vec<Decl>, value: &str, side: Side) {
    if let Some(color) = with_value(value, parse_color) {
        out.push(Decl::BorderColor(side, color));
    }
}

/// `flex` shorthand (<https://drafts.csswg.org/css-flexbox-1/#flex-property>).
fn parse_flex_shorthand(value: &str) -> Vec<Decl> {
    use Dimension::{Auto, Length as DimLength};
    let parsed = with_value(value, |input| {
        if let Ok(name) = input.try_parse(Parser::expect_ident_cloned) {
            return match name.to_ascii_lowercase().as_str() {
                "none" => Ok((0.0, 0.0, Auto)),
                "auto" => Ok((1.0, 1.0, Auto)),
                _ => Err(input.new_custom_error(())),
            };
        }
        if let Ok(grow) = input.try_parse(Parser::expect_number) {
            let shrink = input.try_parse(Parser::expect_number).unwrap_or(1.0);
            let basis = input.try_parse(parse_dimension).unwrap_or(DimLength(Length::Px(0.0)));
            return Ok((grow.max(0.0), shrink.max(0.0), basis));
        }
        let basis = parse_dimension(input)?;
        Ok((1.0, 1.0, basis))
    });
    let Some((grow, shrink, basis)) = parsed else {
        return Vec::new();
    };
    vec![
        Decl::FlexGrow(grow),
        Decl::FlexShrink(shrink),
        Decl::FlexBasis(basis),
    ]
}

// ── Cascade application ─────────────────────────────────────────────────────

/// Applies one declaration to a draft style.
///
/// `font_size` is the element's resolved font size, so other lengths resolve
/// their `em` units against the final value rather than declaration order
/// (<https://drafts.csswg.org/css-cascade-5/#computed-value>).
pub(crate) fn apply(decl: Decl, style: &mut Style, root_font_size: f32) {
    let font_size = style.font_size;
    match decl {
        Decl::Display(display) => style.display = display,
        Decl::Position(position) => style.position = position,
        Decl::InsetTop(value) => style.inset_top = value,
        Decl::InsetRight(value) => style.inset_right = value,
        Decl::InsetBottom(value) => style.inset_bottom = value,
        Decl::InsetLeft(value) => style.inset_left = value,
        Decl::Width(value) => style.width = value,
        Decl::Height(value) => style.height = value,
        Decl::MinWidth(value) => style.min_width = value,
        Decl::MinHeight(value) => style.min_height = value,
        Decl::MaxWidth(value) => style.max_width = value,
        Decl::MaxHeight(value) => style.max_height = value,
        Decl::MarginSide(side, value) => {
            match side {
                Side::Top => style.margin.top = value,
                Side::Right => style.margin.right = value,
                Side::Bottom => style.margin.bottom = value,
                Side::Left => style.margin.left = value,
            }
        }
        Decl::PaddingSide(side, value) => {
            match side {
                Side::Top => style.padding.top = value,
                Side::Right => style.padding.right = value,
                Side::Bottom => style.padding.bottom = value,
                Side::Left => style.padding.left = value,
            }
        }
        Decl::BorderWidth(side, width) => {
            let border = &mut style.border;
            match side {
                Side::Top => border.top.width = width,
                Side::Right => border.right.width = width,
                Side::Bottom => border.bottom.width = width,
                Side::Left => border.left.width = width,
            }
        }
        Decl::BorderStyleValue(side, border_style) => {
            let border = &mut style.border;
            match side {
                Side::Top => border.top.style = border_style,
                Side::Right => border.right.style = border_style,
                Side::Bottom => border.bottom.style = border_style,
                Side::Left => border.left.style = border_style,
            }
        }
        Decl::BorderColor(side, color) => {
            let resolved = color.resolve(style.color);
            let border = &mut style.border;
            match side {
                Side::Top => border.top.color = resolved,
                Side::Right => border.right.color = resolved,
                Side::Bottom => border.bottom.color = resolved,
                Side::Left => border.left.color = resolved,
            }
        }
        Decl::BoxSizing(sizing) => style.box_sizing = sizing,
        Decl::Overflow(overflow) => style.overflow = overflow,
        Decl::Float(float) => style.float = float,
        Decl::Clear(clear) => style.clear = clear,
        Decl::Color(color) => style.color = color,
        Decl::Background(color) => style.background = color,
        Decl::FontWeight(weight) => style.font_weight = weight,
        Decl::LineHeightValue(spec) => {
            style.line_height = match spec {
                LineHeightSpec::Normal => LineHeight::Normal,
                LineHeightSpec::Number(number) => LineHeight::Number(number),
                LineHeightSpec::Length(length) => LineHeight::Px(
                    length.resolve(font_size, font_size, root_font_size),
                ),
            };
        }
        text @ (Decl::TextAlign(_)
        | Decl::WhiteSpace(_)
        | Decl::TextDecoration(_)
        | Decl::TextTransform(_)
        | Decl::LetterSpacing(_)
        | Decl::Visibility(_)
        | Decl::VerticalAlign(_)) => Style::apply_text(&text, style, font_size, root_font_size),
        Decl::FlexDirection(direction) => style.flex_direction = direction,
        Decl::FlexWrap(wrap) => style.flex_wrap = wrap,
        Decl::JustifyContent(justify) => style.justify_content = justify,
        Decl::AlignItems(align) => style.align_items = align,
        Decl::AlignSelf(align) => style.align_self = align,
        Decl::AlignContent(align) => style.align_content = align,
        flex @ (Decl::FlexGrow(_)
        | Decl::FlexShrink(_)
        | Decl::FlexBasis(_)
        | Decl::Order(_)
        | Decl::RowGap(_)
        | Decl::ColumnGap(_)) => Style::apply_flex(&flex, style),
        grid @ (Decl::GridTemplateColumns(_)
        | Decl::GridTemplateRows(_)
        | Decl::GridColumn(_)
        | Decl::GridRow(_)
        | Decl::JustifyItems(_)) => Style::apply_grid(&grid, style),
        // `font-size` is resolved before the other declarations apply.
        Decl::FontSize(_) => {}
    }
}

// ── Stylesheet parsing ──────────────────────────────────────────────────────

/// Which origin a sheet came from, for cascade ordering.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Origin {
    /// The UA stylesheet.
    UserAgent,
    /// Author sheets and `style` attributes.
    Author,
}

/// One parsed declaration with its `!important` flag.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Declared {
    /// The declaration.
    pub(crate) decl: Decl,
    /// Whether the declaration carried `!important`.
    pub(crate) important: bool,
}

/// One parsed style rule with its cascade sort key.
pub(crate) struct Rule {
    /// Compiled selectors, reusable across elements.
    pub(crate) selectors: CompiledSelectors,
    /// Declarations in source order.
    pub(crate) declarations: Vec<Declared>,
    /// Origin for cascade ordering.
    pub(crate) origin: Origin,
    /// Source order across all sheets.
    pub(crate) order: u32,
}

/// The UA stylesheet: the HTML rendering spec's presentational hints and
/// defaults for the elements this engine formats
/// (<https://html.spec.whatwg.org/multipage/rendering.html>).
pub(crate) const UA_STYLESHEET: &str = r"
html, body, div, p, h1, h2, h3, h4, h5, h6, ul, ol, li, dl, dt, dd,
header, footer, main, section, article, nav, aside, address, blockquote,
figure, figcaption, form, fieldset, hr, pre, table, thead, tbody, tfoot,
tr, td, th, caption, colgroup, video, audio, canvas, details, summary,
dialog, dir, menu, center, legend, output, optgroup, option {
  display: block;
}
head, title, meta, link, style, script, base, template, noscript, param, source, track {
  display: none;
}
body { margin: 8px; }
h1 { display: block; font-size: 2em; font-weight: bold; margin: 0.67em 0; }
h2 { display: block; font-size: 1.5em; font-weight: bold; margin: 0.83em 0; }
h3 { display: block; font-size: 1.17em; font-weight: bold; margin: 1em 0; }
h4 { display: block; font-weight: bold; margin: 1.33em 0; }
h5 { display: block; font-size: 0.83em; font-weight: bold; margin: 1.67em 0; }
h6 { display: block; font-size: 0.67em; font-weight: bold; margin: 2.33em 0; }
p { margin: 1em 0; }
blockquote { margin: 1em 40px; }
ul, ol { margin: 1em 0; padding-left: 40px; }
pre { margin: 1em 0; white-space: pre; }
hr { margin: 0.5em auto; border: 1px solid #808080; }
a:link { color: #0000ee; text-decoration: underline; }
b, strong { font-weight: bold; }
i, em, cite, var { font-style: italic; }
code, kbd, samp, pre, tt { font-family: monospace; }
small { font-size: 0.83em; }
big { font-size: 1.17em; }
sub, sup { font-size: 0.83em; vertical-align: baseline; }
center { text-align: center; }
table { border-collapse: separate; }
td, th { padding: 1px; }
th { font-weight: bold; text-align: center; }
img { display: inline-block; }
input, textarea, select, button { display: inline-block; }
textarea { white-space: pre-wrap; }
";

/// Parses a stylesheet into cascade rules. Rules with invalid selectors are
/// dropped whole, as browsers drop them
/// (<https://drafts.csswg.org/css-syntax-3/#consume-qualified-rule>).
pub(crate) fn parse_stylesheet(
    dom: &Dom,
    css: &str,
    origin: Origin,
    order: &mut u32,
    viewport_width: f32,
) -> Vec<Rule> {
    let mut input = ParserInput::new(css);
    let mut parser = Parser::new(&mut input);
    let mut handler = SheetParser {
        dom,
        origin,
        order,
        viewport_width,
        rules: Vec::new(),
    };
    for result in StyleSheetParser::new(&mut parser, &mut handler) {
        // Invalid rules are skipped; parsing continues at the next rule.
        let _ = result;
    }
    handler.rules
}

struct SheetParser<'d> {
    dom: &'d Dom,
    origin: Origin,
    order: &'d mut u32,
    viewport_width: f32,
    rules: Vec<Rule>,
}

/// A rule captured inside a nested block before selector compilation.
struct LeafRule {
    selectors: String,
    declarations: Vec<Declared>,
}

/// Compiles one leaf rule and appends it in source order.
fn push_leaf_rule(handler: &mut SheetParser<'_>, leaf: LeafRule) {
    let Ok(selectors) = handler.dom.compile_selectors(&leaf.selectors) else {
        return; // selector syntax error: drop the rule
    };
    if selectors.is_empty() {
        return;
    }
    let order = *handler.order;
    *handler.order = handler.order.wrapping_add(1);
    handler.rules.push(Rule {
        selectors,
        declarations: leaf.declarations,
        origin: handler.origin,
        order,
    });
}

/// One at-rule prelude captured as text.
struct AtPrelude<'i> {
    name: CowRcStr<'i>,
    prelude: CowRcStr<'i>,
}

impl<'i> QualifiedRuleParser<'i> for SheetParser<'_> {
    type Prelude = CowRcStr<'i>;
    type QualifiedRule = ();
    type Error = ();

    fn parse_prelude<'t>(
        &mut self,
        input: &mut Parser<'i, 't>,
    ) -> Result<Self::Prelude, ParseError<'i, Self::Error>> {
        let start = input.position();
        while input.next().is_ok() {}
        Ok(input.slice_from(start).into())
    }

    fn parse_block<'t>(
        &mut self,
        prelude: Self::Prelude,
        _start: &ParserState,
        input: &mut Parser<'i, 't>,
    ) -> Result<Self::QualifiedRule, ParseError<'i, Self::Error>> {
        let declarations = parse_declaration_block(input);
        if !declarations.is_empty() {
            push_leaf_rule(
                self,
                LeafRule {
                    selectors: prelude.to_string(),
                    declarations,
                },
            );
        }
        Ok(())
    }
}

impl<'i> AtRuleParser<'i> for SheetParser<'_> {
    type Prelude = AtPrelude<'i>;
    type AtRule = ();
    type Error = ();

    fn parse_prelude<'t>(
        &mut self,
        name: CowRcStr<'i>,
        input: &mut Parser<'i, 't>,
    ) -> Result<Self::Prelude, ParseError<'i, Self::Error>> {
        let start = input.position();
        while input.next().is_ok() {}
        Ok(AtPrelude {
            name,
            prelude: input.slice_from(start).into(),
        })
    }

    fn parse_block<'t>(
        &mut self,
        prelude: Self::Prelude,
        _start: &ParserState,
        input: &mut Parser<'i, 't>,
    ) -> Result<Self::AtRule, ParseError<'i, Self::Error>> {
        if prelude.name.eq_ignore_ascii_case("media") {
            if media_matches(&prelude.prelude, self.viewport_width) {
                let leaves = parse_leaf_rules(input);
                for leaf in leaves {
                    push_leaf_rule(self, leaf);
                }
            }
            return Ok(());
        }
        if prelude.name.eq_ignore_ascii_case("layer") && prelude.prelude.trim().is_empty() {
            let leaves = parse_leaf_rules(input);
            for leaf in leaves {
                push_leaf_rule(self, leaf);
            }
            return Ok(());
        }
        // @supports, @import, @font-face, @keyframes, @page, named @layer:
        // skipped; consuming the block keeps the outer parse aligned.
        input.skip_whitespace();
        while input.next().is_ok() {}
        Ok(())
    }
}

/// Parses a nested rule list (inside `@media`/`@layer`).
fn parse_leaf_rules(input: &mut Parser<'_, '_>) -> Vec<LeafRule> {
    let mut leaf = LeafSheetParser { rules: Vec::new() };
    for result in StyleSheetParser::new(input, &mut leaf) {
        // Invalid rules are skipped; parsing continues at the next rule.
        let _ = result;
    }
    leaf.rules
}

/// The rule parser used inside `@media` and `@layer` blocks.
struct LeafSheetParser {
    rules: Vec<LeafRule>,
}

impl<'i> QualifiedRuleParser<'i> for LeafSheetParser {
    type Prelude = CowRcStr<'i>;
    type QualifiedRule = ();
    type Error = ();

    fn parse_prelude<'t>(
        &mut self,
        input: &mut Parser<'i, 't>,
    ) -> Result<Self::Prelude, ParseError<'i, Self::Error>> {
        let start = input.position();
        while input.next().is_ok() {}
        Ok(input.slice_from(start).into())
    }

    fn parse_block<'t>(
        &mut self,
        prelude: Self::Prelude,
        _start: &ParserState,
        input: &mut Parser<'i, 't>,
    ) -> Result<Self::QualifiedRule, ParseError<'i, Self::Error>> {
        let declarations = parse_declaration_block(input);
        if !declarations.is_empty() {
            self.rules.push(LeafRule {
                selectors: prelude.to_string(),
                declarations,
            });
        }
        Ok(())
    }
}

impl<'i> AtRuleParser<'i> for LeafSheetParser {
    type Prelude = AtPrelude<'i>;
    type AtRule = ();
    type Error = ();

    fn parse_prelude<'t>(
        &mut self,
        name: CowRcStr<'i>,
        input: &mut Parser<'i, 't>,
    ) -> Result<Self::Prelude, ParseError<'i, Self::Error>> {
        let start = input.position();
        while input.next().is_ok() {}
        Ok(AtPrelude {
            name,
            prelude: input.slice_from(start).into(),
        })
    }

    fn parse_block<'t>(
        &mut self,
        _prelude: Self::Prelude,
        _start: &ParserState,
        input: &mut Parser<'i, 't>,
    ) -> Result<Self::AtRule, ParseError<'i, Self::Error>> {
        input.skip_whitespace();
        while input.next().is_ok() {}
        Ok(())
    }
}

/// Parses a `{ declarations }` block, expanding shorthands and recording
/// `!important` per declaration.
pub(crate) fn parse_declaration_block(input: &mut Parser<'_, '_>) -> Vec<Declared> {
    let mut handler = DeclarationHandler {
        declarations: Vec::new(),
    };
    for result in RuleBodyParser::<_, (), ()>::new(input, &mut handler) {
        // Invalid declarations are skipped; parsing continues at the next one.
        let _ = result;
    }
    handler.declarations
}

/// Parses one `style` attribute value.
pub(crate) fn parse_inline_style(value: &str) -> Vec<Declared> {
    let mut input = ParserInput::new(value);
    let mut parser = Parser::new(&mut input);
    parse_declaration_block(&mut parser)
}

/// The declaration parser inside a style rule block.
struct DeclarationHandler {
    declarations: Vec<Declared>,
}

impl<'i> DeclarationParser<'i> for DeclarationHandler {
    type Declaration = ();
    type Error = ();

    fn parse_value<'t>(
        &mut self,
        name: CowRcStr<'i>,
        input: &mut Parser<'i, 't>,
    ) -> Result<Self::Declaration, ParseError<'i, Self::Error>> {
        let start = input.position();
        while input.next().is_ok() {}
        let raw = input.slice_from(start);
        let (value, important) = split_important(raw);
        for decl in parse_declaration(&name.to_ascii_lowercase(), value) {
            self.declarations.push(Declared { decl, important });
        }
        Ok(())
    }
}

impl QualifiedRuleParser<'_> for DeclarationHandler {
    type Prelude = ();
    type QualifiedRule = ();
    type Error = ();
}

impl AtRuleParser<'_> for DeclarationHandler {
    type Prelude = ();
    type AtRule = ();
    type Error = ();
}

impl RuleBodyItemParser<'_, (), ()> for DeclarationHandler {
    fn parse_declarations(&self) -> bool {
        true
    }

    fn parse_qualified(&self) -> bool {
        false
    }
}

/// Splits a `!important` suffix from a declaration value.
fn split_important(raw: &str) -> (&str, bool) {
    let trimmed = raw.trim_end();
    let Some(bang) = trimmed.rfind('!') else {
        return (trimmed, false);
    };
    let flag = trimmed[bang + 1..].trim();
    if flag.eq_ignore_ascii_case("important") {
        (trimmed[..bang].trim_end(), true)
    } else {
        (trimmed, false)
    }
}

/// Evaluates the media queries this engine understands: `screen`/`all` and
/// width features joined by `and`
/// (<https://drafts.csswg.org/mediaqueries-5/#evaluating>).
pub(crate) fn media_matches(query: &str, viewport_width: f32) -> bool {
    let query = query.trim();
    if query.is_empty() {
        return true;
    }
    let mut matched = true;
    for part in query.split(" and ") {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        if part.eq_ignore_ascii_case("screen") || part.eq_ignore_ascii_case("all") {
            continue;
        }
        if let Some(value) = feature_value(part, "min-width") {
            matched &= viewport_width >= value;
            continue;
        }
        if let Some(value) = feature_value(part, "max-width") {
            matched &= viewport_width <= value;
            continue;
        }
        // Unsupported feature or a `not`/`,` form: exclude conservatively.
        matched = false;
    }
    matched
}

/// `(name: <length>)` from one media feature.
fn feature_value(feature: &str, name: &str) -> Option<f32> {
    let inner = feature.strip_prefix('(')?.strip_suffix(')')?;
    let (key, value) = inner.split_once(':')?;
    if !key.trim().eq_ignore_ascii_case(name) {
        return None;
    }
    let value = value.trim();
    let number = value.strip_suffix("px")?.trim();
    number.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::{Decl, Length, Style, parse_declaration};

    #[test]
    fn parses_box_and_text_properties() {
        assert_eq!(
            parse_declaration("margin", "1px 2em auto 4%"),
            vec![
                Decl::MarginSide(super::Side::Top, super::Dimension::Length(Length::Px(1.0))),
                Decl::MarginSide(super::Side::Right, super::Dimension::Length(Length::Em(2.0))),
                Decl::MarginSide(super::Side::Bottom, super::Dimension::Auto),
                Decl::MarginSide(super::Side::Left, super::Dimension::Length(Length::Percent(4.0))),
            ]
        );
        assert_eq!(
            parse_declaration("font-weight", "700"),
            vec![Decl::FontWeight(crate::font::Weight::Bold)]
        );
        assert_eq!(
            parse_declaration("color", "rgb(1, 2, 3)"),
            vec![Decl::Color(crate::color::Color::rgb(1, 2, 3))]
        );
        assert!(parse_declaration("width", "10furlongs").is_empty());
    }

    #[test]
    fn parses_inline_style_attribute() {
        let declarations =
            super::parse_inline_style("background: #ff0000; width: 100px; height: 40px");
        assert_eq!(declarations.len(), 3, "{declarations:?}");
        assert!(matches!(
            declarations[0].decl,
            Decl::Background(color) if color == crate::color::Color::rgb(255, 0, 0)
        ));
    }

    #[test]
    fn applies_em_after_font_size() {
        let mut style = Style::initial();
        style.font_size = 20.0;
        super::apply(Decl::MarginSide(super::Side::Top, super::Dimension::Length(Length::Em(2.0))), &mut style, 16.0);
        assert_eq!(
            style.margin.top,
            super::Dimension::Length(Length::Em(2.0)),
            "em stays symbolic until used"
        );
        let resolved = match style.margin.top {
            super::Dimension::Length(length) => length.resolve(100.0, style.font_size, 16.0),
            super::Dimension::Auto => 0.0,
        };
        assert!((resolved - 40.0).abs() < f32::EPSILON);
    }
}
