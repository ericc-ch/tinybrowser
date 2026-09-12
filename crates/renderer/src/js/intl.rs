use std::rc::Rc;

use fixed_decimal::{Decimal, FloatPrecision, SignedRoundingMode, UnsignedRoundingMode};
use icu_calendar::Gregorian;
use icu_datetime::{
    FixedCalendarDateTimeFormatter, NoCalendarFormatter,
    fieldsets::{T, YMD, YMDE},
    input::{Date, DateTime, Time},
};
use icu_decimal::{
    DecimalFormatter,
    options::{DecimalFormatterOptions, GroupingStrategy},
};
use icu_experimental::dimension::{
    currency::{
        CurrencyType,
        formatter::{CurrencyFormatter, CurrencyFormatterPreferences},
        options::{CurrencyFormatterOptions, CurrencyUsage},
    },
    percent::{
        formatter::{PercentFormatter, PercentFormatterPreferences},
        options::PercentFormatterOptions,
    },
    provider::currency::fractions::CurrencyFractionsV1,
};
use icu_locale::fallback::LocaleFallbacker;
use icu_locale_core::Locale;
use icu_provider::{DataProvider, DataRequest, buf::AsDeserializingBufferProvider};
use icu_provider_adapters::fallback::LocaleFallbackProvider;
use icu_provider_blob::BlobDataProvider;
use rquickjs::{Array, Ctx, Exception, Result, prelude::Func};
use writeable::Writeable;

type IntlProvider = LocaleFallbackProvider<BlobDataProvider>;

struct NumberFormatInput {
    locale: String,
    value: f64,
    style: String,
    currency: String,
    currency_display: String,
    currency_sign: String,
    minimum_integer_digits: u8,
    minimum_fraction_digits: u8,
    maximum_fraction_digits: u8,
    use_grouping: bool,
}

struct DateTimeFormatInput {
    locale: String,
    year: i32,
    month: u8,
    day: u8,
    hour: u8,
    minute: u8,
    second: u8,
    date_style: u8,
    time_style: u8,
}

const ICU_DATA: &[u8] = include_bytes!("intl_data.postcard");

