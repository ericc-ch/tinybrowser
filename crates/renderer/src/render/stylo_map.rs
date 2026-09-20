//! Computed values to our style model: Stylo's cascade output translated to
//! the [`Style`](crate::render::style::Style) the layout engine consumes.
//!
//! The mapping starts from [`Style::initial`] and overwrites every property
//! the layout engine reads. Anything our model cannot represent keeps its
//! initial value, which is exactly what the old hand-rolled cascade did when
//! a declaration failed to parse — with three deliberate exceptions, each
//! marked below:
//!
//! - Table display values map to `Block`, as the old `display` parser did.
//! - `float: inline-start/end` and `clear` logical values resolve physically
//!   (the tree is always left-to-right; no `direction` support exists yet).
//! - `font-style`, `font-family`, and other properties with no [`Style`]
//!   field are not read at all, as before.
//!
//! Lengths arrive computed: font-relative units are already pixels, so only
//! absolute lengths and percentages translate. `calc()` keeps no symbolic
//! form in our model, so any value containing it falls back to initial, like
//! an unparseable declaration did.

use style::properties::ComputedValues;
use style::values::computed::LengthPercentage;
use style::values::computed::length_percentage::Unpacked as UnpackedLp;
use style::values::specified::align::AlignFlags;

use crate::render::color::Color;
use crate::render::font::Weight;
use crate::render::geometry::Edges;
use crate::render::style::{
    AlignContent, AlignItems, AlignSelf, BorderSide, BorderStyle, BoxSizing, Clear, Dimension,
    Display, FlexDirection, FlexWrap, GridLine, GridPlacement, GridTrack, JustifyContent, Length,
    LineHeight, Overflow, Position, Style, TextAlign, TextDecoration, TextTransform, VerticalAlign,
    Visibility, WhiteSpace,
};

/// Translates one element's computed values into our layout style.
///
/// The mapping starts from [`Style::initial`] and overwrites every property
/// the layout engine reads. Anything our model cannot represent keeps its
/// initial value, which is exactly what the old hand-rolled cascade did when
/// a declaration failed to parse, with the deliberate exceptions marked on
/// each helper below.
pub(crate) fn map_style(values: &ComputedValues) -> Style {
    let mut style = Style::initial();
    map_box_model(values, &mut style);
    map_text(values, &mut style);
    map_flex_and_grid(values, &mut style);

    // `border-width` computes to zero when the style is none
    // (<https://drafts.csswg.org/css-backgrounds-3/#border-width>).
    style.border = style.border.map(|mut side| {
        if side.style == BorderStyle::None {
            side.width = 0.0;
        }
        side
    });

    style
}

