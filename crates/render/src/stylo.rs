//! One-shot styling through Stylo, Servo's style engine.
//!
//! This is the third "take parts from Blitz" slice: after Taffy (layout) and
//! Parley (text), Stylo replaces our hand-rolled property database and
//! cascade. What we take is the cascade math — selector matching, origins,
//! inheritance, computed values. What we skip is everything platform: no
//! rayon traversal (our [`dom::Dom`] is `!Sync` by design, so the traversal
//! below is a single-threaded breadth-first walk), no restyle/invalidation
//! (one screenshot styles each element exactly once), no animations, no
//! snapshots, no shadow DOM.
//!
//! Flow: parse the UA sheet and author sheets into one [`Stylist`], walk the
//! DOM once calling [`recalc_style_at`] per element, then translate each
//! [`ComputedValues`] into our layout [`Style`] in `stylo_map`. Lengths stay
//! symbolic where our model keeps them symbolic (percentages, `em`); Stylo
//! resolves font-relative units against the used fonts during the cascade,
//! which subsumes the old `font-size`-first two-phase resolution.
//!
//! `rem` follows the spec through a second pass: the first pass styles with
//! the initial 16px root size (which is exactly right for the root element's
//! own `font-size`), and when the root computes a different size the device
//! adopts it and styling re-runs, so descendants resolve `rem` against the
//! real root size. The root keeps its first-pass `font-size`, which is the
//! spec value (`rem` on `font-size` itself means the initial size).

use std::collections::{HashMap, VecDeque};

use dom::{Dom, NodeId};
use style::animation::DocumentAnimationSet;
use style::context::{
    QuirksMode, RegisteredSpeculativePainter, RegisteredSpeculativePainters,
    SharedStyleContext, StyleContext, StyleSystemOptions, ThreadLocalStyleContext,
};
use style::device::Device;
use style::dom::{TElement, TNode};
use style::dom::NodeInfo;
use style::media_queries::MediaType;
use style::properties::ComputedValues;
use style::selector_parser::SnapshotMap;
use style::servo_arc::Arc;
use style::shared_lock::StylesheetGuards;
use style::stylesheets::{AllowImportRules, CssRuleType, DocumentStyleSheet, Origin, Stylesheet, UrlExtraData};
use style::stylist::Stylist;
use style::traversal::{DomTraversal, PerLevelTraversalData, recalc_style_at};
use style::traversal_flags::TraversalFlags;
use style::Atom;

use crate::style::Style;
use crate::stylo_map::map_style;
use crate::stylo_view::{StyloElement, StyloNode, StyloTables, node_at};

/// Our UA stylesheet: the same rules every browser ships
/// (<https://html.spec.whatwg.org/#rendering>), trimmed to what we paint.
const UA_STYLESHEET: &str = r"
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

/// Styles every element of `dom` and returns our layout styles by node.
///
/// `sheets` are author stylesheets in document order; `style` attributes are
/// parsed per element. Viewport units and `@media` resolve against
/// `viewport_width` x `viewport_height`.
pub(crate) fn style_document(
    dom: &Dom,
    sheets: &[String],
    viewport_width: f32,
    viewport_height: f32,
) -> HashMap<NodeId, Style> {
    let mut tables = StyloTables::new();
    register_elements(dom, &mut tables);
    parse_attributes(dom, &mut tables);

    // Stylo gates several properties behind compile-time-default-off prefs;
    // this engine's grid support needs the grid pref before the Stylist
    // parses any sheet. `layout.unimplemented` matches Blitz's choice so the
    // cascade sees the same property set as a shipping engine.
    stylo_static_prefs::set_pref!("layout.grid.enabled", true);
    stylo_static_prefs::set_pref!("layout.unimplemented", true);

    let url = UrlExtraData(Arc::new(
        url::Url::parse("about:blank").expect("about:blank parses"),
    ));
    let mut stylist = Stylist::new(
        make_device(viewport_width, viewport_height),
        QuirksMode::NoQuirks,
    );
    append_sheet(&mut stylist, &tables, &url, UA_STYLESHEET, Origin::UserAgent);
    for sheet in sheets {
        append_sheet(&mut stylist, &tables, &url, sheet, Origin::Author);
    }
    {
        let guard = tables.lock.read();
        let guards = StylesheetGuards {
            author: &guard,
            ua_or_user: &guard,
        };
        stylist.flush(&guards);
    }

    // First pass at the initial root size; adopt the real root size and
    // re-run when `rem` could have resolved differently.
    let mut computed = run_traversal(dom, &tables, &stylist);
    let root_size = root_font_size(dom, &computed);
    if (root_size - 16.0).abs() > f32::EPSILON {
        let root_style = root_computed(dom, &computed);
        stylist.device().set_root_font_size(root_size);
        if let Some(root_style) = root_style {
            stylist.device().set_root_style(&root_style);
        }
        tables.data.clear();
        tables.dirty.borrow_mut().clear();
        register_data(dom, &mut tables);
        computed = run_traversal(dom, &tables, &stylist);
    }

    computed
        .into_iter()
        .map(|(id, values)| (id, map_style(&values)))
        .collect()
}