// ECMA-402 constructor and prototype surface:
// https://402.ecma-international.org/#sec-intl-object
const INSTALL_INTL_JS: &str = r"
(function() {
  const nativeCanonicalLocale = globalThis.__tbIntlCanonicalLocale;
  const nativeResolveLocale = globalThis.__tbIntlResolveLocale;
  const nativeCurrencyDigits = globalThis.__tbIntlCurrencyDigits;
  const nativeFormatNumber = globalThis.__tbIntlFormatNumber;
  const nativeFormatDateTime = globalThis.__tbIntlFormatDateTime;
  delete globalThis.__tbIntlCanonicalLocale;
  delete globalThis.__tbIntlResolveLocale;
  delete globalThis.__tbIntlCurrencyDigits;
  delete globalThis.__tbIntlFormatNumber;
  delete globalThis.__tbIntlFormatDateTime;

  const call = Function.prototype.call.bind(Function.prototype.call);
  const arrayFrom = Array.from.bind(Array);
  const objectConstructor = Object;
  const objectAssign = Object.assign;
  const stringConstructor = String;
  const numberConstructor = Number;
  const numberIsFinite = Number.isFinite;
  const numberValueOf = Number.prototype.valueOf;
  const mathFloor = Math.floor;
  const dateConstructor = Date;
  const dateNow = Date.now;
  const dateValueOf = Date.prototype.valueOf;
  const dateGetUTCFullYear = Date.prototype.getUTCFullYear;
  const dateGetUTCMonth = Date.prototype.getUTCMonth;
  const dateGetUTCDate = Date.prototype.getUTCDate;
  const dateGetUTCHours = Date.prototype.getUTCHours;
  const dateGetUTCMinutes = Date.prototype.getUTCMinutes;
  const dateGetUTCSeconds = Date.prototype.getUTCSeconds;

  const numberSlots = new WeakMap();
  const dateTimeSlots = new WeakMap();

  // https://402.ecma-international.org/#sec-canonicalizelocalelist
  function localeList(locales) {
    if (locales === undefined) return [];
    if (typeof locales === 'string') return [locales];
    return arrayFrom(locales, stringConstructor);
  }

  function resolveLocale(locales) {
    const requested = localeList(locales);
    for (const tag of requested) {
      const canonical = nativeCanonicalLocale(stringConstructor(tag));
      if (canonical === '!') throw new RangeError('invalid language tag: ' + tag);
      const resolved = nativeResolveLocale(canonical);
      if (resolved) return resolved;
    }
    return 'en-US';
  }

  // https://402.ecma-international.org/#sec-supportedlocales
  function supportedLocales(locales, options) {
    options = optionsObject(options);
    stringOption(options, 'localeMatcher', ['lookup', 'best fit'], 'best fit');
    const result = [];
    for (const tag of localeList(locales)) {
      const canonical = nativeCanonicalLocale(stringConstructor(tag));
      if (canonical === '!') throw new RangeError('invalid language tag: ' + tag);
      if (nativeResolveLocale(canonical) && result.indexOf(canonical) < 0) {
        result.push(canonical);
      }
    }
    return result;
  }

  function optionsObject(options) {
    if (options === undefined) return {};
    if (options === null) throw new TypeError('options must be an object');
    return objectConstructor(options);
  }

  function stringOption(options, name, values, fallback) {
    const value = options[name];
    if (value === undefined) return fallback;
    const text = stringConstructor(value);
    if (values.indexOf(text) < 0) throw new RangeError('invalid ' + name);
    return text;
  }

  function digitOption(options, name, fallback, minimum, maximum) {
    const value = options[name];
    if (value === undefined) return fallback;
    const integer = mathFloor(numberConstructor(value));
    if (!numberIsFinite(integer) || integer < minimum || integer > maximum) {
      throw new RangeError('invalid ' + name);
    }
    return integer;
  }

  // https://402.ecma-international.org/#sec-intl.numberformat
  function NumberFormat(locales, options) {
    if (!new.target) return new NumberFormat(locales, options);
    const locale = resolveLocale(locales);
    options = optionsObject(options);
    const style = stringOption(options, 'style', ['decimal', 'percent', 'currency'], 'decimal');
    let currency;
    let currencyDisplay = 'symbol';
    let currencySign = 'standard';
    if (style === 'currency') {
      const currencyOption = options.currency;
      if (currencyOption === undefined) throw new TypeError('currency is required');
      currency = stringConstructor(currencyOption).toUpperCase();
      if (!/^[A-Z]{3}$/.test(currency)) throw new RangeError('invalid currency');
      currencyDisplay = stringOption(
        options, 'currencyDisplay', ['symbol', 'narrowSymbol', 'code', 'name'], 'symbol'
      );
      currencySign = stringOption(options, 'currencySign', ['standard', 'accounting'], 'standard');
    }
    if (style === 'currency' &&
        (options.minimumFractionDigits !== undefined || options.maximumFractionDigits !== undefined)) {
      throw new RangeError('currency fraction digit overrides are not currently supported');
    }
    const currencyDigits = style === 'currency'
      ? nativeCurrencyDigits(currency)
      : 0;
    const defaultMaximum = style === 'percent' ? 0 : style === 'currency' ? currencyDigits : 3;
    const defaultMinimum = style === 'currency' ? defaultMaximum : 0;
    const minimumIntegerDigits = digitOption(options, 'minimumIntegerDigits', 1, 1, 21);
    const minimumFractionDigits = digitOption(
      options, 'minimumFractionDigits', defaultMinimum, 0, 100
    );
    const maximumFractionDigits = digitOption(
      options, 'maximumFractionDigits', Math.max(defaultMaximum, minimumFractionDigits), 0, 100
    );
    if (minimumFractionDigits > maximumFractionDigits) {
      throw new RangeError('minimumFractionDigits exceeds maximumFractionDigits');
    }
    const useGrouping = options.useGrouping === undefined ? true : Boolean(options.useGrouping);
    const slots = {
      locale, style, currency, currencyDisplay, currencySign,
      minimumIntegerDigits, minimumFractionDigits, maximumFractionDigits,
      useGrouping, boundFormat: null,
    };
    numberSlots.set(this, slots);
  }
  Object.defineProperty(NumberFormat, 'length', { value: 0 });

  Object.defineProperty(NumberFormat.prototype, 'format', {
    configurable: true,
    get: function() {
      const slots = numberSlots.get(this);
      if (!slots) throw new TypeError('incompatible NumberFormat receiver');
      if (!slots.boundFormat) {
        slots.boundFormat = value => nativeFormatNumber([
          slots.locale,
          numberConstructor(value),
          slots.style,
          slots.currency || '',
          slots.currencyDisplay,
          slots.currencySign,
          slots.minimumIntegerDigits,
          slots.minimumFractionDigits,
          slots.maximumFractionDigits,
          slots.useGrouping
        ]);
      }
      return slots.boundFormat;
    },
  });

  NumberFormat.prototype.resolvedOptions = function() {
    const slots = numberSlots.get(this);
    if (!slots) throw new TypeError('incompatible NumberFormat receiver');
    const result = {
      locale: slots.locale,
      numberingSystem: 'latn',
      style: slots.style,
      minimumIntegerDigits: slots.minimumIntegerDigits,
      minimumFractionDigits: slots.minimumFractionDigits,
      maximumFractionDigits: slots.maximumFractionDigits,
      useGrouping: slots.useGrouping ? 'auto' : false,
      notation: 'standard',
      signDisplay: 'auto',
      roundingIncrement: 1,
      roundingMode: 'halfExpand',
      roundingPriority: 'auto',
      trailingZeroDisplay: 'auto',
    };
    if (slots.currency) {
      result.currency = slots.currency;
      result.currencyDisplay = slots.currencyDisplay;
      result.currencySign = slots.currencySign;
    }
    return result;
  };
  NumberFormat.supportedLocalesOf = supportedLocales;

  function dateStyle(options) {
    const explicit = stringOption(options, 'dateStyle', ['full', 'long', 'medium', 'short'], undefined);
    if (explicit !== undefined) return explicit;
    const hasDate = options.weekday !== undefined || options.year !== undefined ||
      options.month !== undefined || options.day !== undefined;
    if (!hasDate) return undefined;
    if (options.weekday !== undefined) return 'full';
    if (options.month === 'long') return 'long';
    if (options.month === 'short') return 'medium';
    return 'short';
  }

  function timeStyle(options) {
    const explicit = stringOption(options, 'timeStyle', ['full', 'long', 'medium', 'short'], undefined);
    if (explicit !== undefined) return explicit;
    return options.hour !== undefined || options.minute !== undefined || options.second !== undefined
      ? (options.second === undefined ? 'short' : 'medium')
      : undefined;
  }

  const styleCode = { short: 1, medium: 2, long: 3, full: 4 };

  // https://402.ecma-international.org/#sec-intl.datetimeformat
  function DateTimeFormat(locales, options) {
    if (!new.target) return new DateTimeFormat(locales, options);
    const locale = resolveLocale(locales);
    options = optionsObject(options);
    const timeZone = options.timeZone === undefined ? 'UTC' : stringConstructor(options.timeZone);
    if (timeZone.toUpperCase() !== 'UTC') {
      throw new RangeError('only UTC is currently supported');
    }
    let date = dateStyle(options);
    let time = timeStyle(options);
    if (date === undefined && time === undefined) date = 'short';
    dateTimeSlots.set(this, { locale, dateStyle: date, timeStyle: time, timeZone: 'UTC', boundFormat: null });
  }
  Object.defineProperty(DateTimeFormat, 'length', { value: 0 });

  Object.defineProperty(DateTimeFormat.prototype, 'format', {
    configurable: true,
    get: function() {
      const slots = dateTimeSlots.get(this);
      if (!slots) throw new TypeError('incompatible DateTimeFormat receiver');
      if (!slots.boundFormat) {
        slots.boundFormat = value => {
          const date = new dateConstructor(
            value === undefined ? dateNow() : numberConstructor(value)
          );
          if (!numberIsFinite(call(dateValueOf, date))) throw new RangeError('invalid time value');
          return nativeFormatDateTime([
            slots.locale,
            call(dateGetUTCFullYear, date), call(dateGetUTCMonth, date) + 1,
            call(dateGetUTCDate, date), call(dateGetUTCHours, date),
            call(dateGetUTCMinutes, date), call(dateGetUTCSeconds, date),
            slots.dateStyle ? styleCode[slots.dateStyle] : 0,
            slots.timeStyle ? styleCode[slots.timeStyle] : 0
          ]);
        };
      }
      return slots.boundFormat;
    },
  });

  DateTimeFormat.prototype.resolvedOptions = function() {
    const slots = dateTimeSlots.get(this);
    if (!slots) throw new TypeError('incompatible DateTimeFormat receiver');
    const result = {
      locale: slots.locale,
      calendar: 'gregory',
      numberingSystem: 'latn',
      timeZone: slots.timeZone,
    };
    if (slots.dateStyle) result.dateStyle = slots.dateStyle;
    if (slots.timeStyle) result.timeStyle = slots.timeStyle;
    return result;
  };
  DateTimeFormat.supportedLocalesOf = supportedLocales;

  const IntlObject = {};
  Object.defineProperties(IntlObject, {
    NumberFormat: { value: NumberFormat, writable: true, configurable: true },
    DateTimeFormat: { value: DateTimeFormat, writable: true, configurable: true },
    getCanonicalLocales: {
      // https://402.ecma-international.org/#sec-intl.getcanonicallocales
      value: function(locales) {
        const result = [];
        for (const tag of localeList(locales)) {
          const canonical = nativeCanonicalLocale(stringConstructor(tag));
          if (canonical === '!') throw new RangeError('invalid language tag: ' + tag);
          if (result.indexOf(canonical) < 0) result.push(canonical);
        }
        return result;
      },
      writable: true,
      configurable: true,
    },
  });
  Object.defineProperty(globalThis, 'Intl', {
    value: IntlObject,
    writable: true,
    configurable: true,
  });

  // https://402.ecma-international.org/#sup-number.prototype.tolocalestring
  Number.prototype.toLocaleString = function(locales, options) {
    return new NumberFormat(locales, options).format(call(numberValueOf, this));
  };
  // https://402.ecma-international.org/#sup-date.prototype.tolocalestring
  Date.prototype.toLocaleString = function(locales, options) {
    const value = call(dateValueOf, this);
    if (!numberIsFinite(value)) return 'Invalid Date';
    const merged = objectAssign(
      { dateStyle: 'short', timeStyle: 'medium' },
      options === undefined ? {} : optionsObject(options)
    );
    return new DateTimeFormat(locales, merged).format(value);
  };
  Date.prototype.toLocaleDateString = function(locales, options) {
    const value = call(dateValueOf, this);
    if (!numberIsFinite(value)) return 'Invalid Date';
    const merged = objectAssign(
      { dateStyle: 'short' }, options === undefined ? {} : optionsObject(options)
    );
    return new DateTimeFormat(locales, merged).format(value);
  };
  Date.prototype.toLocaleTimeString = function(locales, options) {
    const value = call(dateValueOf, this);
    if (!numberIsFinite(value)) return 'Invalid Date';
    const merged = objectAssign(
      { timeStyle: 'medium' }, options === undefined ? {} : optionsObject(options)
    );
    return new DateTimeFormat(locales, merged).format(value);
  };

})();
";