/// Maps the box model: display, position, sizes, spacing, borders, and paint
/// basics. Colours come first because borders default to `currentColor`.
fn map_box_model(values: &ComputedValues, style: &mut Style) {
    use style::values::computed::{
        Clear as ComputedClear, Float as ComputedFloat, Overflow as ComputedOverflow,
        PositionProperty as ComputedPosition,
    };

    let boxy = values.get_box();
    let position = values.get_position();

    style.display = map_display(boxy.display);
    style.position = match boxy.position {
        ComputedPosition::Static | ComputedPosition::Sticky => Position::Static,
        ComputedPosition::Relative => Position::Relative,
        ComputedPosition::Absolute => Position::Absolute,
        ComputedPosition::Fixed => Position::Fixed,
        // `sticky` needs scrolling; in a one-shot screenshot it sits in flow.
    };
    style.inset_top = map_inset(&position.top);
    style.inset_right = map_inset(&position.right);
    style.inset_bottom = map_inset(&position.bottom);
    style.inset_left = map_inset(&position.left);
    style.width = map_size(&position.width);
    style.height = map_size(&position.height);
    style.min_width = map_size(&position.min_width);
    style.min_height = map_size(&position.min_height);
    style.max_width = map_max_size(&position.max_width);
    style.max_height = map_max_size(&position.max_height);

    let margin = values.get_margin();
    style.margin = Edges::new(
        map_margin(&margin.margin_top),
        map_margin(&margin.margin_right),
        map_margin(&margin.margin_bottom),
        map_margin(&margin.margin_left),
    );
    let padding = values.get_padding();
    style.padding = Edges::new(
        map_padding(&padding.padding_top),
        map_padding(&padding.padding_right),
        map_padding(&padding.padding_bottom),
        map_padding(&padding.padding_left),
    );

    let current_color = values.get_inherited_text().color;
    style.color = map_absolute(current_color);
    let border = values.get_border();
    style.border = Edges::new(
        map_border_side(
            &border.border_top_width,
            border.border_top_style,
            &border.border_top_color,
            current_color,
        ),
        map_border_side(
            &border.border_right_width,
            border.border_right_style,
            &border.border_right_color,
            current_color,
        ),
        map_border_side(
            &border.border_bottom_width,
            border.border_bottom_style,
            &border.border_bottom_color,
            current_color,
        ),
        map_border_side(
            &border.border_left_width,
            border.border_left_style,
            &border.border_left_color,
            current_color,
        ),
    );
    style.box_sizing = match position.box_sizing {
        style::properties::longhands::box_sizing::computed_value::T::ContentBox => {
            BoxSizing::ContentBox
        }
        style::properties::longhands::box_sizing::computed_value::T::BorderBox => {
            BoxSizing::BorderBox
        }
    };
    style.overflow = match boxy.overflow_x {
        ComputedOverflow::Visible => Overflow::Visible,
        // The old parser mapped every non-`visible` keyword to clipping.
        ComputedOverflow::Hidden
        | ComputedOverflow::Scroll
        | ComputedOverflow::Auto
        | ComputedOverflow::Clip => Overflow::Hidden,
    };
    style.float = match boxy.float {
        // Logical floats resolve physically; the tree is always left-to-right.
        ComputedFloat::Left | ComputedFloat::InlineStart => crate::render::style::Float::Left,
        ComputedFloat::Right | ComputedFloat::InlineEnd => crate::render::style::Float::Right,
        ComputedFloat::None => crate::render::style::Float::None,
    };
    style.clear = match boxy.clear {
        ComputedClear::Left | ComputedClear::InlineStart => Clear::Left,
        ComputedClear::Right | ComputedClear::InlineEnd => Clear::Right,
        ComputedClear::Both => Clear::Both,
        ComputedClear::None => Clear::None,
    };
    style.background = map_color(&values.get_background().background_color, current_color);
}

/// Maps fonts, text, and inherited paint: the properties paint and Parley
/// read.
fn map_text(values: &ComputedValues, style: &mut Style) {
    use style::values::computed::TextAlign as ComputedTextAlign;
    use style::values::specified::text::TextTransformCase;

    let font = values.get_font();
    let text = values.get_inherited_text();

    style.font_size = font.font_size.computed_size.px();
    style.font_weight = if font.font_weight.value() >= 600.0 {
        Weight::Bold
    } else {
        Weight::Normal
    };
    style.line_height = match font.line_height {
        style::values::computed::LineHeight::Normal => LineHeight::Normal,
        style::values::computed::LineHeight::Number(number) => LineHeight::Number(number.0),
        style::values::computed::LineHeight::Length(length) => LineHeight::Px(length.px()),
    };
    style.text_align = match text.text_align {
        ComputedTextAlign::Left
        | ComputedTextAlign::MozLeft
        | ComputedTextAlign::Start
        | ComputedTextAlign::Justify => TextAlign::Left,
        ComputedTextAlign::Right | ComputedTextAlign::MozRight | ComputedTextAlign::End => {
            TextAlign::Right
        }
        ComputedTextAlign::Center | ComputedTextAlign::MozCenter => TextAlign::Center,
    };
    style.white_space = map_white_space(text.white_space_collapse, text.text_wrap_mode);
    style.text_transform = match text.text_transform.case() {
        TextTransformCase::Uppercase => TextTransform::Uppercase,
        TextTransformCase::Lowercase => TextTransform::Lowercase,
        // `capitalize` has no model; the old parser dropped it.
        TextTransformCase::None | TextTransformCase::Capitalize => TextTransform::None,
    };
    style.letter_spacing = match text.letter_spacing.0.unpack() {
        UnpackedLp::Length(length) => length.px(),
        UnpackedLp::Percentage(percent) => percent.0 * style.font_size,
        // `calc()` keeps no symbolic form; fall back to initial.
        UnpackedLp::Calc(_) => 0.0,
    };
    style.visibility = match values.get_inherited_box().visibility {
        style::properties::longhands::visibility::computed_value::T::Visible => Visibility::Visible,
        style::properties::longhands::visibility::computed_value::T::Hidden
        | style::properties::longhands::visibility::computed_value::T::Collapse => {
            Visibility::Hidden
        }
    };
    style.vertical_align = map_vertical_align(values);

    let decorated = values.get_text().text_decoration_line;
    style.text_decoration =
        if decorated.contains(style::values::specified::TextDecorationLine::UNDERLINE) {
            TextDecoration::Underline
        } else if decorated.contains(style::values::specified::TextDecorationLine::LINE_THROUGH) {
            TextDecoration::LineThrough
        } else {
            TextDecoration::None
        };
}