/// Registers every element under the document: traversal indices plus fresh
/// element data.
fn register_elements(dom: &Dom, tables: &mut StyloTables) {
    let document = dom.document();
    tables.register(document);
    for node in dom.descendants(document) {
        tables.register(node);
        if matches!(dom.kind(node), Some(dom::NodeKind::Element { .. })) {
            tables
                .data
                .insert(node, style::data::ElementDataWrapper::default());
        }
    }
}

/// Re-inserts fresh element data after a `rem` second pass clears the table.
fn register_data(dom: &Dom, tables: &mut StyloTables) {
    let document = dom.document();
    for node in dom.descendants(document) {
        if matches!(dom.kind(node), Some(dom::NodeKind::Element { .. })) {
            tables
                .data
                .insert(node, style::data::ElementDataWrapper::default());
        }
    }
}

/// Parses every element's `id` and `style` attribute into the side tables.
/// Unparseable declarations drop, like browsers drop them.
fn parse_attributes(dom: &Dom, tables: &mut StyloTables) {
    let url = UrlExtraData(Arc::new(
        url::Url::parse("about:blank").expect("about:blank parses"),
    ));
    let document = dom.document();
    for node in dom.descendants(document) {
        if !matches!(dom.kind(node), Some(dom::NodeKind::Element { .. })) {
            continue;
        }
        if let Some(id) = dom.attribute(node, "id")
            && !id.is_empty()
        {
            tables.ids.insert(node, Atom::from(id.as_str()));
        }
        let Some(value) = dom.attribute(node, "style") else {
            continue;
        };
        let block = style::properties::parse_style_attribute(
            &value,
            &url,
            None,
            QuirksMode::NoQuirks,
            CssRuleType::Style,
        );

        tables
            .style_attrs
            .insert(node, Arc::new(tables.lock.wrap(block)));
    }
}

/// Builds a Servo device for a viewport in CSS pixels at scale 1.
fn make_device(width: f32, height: f32) -> Device {
    use style_traits::{CSSPixel, DevicePixel};
    Device::new(
        MediaType::screen(),
        QuirksMode::NoQuirks,
        euclid::Size2D::<f32, CSSPixel>::new(width, height),
        euclid::Size2D::<f32, DevicePixel>::new(width, height),
        euclid::Scale::<f32, CSSPixel, DevicePixel>::new(1.0),
        Box::new(EmbeddedFontMetrics),
        ComputedValues::initial_values_with_font_override(
            style::properties::style_structs::Font::initial_values(),
        ),
        style::queries::values::PrefersColorScheme::Light,
        style::servo::media_features::PointerCapabilities::default(),
        style::servo::media_features::PointerCapabilities::default(),
    )
}