pub(super) fn install(ctx: &Ctx<'_>) -> Result<()> {
    let blob = BlobDataProvider::try_new_from_static_blob(ICU_DATA)
        .map_err(|err| Exception::throw_internal(ctx, &err.to_string()))?;
    let fallbacker = LocaleFallbacker::try_new_with_buffer_provider(&blob)
        .map_err(|err| Exception::throw_internal(ctx, &err.to_string()))?;
    let provider = Rc::new(LocaleFallbackProvider::new(blob, fallbacker));

    let number_provider = Rc::clone(&provider);
    let date_provider = Rc::clone(&provider);
    let currency_provider = Rc::clone(&provider);
    let globals = ctx.globals();
    globals.set(
        "__tbIntlCanonicalLocale",
        Func::from(|tag: String| canonicalize_locale(&tag)),
    )?;
    globals.set(
        "__tbIntlResolveLocale",
        Func::from(|tag: String| resolve_locale(&tag)),
    )?;
    globals.set(
        "__tbIntlCurrencyDigits",
        Func::from(move |ctx: Ctx<'_>, currency: String| {
            currency_digits(&ctx, &currency_provider, &currency)
        }),
    )?;
    globals.set(
        "__tbIntlFormatNumber",
        Func::from(move |ctx: Ctx<'_>, args: Array<'_>| {
            format_number_args(&ctx, &number_provider, &args)
        }),
    )?;
    globals.set(
        "__tbIntlFormatDateTime",
        Func::from(move |ctx: Ctx<'_>, args: Array<'_>| {
            format_date_time_args(&ctx, &date_provider, &args)
        }),
    )?;
    ctx.eval::<(), _>(INSTALL_INTL_JS)
}