/// Maps flex and grid properties, the containers Taffy lays out.
fn map_flex_and_grid(values: &ComputedValues, style: &mut Style) {
    use style::properties::longhands::{
        flex_direction::computed_value::T as ComputedFlexDirection,
        flex_wrap::computed_value::T as ComputedFlexWrap,
    };

    let position = values.get_position();
    style.flex_direction = match position.flex_direction {
        ComputedFlexDirection::Row => FlexDirection::Row,
        ComputedFlexDirection::RowReverse => FlexDirection::RowReverse,
        ComputedFlexDirection::Column => FlexDirection::Column,
        ComputedFlexDirection::ColumnReverse => FlexDirection::ColumnReverse,
    };
    style.flex_wrap = match position.flex_wrap {
        ComputedFlexWrap::Nowrap => FlexWrap::Nowrap,
        ComputedFlexWrap::Wrap => FlexWrap::Wrap,
        ComputedFlexWrap::WrapReverse => FlexWrap::WrapReverse,
    };
    style.justify_content =
        map_content_distribution(position.justify_content, JustifyContent::FlexStart);
    style.align_items = map_item_placement(position.align_items);
    style.align_self = map_self_alignment(position.align_self);
    style.align_content = map_align_content(position.align_content);
    style.flex_grow = position.flex_grow.0;
    style.flex_shrink = position.flex_shrink.0;
    style.flex_basis = map_flex_basis(&position.flex_basis);
    style.order = position.order;
    style.row_gap = map_gap(&position.row_gap);
    style.column_gap = map_gap(&position.column_gap);
    if let Some(tracks) = map_template(&position.grid_template_columns) {
        style.grid_template_columns = tracks;
    }
    if let Some(tracks) = map_template(&position.grid_template_rows) {
        style.grid_template_rows = tracks;
    }
    style.grid_column = map_grid_line(&position.grid_column_start, &position.grid_column_end);
    style.grid_row = map_grid_line(&position.grid_row_start, &position.grid_row_end);
    style.justify_items = map_justify_items(position.justify_items);
}

/// Maps `display` through outside/inside, like the old parser's keyword table.
fn map_display(display: style::values::computed::Display) -> Display {
    use style::values::specified::box_::{DisplayInside, DisplayOutside};
    if display.is_none() {
        return Display::None;
    }
    if display.is_list_item() {
        return Display::ListItem;
    }
    match (display.outside(), display.inside()) {
        (DisplayOutside::Inline, DisplayInside::FlowRoot) => Display::InlineBlock,
        (DisplayOutside::Inline, DisplayInside::Flex) => Display::InlineFlex,
        (DisplayOutside::Inline, DisplayInside::Grid) => Display::InlineGrid,
        (DisplayOutside::Block, DisplayInside::Flex) => Display::Flex,
        (DisplayOutside::Block, DisplayInside::Grid) => Display::Grid,
        // Tables and internals lay out as blocks, as before.
        (DisplayOutside::Block | DisplayOutside::InternalTable, _) => Display::Block,
        // `contents`, ruby, and the rest generate no supported box: inline,
        // the old initial value. `TableCaption` maps inline like an anonymous
        // caption wrapper did before, and `None` is unreachable here because
        // `is_none` returned above.
        (DisplayOutside::Inline | DisplayOutside::TableCaption | DisplayOutside::None, _) => {
            Display::Inline
        }
    }
}