/// Parses one sheet and appends it to the stylist. `@import` is rejected:
/// the old engine fetched external stylesheets at the net layer, never
/// through the cascade.
fn append_sheet(
    stylist: &mut Stylist,
    tables: &StyloTables,
    url: &UrlExtraData,
    css: &str,
    origin: Origin,
) {
    let sheet = Stylesheet::from_str(
        css,
        url.clone(),
        origin,
        Arc::new(tables.lock.wrap(style::media_queries::MediaList::empty())),
        tables.lock.clone(),
        None,
        None,
        QuirksMode::NoQuirks,
        AllowImportRules::No,
    );
    let guard = tables.lock.read();
    stylist.append_stylesheet(DocumentStyleSheet(Arc::new(sheet)), &guard);
}

/// Runs one breadth-first styling pass, returning computed values by element.
fn run_traversal(
    dom: &Dom,
    tables: &StyloTables,
    stylist: &Stylist,
) -> HashMap<NodeId, Arc<ComputedValues>> {
    // Stylo asserts the layout thread state around traversal bookkeeping.
    style::thread_state::enter(style::thread_state::ThreadState::LAYOUT);
    let guard = tables.lock.read();
    let guards = StylesheetGuards {
        author: &guard,
        ua_or_user: &guard,
    };
    let painters = NoPainters;
    let snapshots = SnapshotMap::new();
    let animations = DocumentAnimationSet::default();
    let shared = SharedStyleContext {
        stylist,
        visited_styles_enabled: false,
        options: StyleSystemOptions::default(),
        guards,
        current_time_for_animations: 0.0,
        traversal_flags: TraversalFlags::empty(),
        snapshot_map: &snapshots,
        animations,
        registered_speculative_painters: &painters,
    };
    let traversal = SingleTraversal { shared };

    // One record per node; handles are `&StyloNode` and the records live
    // here for the whole pass.
    let nodes = StyloNode::build_all(dom, tables);
    let node_refs: &[StyloNode<'_>] = &nodes;
    for node in node_refs {
        node.fill_neighbors(node_refs);
    }

    {
        let mut thread_local = ThreadLocalStyleContext::<StyloElement<'_>>::new();
        let mut context = StyleContext {
            shared: traversal.shared_context(),
            thread_local: &mut thread_local,
        };

        let document = dom.document();
        // Depth counts element ancestors (the root element is 0): the bloom
        // filter keys off it.
        let mut queue: VecDeque<(StyloElement<'_>, usize)> = VecDeque::new();
        if let Some(kids) = dom.children(document) {
            for &kid in kids {
                if let Some(element) =
                    node_at(node_refs, tables, Some(kid)).filter(NodeInfo::is_element)
                {
                    queue.push_back((element, 0));
                }
            }
        }
        while let Some((node, depth)) = queue.pop_front() {
            let traversal_data = PerLevelTraversalData {
                current_dom_depth: depth,
            };
            traversal.process_preorder(&traversal_data, &mut context, node, |child| {
                queue.push_back((child, depth + 1));
            });
        }
        // `thread_local` drops here, while the layout state is still set.
    }

    let mut computed = HashMap::new();
    for &id in tables.data.keys() {
        let Some(node) = node_at(node_refs, tables, Some(id)) else {
            continue;
        };
        if let Some(data) = node.borrow_data()
            && let Some(primary) = data.styles.get_primary()
        {
            computed.insert(id, primary.clone());
        }
    }
    style::thread_state::exit(style::thread_state::ThreadState::LAYOUT);
    computed
}

/// The root element's computed font size, or 16px without a root.
fn root_font_size(dom: &Dom, computed: &HashMap<NodeId, Arc<ComputedValues>>) -> f32 {
    root_element(dom)
        .and_then(|root| computed.get(&root))
        .map_or(16.0, |values| values.get_font().font_size.computed_size.px())
}

/// The root element's computed values, if styled.
fn root_computed(
    dom: &Dom,
    computed: &HashMap<NodeId, Arc<ComputedValues>>,
) -> Option<Arc<ComputedValues>> {
    root_element(dom).and_then(|root| computed.get(&root).cloned())
}

/// The document's root element (`<html>` in practice).
fn root_element(dom: &Dom) -> Option<NodeId> {
    let document = dom.document();
    dom.children(document)?.find_map(|&kid| {
        matches!(dom.kind(kid), Some(dom::NodeKind::Element { .. })).then_some(kid)
    })
}

/// Our single-threaded traversal: [`recalc_style_at`] per element, children
/// discovered breadth-first like Servo's parallel driver.
struct SingleTraversal<'a> {
    shared: SharedStyleContext<'a>,
}

impl<'dom> DomTraversal<&'dom StyloNode<'dom>> for SingleTraversal<'_> {
    #[expect(
        unsafe_code,
        reason = "upstream Stylo requires unsafe data hooks; single-threaded traversal gives exclusive logical access"
    )]
    fn process_preorder<F: FnMut(&'dom StyloNode<'dom>)>(
        &self,
        traversal_data: &PerLevelTraversalData,
        context: &mut StyleContext<&'dom StyloNode<'dom>>,
        node: &'dom StyloNode<'dom>,
        note_child: F,
    ) {
        let Some(element) = node.as_element() else {
            return;
        };
        // SAFETY: single-threaded walk; no other borrow of this element's
        // pre-populated wrapper is live.
        let mut data = unsafe { element.ensure_data() };
        recalc_style_at(self, traversal_data, context, element, &mut data, note_child);
        // SAFETY: same exclusive access as above.
        unsafe { element.unset_dirty_descendants() };
    }

    fn process_postorder(
        &self,
        _context: &mut StyleContext<&'dom StyloNode<'dom>>,
        _node: &'dom StyloNode<'dom>,
    ) {
        // Never called: `needs_postorder_traversal` is false.
    }

    fn needs_postorder_traversal() -> bool {
        false
    }

    fn shared_context(&self) -> &SharedStyleContext<'_> {
        &self.shared
    }
}