fn resolve_locale(tag: &str) -> String {
    let Ok(locale) = tag.parse::<Locale>() else {
        return "!".to_owned();
    };
    match locale.id.language.as_str() {
        "en" => "en-US",
        "es" => "es-ES",
        "de" => "de-DE",
        "ja" => "ja-JP",
        "fr" => "fr-FR",
        "zh" => "zh-CN",
        "ko" => "ko-KR",
        _ => "",
    }
    .to_owned()
}

fn canonicalize_locale(tag: &str) -> String {
    tag.parse::<Locale>()
        .map_or_else(|_| "!".to_owned(), |locale| locale.to_string())
}

fn format_number_args(ctx: &Ctx<'_>, provider: &IntlProvider, args: &Array<'_>) -> Result<String> {
    let input = NumberFormatInput {
        locale: args.get(0)?,
        value: args.get(1)?,
        style: args.get(2)?,
        currency: args.get(3)?,
        currency_display: args.get(4)?,
        currency_sign: args.get(5)?,
        minimum_integer_digits: args.get(6)?,
        minimum_fraction_digits: args.get(7)?,
        maximum_fraction_digits: args.get(8)?,
        use_grouping: args.get(9)?,
    };
    format_number(ctx, provider, &input)
}

fn format_date_time_args(
    ctx: &Ctx<'_>,
    provider: &IntlProvider,
    args: &Array<'_>,
) -> Result<String> {
    let input = DateTimeFormatInput {
        locale: args.get(0)?,
        year: args.get(1)?,
        month: args.get(2)?,
        day: args.get(3)?,
        hour: args.get(4)?,
        minute: args.get(5)?,
        second: args.get(6)?,
        date_style: args.get(7)?,
        time_style: args.get(8)?,
    };
    format_date_time(ctx, provider, &input)
}