/// Maps an inset (`top`/`right`/`bottom`/`left`).
fn map_inset(inset: &style::values::computed::Inset) -> Dimension {
    use style::values::computed::Inset;
    match inset {
        Inset::LengthPercentage(length) => {
            map_length_percentage(length).map_or(Dimension::Auto, Dimension::Length)
        }
        // `auto` and anchor functions both mean no offset here.
        _ => Dimension::Auto,
    }
}

/// Maps `width`/`height`/`min-width`/`min-height`.
fn map_size(size: &style::values::computed::Size) -> Dimension {
    use style::values::computed::Size;
    match size {
        Size::LengthPercentage(length) => {
            map_length_percentage(&length.0).map_or(Dimension::Auto, Dimension::Length)
        }
        // `auto` and intrinsic sizes all mean automatic sizing, as before.
        _ => Dimension::Auto,
    }
}

/// Maps `max-width`/`max-height`; `none` is our `auto`.
fn map_max_size(size: &style::values::computed::MaxSize) -> Dimension {
    use style::values::computed::MaxSize;
    match size {
        MaxSize::LengthPercentage(length) => {
            map_length_percentage(&length.0).map_or(Dimension::Auto, Dimension::Length)
        }
        // `none` and intrinsic sizes all mean unconstrained, as before.
        _ => Dimension::Auto,
    }
}

/// Maps `margin-*`; `auto` survives, `calc()` falls back to initial zero.
fn map_margin(margin: &style::values::computed::Margin) -> Dimension {
    use style::values::computed::Margin;
    match margin {
        Margin::LengthPercentage(length) => map_length_percentage(length)
            .map_or(Dimension::Length(Length::Px(0.0)), Dimension::Length),
        Margin::Auto => Dimension::Auto,
        // Anchor functions have no model; the old parser dropped them to zero.
        _ => Dimension::Length(Length::Px(0.0)),
    }
}

/// Maps `padding-*`; negative lengths cannot occur post-cascade.
fn map_padding(padding: &style::values::computed::NonNegativeLengthPercentage) -> Length {
    map_length_percentage(&padding.0).unwrap_or(Length::Px(0.0))
}

/// Maps one border side; keyword colors resolve against the element's own
/// `color`.
fn map_border_side(
    width: &style::values::computed::BorderSideWidth,
    style: style::values::computed::BorderStyle,
    color: &style::values::computed::Color,
    current: style::color::AbsoluteColor,
) -> BorderSide {
    BorderSide {
        width: width.0.to_f32_px(),
        style: match style {
            // Non-solid styles paint as solid, as documented on `BorderStyle`.
            style::values::computed::BorderStyle::None
            | style::values::computed::BorderStyle::Hidden => BorderStyle::None,
            _ => BorderStyle::Solid,
        },
        color: map_color(color, current),
    }
}

/// Maps `white-space` from its collapse and wrap longhands.
fn map_white_space(
    collapse: style::properties::longhands::white_space_collapse::computed_value::T,
    wrap: style::properties::longhands::text_wrap_mode::computed_value::T,
) -> WhiteSpace {
    use style::properties::longhands::text_wrap_mode::computed_value::T as Wrap;
    use style::properties::longhands::white_space_collapse::computed_value::T as Collapse;
    let preserve = !matches!(collapse, Collapse::Collapse);
    match (preserve, wrap) {
        (false, Wrap::Wrap) => WhiteSpace::Normal,
        (false, Wrap::Nowrap) => WhiteSpace::Nowrap,
        // `pre-line` and `break-spaces` have no model; preserved wrapping is
        // the closest behavior.
        (true, Wrap::Wrap) => WhiteSpace::PreWrap,
        (true, Wrap::Nowrap) => WhiteSpace::Pre,
    }
}

