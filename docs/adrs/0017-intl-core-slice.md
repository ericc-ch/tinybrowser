# Locale-filtered Intl core slice

tinybrowser ships a bounded first `Intl` slice backed by ICU4X 2.3 and a
generated locale-filtered data blob. JavaScript owns ECMA-402 option processing,
internal slots, prototype behavior, and bound formatter functions. Rust receives
only normalized value records and owns ICU4X data loading and formatting.

Status: proposed (2026-09-12). This extends the engine charter in
[ADR 0007](0007-engine-charter.md) without changing renderer ownership or adding
an event loop dependency.

## Sources

The governing algorithms are ECMA-402's
[`CanonicalizeLocaleList`](https://402.ecma-international.org/#sec-canonicalizelocalelist),
[`SupportedLocales`](https://402.ecma-international.org/#sec-supportedlocales),
[`Intl.NumberFormat`](https://402.ecma-international.org/#sec-intl.numberformat),
[`SetNumberFormatDigitOptions`](https://402.ecma-international.org/#sec-setnfdigitoptions),
and
[`CreateDateTimeFormat`](https://402.ecma-international.org/#sec-createdatetimeformat).

Firefox is the implementation reference. SpiderMonkey gathers and validates
options into compact internal records before constructing its native formatter
([NumberFormat initialization](https://searchfox.org/firefox-main/source/js/src/builtin/intl/NumberFormat.cpp#1128-1307),
[native formatter construction](https://searchfox.org/firefox-main/source/js/src/builtin/intl/NumberFormat.cpp#1652-1689),
[DateTimeFormat initialization](https://searchfox.org/firefox-main/source/js/src/builtin/intl/DateTimeFormat.cpp#497-855)).
It also sends `BigInt` through a distinct exact-value path rather than converting
it to a binary64 number
([Firefox `FormatBigInt`](https://searchfox.org/firefox-main/source/js/src/builtin/intl/NumberFormat.cpp#2023)).
tinybrowser follows those boundaries with QuickJS slots in the façade and
value-only calls into Rust; ICU4X replaces Firefox's native ICU wrapper.

## Merge target

The first merge target is deliberately smaller than all of ECMA-402:

- seven locales: `en-US`, `es-ES`, `de-DE`, `ja-JP`, `fr-FR`, `zh-CN`, and
  `ko-KR`, with language-only fallback to the corresponding locale;
- locale alias canonicalization, filtering, constructor/prototype descriptors,
  bound `format` functions, and `resolvedOptions()` for the implemented surface;
- decimal, percent, and currency number formatting, exact `BigInt` and ordinary
  decimal-string transport, integer/fraction/significant digit bounds, default
  currency fraction digits, grouping for decimal style, all `signDisplay`
  modes, symbol/code/name display, and accounting sign selection;
- Gregorian UTC date formatting for every `dateStyle`, short/medium time
  formatting, the default numeric component shapes, and the defaults used by
  `Date.prototype` locale methods;
- locale methods on `Number`, `BigInt`, and `Date`.

Recognized options outside that target fail at the façade where practical.
There is no claim of full `intl402` conformance. In particular, non-Latin
numbering systems, non-Gregorian calendars, non-UTC zones, long/full time styles
that require zone names, arbitrary date/time component skeletons,
NumberFormat-v3 rounding modes/increments/priorities, notation, units, parts,
and ranges are follow-up slices.

## Verification target

`tools/intl/test262-target.txt` is the reviewable conformance target. The
`tools/intl/test262` runner executes every listed upstream Test262 case through
the shipping CLI and a fresh tab. The initial target covers constructors,
locale filtering, property descriptors, bound formatter metadata, decimal,
percent and currency defaults, exact BigInt receiver/error behavior,
date/time styles, style/component conflicts, non-finite handling, option getter
order, exact large values, alias replacement, and locale prototype methods. The
initial merge target contains 106 upstream cases.

Cargo tests remain for tinybrowser-owned behavior only. ECMA-402 claims are
added to the Test262 target instead of being copied into Rust tests. Expanding
the implementation means adding the relevant upstream cases to the target and
recording the new count here and in the size checkpoint.

## Data and size

`tools/intl/generate-data` pins `icu4x-datagen` 2.3.0 and derives the exact data
marker set from the release executable. The checked-in postcard blob makes
runtime behavior independent of host locale packages. Each target expansion
must regenerate the blob when markers change and remeasure the stripped release
binary against the 10 MB ceiling in
[`docs/researches/size-budget.md`](../researches/size-budget.md).

Dynamic ICU4X field-set enums are excluded from this slice because they retain
all variants and materially weaken the binary-size argument. Supported static
field sets are selected explicitly.

## Consequences

- A fresh realm gets deterministic locale data without a system ICU dependency.
- JavaScript-visible validation stays auditable against ECMA-402, while Rust
  does not need to hold QuickJS lifetimes or web-facing option objects.
- Unsupported valid ECMA-402 inputs can still throw. This is an explicit,
  measured partial implementation rather than a full-compatibility claim.
- Adding locales or features is a data and code-size decision as well as a
  conformance decision.