fn format_number(
    ctx: &Ctx<'_>,
    provider: &IntlProvider,
    input: &NumberFormatInput,
) -> Result<String> {
    if input.value.is_nan() {
        return Ok("NaN".to_owned());
    }
    if input.value.is_infinite() {
        return Ok(if input.value.is_sign_negative() {
            "-∞"
        } else {
            "∞"
        }
        .to_owned());
    }

    let locale = input
        .locale
        .parse::<Locale>()
        .map_err(|err| Exception::throw_internal(ctx, &err.to_string()))?;
    let mut decimal = Decimal::try_from_f64(input.value, FloatPrecision::RoundTrip)
        .map_err(|err| Exception::throw_internal(ctx, &err.to_string()))?;
    if input.style == "percent" {
        decimal.multiply_pow10(2);
        decimal.trim_start();
    }
    let position = -i16::from(input.maximum_fraction_digits);
    decimal.round_with_mode(
        position,
        SignedRoundingMode::Unsigned(UnsignedRoundingMode::HalfExpand),
    );
    decimal.trim_end();
    decimal.pad_end(-i16::from(input.minimum_fraction_digits));
    decimal.pad_start(i16::from(input.minimum_integer_digits));

    match input.style.as_str() {
        "currency" => format_currency(
            ctx,
            provider,
            locale,
            &decimal,
            &input.currency,
            &input.currency_display,
            &input.currency_sign,
        ),
        "percent" => {
            let formatter = PercentFormatter::try_new_with_buffer_provider(
                provider,
                PercentFormatterPreferences::from(locale),
                PercentFormatterOptions::default(),
            )
            .map_err(|err| Exception::throw_internal(ctx, &err.to_string()))?;
            Ok(formatter.format(&decimal).write_to_string().into_owned())
        }
        _ => {
            let grouping = if input.use_grouping {
                GroupingStrategy::Auto
            } else {
                GroupingStrategy::Never
            };
            let formatter = DecimalFormatter::try_new_with_buffer_provider(
                provider,
                locale.into(),
                DecimalFormatterOptions::from(grouping),
            )
            .map_err(|err| Exception::throw_internal(ctx, &err.to_string()))?;
            Ok(formatter.format(&decimal).write_to_string().into_owned())
        }
    }
}