/// Maps `vertical-align` from the cascade's split longhands. Explicit shifts
/// collapse to the baseline, as the old keyword table did; otherwise the
/// baseline source decides.
fn map_vertical_align(values: &ComputedValues) -> VerticalAlign {
    use style::values::computed::{AlignmentBaseline, BaselineShift};
    use style::values::generics::box_::BaselineShiftKeyword;
    match &values.get_box().baseline_shift {
        BaselineShift::Keyword(BaselineShiftKeyword::Top) => VerticalAlign::Top,
        BaselineShift::Keyword(BaselineShiftKeyword::Center) => VerticalAlign::Middle,
        BaselineShift::Keyword(BaselineShiftKeyword::Bottom) => VerticalAlign::Bottom,
        // `sub`, `super`, and lengths all sat on the baseline before.
        BaselineShift::Keyword(BaselineShiftKeyword::Sub | BaselineShiftKeyword::Super)
        | BaselineShift::Length(_) => match values.get_box().alignment_baseline {
            AlignmentBaseline::Middle => VerticalAlign::Middle,
            AlignmentBaseline::TextTop => VerticalAlign::Top,
            AlignmentBaseline::TextBottom => VerticalAlign::Bottom,
            AlignmentBaseline::Baseline => VerticalAlign::Baseline,
        },
    }
}

/// Maps a content-distribution (`justify-content`) through its primary flag;
/// `normal` behaves as the property's initial packing.
fn map_content_distribution(
    distribution: style::values::computed::ContentDistribution,
    normal: JustifyContent,
) -> JustifyContent {
    JustifyContent::from_flag(distribution.primary().value()).unwrap_or(normal)
}

/// Maps `align-content` through its primary flag; `normal` stretches.
fn map_align_content(distribution: style::values::computed::ContentDistribution) -> AlignContent {
    AlignContent::from_flag(distribution.primary().value()).unwrap_or(AlignContent::Stretch)
}

/// Maps `align-items` through its primary flag.
fn map_item_placement(placement: style::values::computed::ItemPlacement) -> AlignItems {
    AlignItems::from_flag(placement.0.value()).unwrap_or(AlignItems::Stretch)
}

/// Maps `align-self` through its primary flag.
fn map_self_alignment(alignment: style::values::computed::SelfAlignment) -> AlignSelf {
    AlignSelf::from_flag(alignment.0.value()).unwrap_or(AlignSelf::Auto)
}

/// Maps `justify-items`, whose computed value carries the legacy quirk.
fn map_justify_items(items: style::values::computed::JustifyItems) -> AlignItems {
    AlignItems::from_flag(items.computed.0.0.value()).unwrap_or(AlignItems::Stretch)
}

/// Maps `flex-basis`; `content` sizes by content, which our `auto` means.
fn map_flex_basis(basis: &style::values::computed::FlexBasis) -> Dimension {
    use style::values::computed::FlexBasis as Basis;
    match basis {
        Basis::Content => Dimension::Auto,
        Basis::Size(size) => map_size(size),
    }
}

/// Maps `gap`; `normal` is zero spacing.
fn map_gap(gap: &style::values::computed::length::NonNegativeLengthPercentageOrNormal) -> Length {
    match gap {
        style::values::computed::length::NonNegativeLengthPercentageOrNormal::LengthPercentage(
            length,
        ) => map_length_percentage(&length.0).unwrap_or(Length::Px(0.0)),
        // `normal` is zero spacing.
        style::values::computed::length::NonNegativeLengthPercentageOrNormal::Normal => {
            Length::Px(0.0)
        }
    }
}