/// No speculative painters exist: `paintworklet` images are not supported.
struct NoPainters;

impl RegisteredSpeculativePainters for NoPainters {
    fn get(&self, _name: &Atom) -> Option<&dyn RegisteredSpeculativePainter> {
        None
    }
}

/// Font metrics from the embedded faces: Stylo asks for these to resolve
/// font-relative units (`ex`, `ch`, `cap`, `ic`).
#[derive(Debug)]
struct EmbeddedFontMetrics;

impl style::device::servo::FontMetricsProvider for EmbeddedFontMetrics {
    fn query_font_metrics(
        &self,
        _vertical: bool,
        font: &style::properties::style_structs::Font,
        base_size: style::values::computed::CSSPixelLength,
        _flags: style::values::computed::font::QueryFontMetricsFlags,
    ) -> style::font_metrics::FontMetrics {
        use skrifa::instance::{LocationRef, NormalizedCoord};
        let bytes = if font.font_weight >= style::values::computed::font::FontWeight::BOLD {
            crate::font::BOLD_BYTES
        } else {
            crate::font::REGULAR_BYTES
        };
        let coords: &[NormalizedCoord] = &[];
        let empty = crate::font::metrics_for(bytes, base_size.px(), LocationRef::from(coords));
        let to_length = |px: Option<f32>| {
            px.filter(|px| *px != 0.0)
                .map(style::values::computed::Length::new)
        };
        style::font_metrics::FontMetrics {
            ascent: style::values::computed::Length::new(empty.ascent),
            x_height: to_length(empty.x_height),
            cap_height: to_length(empty.cap_height),
            zero_advance_measure: to_length(empty.zero_advance),
            ic_width: to_length(empty.ic_width),
            script_percent_scale_down: None,
            script_script_percent_scale_down: None,
        }
    }

    fn base_size_for_generic(
        &self,
        generic: style::values::computed::font::GenericFontFamily,
    ) -> style::values::computed::Length {
        use style::values::computed::font::GenericFontFamily as Generic;
        let px = match generic {
            Generic::Monospace => 13.0,
            _ => 16.0,
        };
        style::values::computed::Length::new(px)
    }
}