fn currency_digits(ctx: &Ctx<'_>, provider: &IntlProvider, currency: &str) -> Result<u8> {
    let currency = currency
        .parse::<CurrencyType>()
        .map_err(|err| Exception::throw_range(ctx, &err.to_string()))?;
    let fractions = DataProvider::<CurrencyFractionsV1>::load(
        &provider.as_deserializing(),
        DataRequest::default(),
    )
    .map_err(|err| Exception::throw_internal(ctx, &err.to_string()))?
    .payload;
    Ok(fractions.get().resolve(currency).digits)
}

fn format_currency(
    ctx: &Ctx<'_>,
    provider: &IntlProvider,
    locale: Locale,
    decimal: &Decimal,
    currency: &str,
    display: &str,
    sign: &str,
) -> Result<String> {
    let currency = currency
        .parse::<CurrencyType>()
        .map_err(|err| Exception::throw_range(ctx, &err.to_string()))?;
    let mut options = CurrencyFormatterOptions::default();
    options.usage = if sign == "accounting" {
        CurrencyUsage::Accounting
    } else {
        CurrencyUsage::Standard
    };
    let preferences = CurrencyFormatterPreferences::from(locale);
    let formatted = match display {
        "code" => CurrencyFormatter::try_new_code_with_buffer_provider(
            provider,
            preferences,
            currency,
            options,
        )
        .map(|formatter| {
            formatter
                .format_fixed_decimal(decimal)
                .write_to_string()
                .into_owned()
        }),
        "name" => {
            CurrencyFormatter::try_new_name_with_buffer_provider(provider, preferences, currency)
                .map(|formatter| {
                    formatter
                        .format_fixed_decimal(decimal)
                        .write_to_string()
                        .into_owned()
                })
        }
        "narrowSymbol" => CurrencyFormatter::try_new_symbol_narrow_with_buffer_provider(
            provider,
            preferences,
            currency,
            options,
        )
        .map(|formatter| {
            formatter
                .format_fixed_decimal(decimal)
                .write_to_string()
                .into_owned()
        }),
        _ => CurrencyFormatter::try_new_symbol_with_buffer_provider(
            provider,
            preferences,
            currency,
            options,
        )
        .map(|formatter| {
            formatter
                .format_fixed_decimal(decimal)
                .write_to_string()
                .into_owned()
        }),
    }
    .map_err(|err| Exception::throw_internal(ctx, &err.to_string()))?;
    Ok(formatted)
}