/// Maps a `grid-template` track list, or `None` when it holds `calc()`,
/// auto-repeats, or anything else without a model.
fn map_template(
    template: &style::values::computed::GridTemplateComponent,
) -> Option<Vec<GridTrack>> {
    use style::values::generics::grid::{GridTemplateComponent, RepeatCount, TrackListValue};
    // `none`, `subgrid`, and `masonry` all mean no explicit tracks.
    let GridTemplateComponent::TrackList(list) = template else {
        return Some(Vec::new());
    };
    if list.has_auto_repeat() {
        return None;
    }
    let mut out = Vec::with_capacity(list.values.len());
    for value in list.values.iter() {
        match value {
            TrackListValue::TrackSize(size) => out.push(map_track_size(size)?),
            TrackListValue::TrackRepeat(repeat) => {
                let count = match repeat.count {
                    RepeatCount::Number(count) => u16::try_from(count).map_err(|_| ()).ok()?,
                    RepeatCount::AutoFill | RepeatCount::AutoFit => return None,
                };
                let mut tracks = Vec::with_capacity(repeat.track_sizes.len());
                for size in repeat.track_sizes.iter() {
                    tracks.push(map_track_size(size)?);
                }
                out.push(GridTrack::Repeat(count, tracks));
            }
        }
    }
    Some(out)
}

/// Maps one track size; intrinsic sizes approximate as `auto`.
fn map_track_size(
    size: &style::values::generics::grid::TrackSize<style::values::computed::LengthPercentage>,
) -> Option<GridTrack> {
    use style::values::generics::grid::{TrackBreadth, TrackSize as Generic};
    let convert = |breadth: &TrackBreadth<style::values::computed::LengthPercentage>| match breadth
    {
        TrackBreadth::Breadth(length) => Some(crate::render::style::TrackSize::Length(
            map_length_percentage(length)?,
        )),
        TrackBreadth::Flex(flex) => Some(crate::render::style::TrackSize::Flex(flex.0)),
        // Intrinsic sizes approximate as `auto`, as before.
        TrackBreadth::Auto | TrackBreadth::MinContent | TrackBreadth::MaxContent => {
            Some(crate::render::style::TrackSize::Auto)
        }
    };
    match size {
        Generic::Breadth(breadth) => Some(GridTrack::Single(convert(breadth)?)),
        Generic::Minmax(min, max) => {
            let min = match min {
                // Flexible minimums are invalid; fall back like the old code.
                TrackBreadth::Flex(_) => crate::render::style::TrackSize::Auto,
                _ => convert(min)?,
            };
            Some(GridTrack::MinMax(min, convert(max)?))
        }
        // `fit-content()` has no model; `auto` sizes by content loosely.
        Generic::FitContent(_) => Some(GridTrack::Single(crate::render::style::TrackSize::Auto)),
    }
}

/// Maps one axis placement; named lines have no model and place automatically.
fn map_grid_line(
    start: &style::values::computed::GridLine,
    end: &style::values::computed::GridLine,
) -> GridLine {
    GridLine {
        start: map_placement(start),
        end: map_placement(end),
    }
}

/// Maps one line placement.
fn map_placement(line: &style::values::computed::GridLine) -> GridPlacement {
    if line.is_auto() || !line.ident.0.is_empty() {
        return GridPlacement::Auto;
    }
    if line.is_span {
        let count = line.line_num.max(1);
        return GridPlacement::Span(u16::try_from(count).unwrap_or(u16::MAX));
    }
    GridPlacement::Line(line.line_num)
}

/// Maps a length/percentage, or `None` for `calc()`.
fn map_length_percentage(length: &LengthPercentage) -> Option<Length> {
    match length.unpack() {
        UnpackedLp::Length(length) => Some(Length::Px(length.px())),
        // Stylo stores percentages as fractions; ours are 0-100.
        UnpackedLp::Percentage(percent) => Some(Length::Percent(percent.0 * 100.0)),
        UnpackedLp::Calc(_) => None,
    }
}

/// Maps a computed color, resolving `currentcolor`, `color-mix()`, relative
/// color syntax, and `contrast-color()` against the element's own `color`
/// (<https://drafts.csswg.org/css-color-5/#resolving-color-values>).
fn map_color(
    color: &style::values::computed::Color,
    current: style::color::AbsoluteColor,
) -> Color {
    map_absolute(color.resolve_to_absolute(&current))
}

/// Converts one absolute color through legacy sRGB into our RGBA bytes.
fn map_absolute(absolute: style::color::AbsoluteColor) -> Color {
    let srgb = absolute.into_srgb_legacy();
    let [red, green, blue, alpha] = *srgb.raw_components();
    Color::rgba(channel(red), channel(green), channel(blue), channel(alpha))
}

/// Converts one 0-1 channel to a byte.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the value is clamped to 0..=255 before the cast, so it is lossless"
)]
fn channel(value: f32) -> u8 {
    (value * 255.0).round().clamp(0.0, 255.0) as u8
}

impl AlignContent {
    /// Reads one content-distribution flag for lines.
    fn from_flag(flag: AlignFlags) -> Option<Self> {
        Some(match flag.value() {
            AlignFlags::NORMAL | AlignFlags::STRETCH => Self::Stretch,
            AlignFlags::FLEX_START | AlignFlags::START => Self::FlexStart,
            AlignFlags::FLEX_END | AlignFlags::END => Self::FlexEnd,
            AlignFlags::CENTER => Self::Center,
            AlignFlags::SPACE_BETWEEN => Self::SpaceBetween,
            AlignFlags::SPACE_AROUND => Self::SpaceAround,
            _ => return None,
        })
    }
}

impl JustifyContent {
    /// Reads one content-distribution flag; `None` marks values without a
    /// model (`stretch`, baselines, overflow positions ride the initial).
    fn from_flag(flag: AlignFlags) -> Option<Self> {
        Some(match flag.value() {
            AlignFlags::FLEX_START | AlignFlags::START | AlignFlags::LEFT => Self::FlexStart,
            AlignFlags::FLEX_END | AlignFlags::END | AlignFlags::RIGHT => Self::FlexEnd,
            AlignFlags::CENTER => Self::Center,
            AlignFlags::SPACE_BETWEEN => Self::SpaceBetween,
            AlignFlags::SPACE_AROUND => Self::SpaceAround,
            AlignFlags::SPACE_EVENLY => Self::SpaceEvenly,
            _ => return None,
        })
    }
}

impl AlignItems {
    /// Reads one item-placement flag.
    fn from_flag(flag: AlignFlags) -> Option<Self> {
        Some(match flag.value() {
            AlignFlags::NORMAL | AlignFlags::STRETCH => Self::Stretch,
            AlignFlags::FLEX_START
            | AlignFlags::START
            | AlignFlags::SELF_START
            | AlignFlags::LEFT => Self::FlexStart,
            AlignFlags::FLEX_END | AlignFlags::END | AlignFlags::SELF_END | AlignFlags::RIGHT => {
                Self::FlexEnd
            }
            AlignFlags::CENTER => Self::Center,
            AlignFlags::BASELINE | AlignFlags::LAST_BASELINE => Self::Baseline,
            _ => return None,
        })
    }
}

impl AlignSelf {
    /// Reads one self-alignment flag.
    fn from_flag(flag: AlignFlags) -> Option<Self> {
        Some(match flag.value() {
            AlignFlags::AUTO => Self::Auto,
            AlignFlags::NORMAL | AlignFlags::STRETCH => Self::Stretch,
            AlignFlags::FLEX_START
            | AlignFlags::START
            | AlignFlags::SELF_START
            | AlignFlags::LEFT => Self::FlexStart,
            AlignFlags::FLEX_END | AlignFlags::END | AlignFlags::SELF_END | AlignFlags::RIGHT => {
                Self::FlexEnd
            }
            AlignFlags::CENTER => Self::Center,
            AlignFlags::BASELINE | AlignFlags::LAST_BASELINE => Self::Baseline,
            _ => return None,
        })
    }
}