fn format_date_time(
    ctx: &Ctx<'_>,
    provider: &IntlProvider,
    input: &DateTimeFormatInput,
) -> Result<String> {
    let locale = input
        .locale
        .parse::<Locale>()
        .map_err(|err| Exception::throw_internal(ctx, &err.to_string()))?;
    let date = Date::try_new_gregorian(input.year, input.month, input.day)
        .map_err(|err| Exception::throw_range(ctx, &err.to_string()))?;
    let time = Time::try_new(input.hour, input.minute, input.second, 0)
        .map_err(|err| Exception::throw_range(ctx, &err.to_string()))?;
    let datetime = DateTime { date, time };

    let output = match (input.date_style, input.time_style) {
        (0, 1) => {
            NoCalendarFormatter::try_new_with_buffer_provider(provider, locale.into(), T::hm())
                .map(|formatter| formatter.format(&time).write_to_string().into_owned())
        }
        (0, 2..=u8::MAX) => {
            NoCalendarFormatter::try_new_with_buffer_provider(provider, locale.into(), T::hms())
                .map(|formatter| formatter.format(&time).write_to_string().into_owned())
        }
        (1, 0) => FixedCalendarDateTimeFormatter::<Gregorian, _>::try_new_with_buffer_provider(
            provider,
            locale.into(),
            YMD::short(),
        )
        .map(|formatter| formatter.format(&date).write_to_string().into_owned()),
        (2, 0) => FixedCalendarDateTimeFormatter::<Gregorian, _>::try_new_with_buffer_provider(
            provider,
            locale.into(),
            YMD::medium(),
        )
        .map(|formatter| formatter.format(&date).write_to_string().into_owned()),
        (3, 0) => FixedCalendarDateTimeFormatter::<Gregorian, _>::try_new_with_buffer_provider(
            provider,
            locale.into(),
            YMD::long(),
        )
        .map(|formatter| formatter.format(&date).write_to_string().into_owned()),
        (4..=u8::MAX, 0) => {
            FixedCalendarDateTimeFormatter::<Gregorian, _>::try_new_with_buffer_provider(
                provider,
                locale.into(),
                YMDE::long(),
            )
            .map(|formatter| formatter.format(&date).write_to_string().into_owned())
        }
        (4..=u8::MAX, _) => {
            FixedCalendarDateTimeFormatter::<Gregorian, _>::try_new_with_buffer_provider(
                provider,
                locale.into(),
                YMDE::long().with_time_hms(),
            )
            .map(|formatter| formatter.format(&datetime).write_to_string().into_owned())
        }
        (3, _) => FixedCalendarDateTimeFormatter::<Gregorian, _>::try_new_with_buffer_provider(
            provider,
            locale.into(),
            YMD::long().with_time_hms(),
        )
        .map(|formatter| formatter.format(&datetime).write_to_string().into_owned()),
        (2, _) => FixedCalendarDateTimeFormatter::<Gregorian, _>::try_new_with_buffer_provider(
            provider,
            locale.into(),
            YMD::medium().with_time_hms(),
        )
        .map(|formatter| formatter.format(&datetime).write_to_string().into_owned()),
        _ => FixedCalendarDateTimeFormatter::<Gregorian, _>::try_new_with_buffer_provider(
            provider,
            locale.into(),
            YMD::short().with_time_hm(),
        )
        .map(|formatter| formatter.format(&datetime).write_to_string().into_owned()),
    }
    .map_err(|err| Exception::throw_internal(ctx, &err.to_string()))?;
    Ok(output)
}
