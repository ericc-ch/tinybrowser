use std::cell::RefCell;
use std::rc::Rc;

use fixed_decimal::{
    Decimal, FloatPrecision, Sign, SignDisplay, SignedRoundingMode, UnsignedRoundingMode,
};
use icu_calendar::Gregorian;
use icu_datetime::{
    FixedCalendarDateTimeFormatter, NoCalendarFormatter,
    fieldsets::{T, YMD, YMDE},
    input::{Date, DateTime, Time},
    options::YearStyle,
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
use icu_locale::{LocaleCanonicalizer, fallback::LocaleFallbacker};
use icu_locale_core::Locale;
use icu_provider::{DataProvider, DataRequest, buf::AsDeserializingBufferProvider};
use icu_provider_adapters::fallback::LocaleFallbackProvider;
use icu_provider_blob::BlobDataProvider;
use rquickjs::{Array, Ctx, Exception, Result, prelude::Func};
use writeable::Writeable;

type IntlProvider = LocaleFallbackProvider<BlobDataProvider>;

struct NumberFormatInput {
    locale: String,
    value: NumberFormatValue,
    style: String,
    currency: String,
    currency_display: String,
    currency_sign: String,
    minimum_integer_digits: u8,
    minimum_fraction_digits: u8,
    maximum_fraction_digits: u8,
    minimum_significant_digits: u8,
    maximum_significant_digits: u8,
    use_grouping: bool,
    sign_display: String,
}

enum NumberFormatValue {
    Number(f64),
    Exact(String),
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

// ECMA-402 defines the constructor surface and option algorithms. Firefox uses
// the same boundary we use here: SpiderMonkey validates options into internal
// slots, then creates a native formatter from that normalized record.
// https://402.ecma-international.org/#sec-intl-object
// https://searchfox.org/firefox-main/source/js/src/builtin/intl/NumberFormat.cpp#1128-1307
// https://searchfox.org/firefox-main/source/js/src/builtin/intl/NumberFormat.cpp#1652-1689
// https://searchfox.org/firefox-main/source/js/src/builtin/intl/DateTimeFormat.cpp#497-855
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
  const objectConstructor = Object;
  const objectCreate = Object.create;
  const stringConstructor = String;
  const stringToUpperCase = Function.prototype.call.bind(String.prototype.toUpperCase);
  const stringToLowerCase = Function.prototype.call.bind(String.prototype.toLowerCase);
  const stringTrim = Function.prototype.call.bind(String.prototype.trim);
  const numberConstructor = Number;
  const numberIsFinite = Number.isFinite;
  const numberMaxSafeInteger = Number.MAX_SAFE_INTEGER;
  const booleanConstructor = Boolean;
  const numberValueOf = Number.prototype.valueOf;
  const bigintValueOf = BigInt.prototype.valueOf;
  const symbolToPrimitive = Symbol.toPrimitive;
  const arrayIndexOf = Function.prototype.call.bind(Array.prototype.indexOf);
  const arrayPush = Function.prototype.call.bind(Array.prototype.push);
  const arrayConcat = Function.prototype.call.bind(Array.prototype.concat);
  const regexpTest = Function.prototype.call.bind(RegExp.prototype.test);
  const mathFloor = Math.floor;
  const mathMax = Math.max;
  const mathMin = Math.min;
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
  const numberSlotsGet = numberSlots.get.bind(numberSlots);
  const numberSlotsSet = numberSlots.set.bind(numberSlots);
  const dateTimeSlotsGet = dateTimeSlots.get.bind(dateTimeSlots);
  const dateTimeSlotsSet = dateTimeSlots.set.bind(dateTimeSlots);

  // https://402.ecma-international.org/#sec-canonicalizelocalelist
  function localeList(locales) {
    if (locales === undefined) return [];
    if (typeof locales === 'string') return [locales];
    if (locales === null) throw new TypeError('locales must not be null');
    const object = objectConstructor(locales);
    const numericLength = +object.length;
    const length = mathMin(
      mathMax(numberIsFinite(numericLength) ? mathFloor(numericLength) : 0, 0),
      numberMaxSafeInteger
    );
    const result = [];
    for (let index = 0; index < length; index += 1) {
      if (!(index in object)) continue;
      const value = object[index];
      if (value === null || (typeof value !== 'string' && typeof value !== 'object' &&
          typeof value !== 'function')) {
        throw new TypeError('locale list elements must be strings or objects');
      }
      arrayPush(result, stringConstructor(value));
    }
    return result;
  }

  function canonicalLocaleList(locales) {
    const requested = localeList(locales);
    const result = [];
    for (let index = 0; index < requested.length; index += 1) {
      const tag = requested[index];
      const canonical = nativeCanonicalLocale(stringConstructor(tag));
      if (canonical === '!') throw new RangeError('invalid language tag: ' + tag);
      if (arrayIndexOf(result, canonical) < 0) arrayPush(result, canonical);
    }
    return result;
  }

  function resolveLocale(locales) {
    const requested = canonicalLocaleList(locales);
    for (let index = 0; index < requested.length; index += 1) {
      const resolved = nativeResolveLocale(requested[index]);
      if (resolved) return resolved;
    }
    return 'en-US';
  }

  // https://402.ecma-international.org/#sec-supportedlocales
  function supportedLocales(locales, options) {
    options = optionsObject(options);
    stringOption(options, 'localeMatcher', ['lookup', 'best fit'], 'best fit');
    const result = [];
    const requested = canonicalLocaleList(locales);
    for (let index = 0; index < requested.length; index += 1) {
      const canonical = requested[index];
      if (nativeResolveLocale(canonical) && arrayIndexOf(result, canonical) < 0) {
        arrayPush(result, canonical);
      }
    }
    return result;
  }

  function optionsObject(options) {
    if (options === undefined) return objectCreate(null);
    if (options === null) throw new TypeError('options must be an object');
    return objectConstructor(options);
  }

  function stringOption(options, name, values, fallback) {
    const value = options[name];
    if (value === undefined) return fallback;
    const text = stringConstructor(value);
    if (arrayIndexOf(values, text) < 0) throw new RangeError('invalid ' + name);
    return text;
  }

  function digitOption(options, name, fallback, minimum, maximum) {
    const value = options[name];
    if (value === undefined) return fallback;
    const integer = mathFloor(+value);
    if (!numberIsFinite(integer) || integer < minimum || integer > maximum) {
      throw new RangeError('invalid ' + name);
    }
    return integer;
  }

  function primitiveNumberHint(value) {
    if (value === null || (typeof value !== 'object' && typeof value !== 'function')) {
      return value;
    }
    const exotic = value[symbolToPrimitive];
    if (exotic !== undefined) {
      if (typeof exotic !== 'function') throw new TypeError('@@toPrimitive must be callable');
      const primitive = call(exotic, value, 'number');
      if (primitive === null || (typeof primitive !== 'object' &&
          typeof primitive !== 'function')) return primitive;
      throw new TypeError('@@toPrimitive must return a primitive');
    }
    const methods = ['valueOf', 'toString'];
    for (let index = 0; index < methods.length; index += 1) {
      const method = value[methods[index]];
      if (typeof method !== 'function') continue;
      const primitive = call(method, value);
      if (primitive === null || (typeof primitive !== 'object' &&
          typeof primitive !== 'function')) return primitive;
    }
    throw new TypeError('cannot convert object to a primitive value');
  }

  // https://402.ecma-international.org/#sec-tointlmathematicalvalue
  function intlMathematicalValue(value) {
    const primitive = primitiveNumberHint(value);
    if (typeof primitive === 'bigint') {
      return { number: 0, exact: stringConstructor(primitive) };
    }
    if (typeof primitive === 'string') {
      const text = stringTrim(primitive);
      if (regexpTest(/^[+-]?(?:[0-9]+(?:\.[0-9]*)?|\.[0-9]+)$/, text)) {
        return { number: 0, exact: text };
      }
    }
    return { number: +primitive, exact: '' };
  }

  // https://402.ecma-international.org/#sec-intl.numberformat
  function NumberFormat(locales, options) {
    if (!new.target) return new NumberFormat(locales, options);
    const locale = resolveLocale(locales);
    options = optionsObject(options);
    stringOption(options, 'localeMatcher', ['lookup', 'best fit'], 'best fit');
    const numberingSystemOption = options.numberingSystem;
    let numberingSystem = 'latn';
    if (numberingSystemOption !== undefined) {
      numberingSystem = stringToLowerCase(stringConstructor(numberingSystemOption));
      if (!regexpTest(/^[a-z0-9]{3,8}(?:-[a-z0-9]{3,8})*$/, numberingSystem)) {
        throw new RangeError('invalid numberingSystem');
      }
      if (numberingSystem !== 'latn') {
        throw new RangeError('only the latn numbering system is currently supported');
      }
    }

    // ResolveOptions and SetNumberFormatUnitOptions are kept in specification
    // order so getters observe the same sequence as Firefox.
    const style = stringOption(
      options, 'style', ['decimal', 'percent', 'currency', 'unit'], 'decimal'
    );
    const currencyOption = options.currency;
    let currency;
    if (currencyOption !== undefined) {
      const currencyText = stringConstructor(currencyOption);
      if (!regexpTest(/^[A-Za-z]{3}$/, currencyText)) throw new RangeError('invalid currency');
      currency = stringToUpperCase(currencyText);
    }
    if (style === 'currency' && currency === undefined) {
      throw new TypeError('currency is required');
    }
    const currencyDisplay = stringOption(
      options, 'currencyDisplay', ['symbol', 'narrowSymbol', 'code', 'name'], 'symbol'
    );
    const currencySign = stringOption(
      options, 'currencySign', ['standard', 'accounting'], 'standard'
    );
    const unitOption = options.unit;
    const unit = unitOption === undefined ? undefined : stringConstructor(unitOption);
    if (style === 'unit' && unit === undefined) throw new TypeError('unit is required');
    stringOption(options, 'unitDisplay', ['short', 'narrow', 'long'], 'short');
    if (style === 'unit') throw new RangeError('unit formatting is not currently supported');

    const notation = stringOption(
      options, 'notation', ['standard', 'scientific', 'engineering', 'compact'], 'standard'
    );
    if (notation !== 'standard') throw new RangeError('notation is not currently supported');
    const currencyDigits = style === 'currency'
      ? nativeCurrencyDigits(currency)
      : 0;
    const defaultMaximum = style === 'percent' ? 0 : style === 'currency' ? currencyDigits : 3;
    const defaultMinimum = style === 'currency' ? defaultMaximum : 0;
    const minimumIntegerDigits = digitOption(options, 'minimumIntegerDigits', 1, 1, 21);
    const minimumFractionOption = options.minimumFractionDigits;
    const maximumFractionOption = options.maximumFractionDigits;
    const minimumSignificantOption = options.minimumSignificantDigits;
    const maximumSignificantOption = options.maximumSignificantDigits;
    const roundingIncrement = digitOption(options, 'roundingIncrement', 1, 1, 5000);
    if (roundingIncrement !== 1) {
      throw new RangeError('roundingIncrement is not currently supported');
    }
    const roundingMode = stringOption(
      options, 'roundingMode',
      ['ceil', 'floor', 'expand', 'trunc', 'halfCeil', 'halfFloor', 'halfExpand',
       'halfTrunc', 'halfEven'],
      'halfExpand'
    );
    if (roundingMode !== 'halfExpand') {
      throw new RangeError('roundingMode is not currently supported');
    }
    const roundingPriority = stringOption(
      options, 'roundingPriority', ['auto', 'morePrecision', 'lessPrecision'], 'auto'
    );
    if (roundingPriority !== 'auto') {
      throw new RangeError('roundingPriority is not currently supported');
    }
    const trailingZeroDisplay = stringOption(
      options, 'trailingZeroDisplay', ['auto', 'stripIfInteger'], 'auto'
    );
    if (trailingZeroDisplay !== 'auto') {
      throw new RangeError('trailingZeroDisplay is not currently supported');
    }

    let minimumFractionDigits = 0;
    let maximumFractionDigits = 0;
    let minimumSignificantDigits = 0;
    let maximumSignificantDigits = 0;
    if (minimumSignificantOption !== undefined || maximumSignificantOption !== undefined) {
      minimumSignificantDigits = minimumSignificantOption === undefined
        ? 1
        : digitOption(
          { minimumSignificantDigits: minimumSignificantOption },
          'minimumSignificantDigits', 1, 1, 21
        );
      maximumSignificantDigits = maximumSignificantOption === undefined
        ? 21
        : digitOption(
          { maximumSignificantDigits: maximumSignificantOption },
          'maximumSignificantDigits', 21, minimumSignificantDigits, 21
        );
    } else {
      const specifiedMinimum = minimumFractionOption === undefined
        ? undefined
        : digitOption(
          { minimumFractionDigits: minimumFractionOption },
          'minimumFractionDigits', defaultMinimum, 0, 100
        );
      const specifiedMaximum = maximumFractionOption === undefined
        ? undefined
        : digitOption(
          { maximumFractionDigits: maximumFractionOption },
          'maximumFractionDigits', defaultMaximum, 0, 100
        );
      minimumFractionDigits = specifiedMinimum === undefined
        ? specifiedMaximum === undefined ? defaultMinimum : mathMin(defaultMinimum, specifiedMaximum)
        : specifiedMinimum;
      maximumFractionDigits = specifiedMaximum === undefined
        ? mathMax(defaultMaximum, minimumFractionDigits)
        : specifiedMaximum;
      if (minimumFractionDigits > maximumFractionDigits) {
        throw new RangeError('minimumFractionDigits exceeds maximumFractionDigits');
      }
    }

    stringOption(options, 'compactDisplay', ['short', 'long'], 'short');
    const groupingOption = options.useGrouping;
    let useGrouping = true;
    if (typeof groupingOption === 'string') {
      const grouping = stringOption(
        { useGrouping: groupingOption }, 'useGrouping',
        ['min2', 'auto', 'always', 'true', 'false'], 'auto'
      );
      if (grouping === 'min2') throw new RangeError('min2 grouping is not currently supported');
      useGrouping = grouping !== 'false';
    } else if (groupingOption !== undefined) {
      useGrouping = booleanConstructor(groupingOption);
    }
    if (style !== 'decimal' && !useGrouping) {
      throw new RangeError('disabling grouping is currently supported only for decimal style');
    }
    const signDisplay = stringOption(
      options, 'signDisplay', ['auto', 'never', 'always', 'exceptZero', 'negative'], 'auto'
    );
    // SetNumberFormatUnitOptions stores [[Currency]] only for currency style,
    // so resolvedOptions must not expose the currency fields for other styles.
    // https://402.ecma-international.org/#sec-setnumberformatunitoptions
    const slots = {
      locale, numberingSystem, style,
      currency: style === 'currency' ? currency : undefined,
      currencyDisplay, currencySign,
      minimumIntegerDigits, minimumFractionDigits, maximumFractionDigits,
      minimumSignificantDigits, maximumSignificantDigits,
      useGrouping, signDisplay, boundFormat: null,
    };
    numberSlotsSet(this, slots);
  }
  Object.defineProperty(NumberFormat, 'length', { value: 0 });

  Object.defineProperty(NumberFormat.prototype, 'format', {
    configurable: true,
    get: function() {
      const slots = numberSlotsGet(this);
      if (!slots) throw new TypeError('incompatible NumberFormat receiver');
      if (!slots.boundFormat) {
        slots.boundFormat = value => {
          const mathematical = intlMathematicalValue(value);
          return nativeFormatNumber([
            slots.locale,
            mathematical.number,
            mathematical.exact,
            slots.style,
            slots.currency || '',
            slots.currencyDisplay,
            slots.currencySign,
            slots.minimumIntegerDigits,
            slots.minimumFractionDigits,
            slots.maximumFractionDigits,
            slots.minimumSignificantDigits,
            slots.maximumSignificantDigits,
            slots.useGrouping,
            slots.signDisplay
          ]);
        };
      }
      return slots.boundFormat;
    },
  });
  Object.defineProperty(
    Object.getOwnPropertyDescriptor(NumberFormat.prototype, 'format').get,
    'name',
    { value: 'get format' }
  );

  Object.defineProperty(NumberFormat.prototype, 'resolvedOptions', {
    value: function resolvedOptions() {
      const slots = numberSlotsGet(this);
      if (!slots) throw new TypeError('incompatible NumberFormat receiver');
      const result = {
        locale: slots.locale,
        numberingSystem: slots.numberingSystem,
        style: slots.style,
      };
      if (slots.currency) {
        result.currency = slots.currency;
        result.currencyDisplay = slots.currencyDisplay;
        result.currencySign = slots.currencySign;
      }
      result.minimumIntegerDigits = slots.minimumIntegerDigits;
      if (slots.maximumSignificantDigits) {
        result.minimumSignificantDigits = slots.minimumSignificantDigits;
        result.maximumSignificantDigits = slots.maximumSignificantDigits;
      } else {
        result.minimumFractionDigits = slots.minimumFractionDigits;
        result.maximumFractionDigits = slots.maximumFractionDigits;
      }
      result.useGrouping = slots.useGrouping ? 'auto' : false;
      result.notation = 'standard';
      result.signDisplay = slots.signDisplay;
      result.roundingIncrement = 1;
      result.roundingMode = 'halfExpand';
      result.roundingPriority = 'auto';
      result.trailingZeroDisplay = 'auto';
      return result;
    },
    writable: true,
    configurable: true,
  });
  Object.defineProperty(NumberFormat, 'supportedLocalesOf', {
    value: function supportedLocalesOf(locales) {
      return supportedLocales(locales, arguments[1]);
    },
    writable: true,
    configurable: true,
  });
  Object.defineProperty(NumberFormat.prototype, Symbol.toStringTag, {
    value: 'Intl.NumberFormat',
    configurable: true,
  });
  Object.defineProperty(NumberFormat, 'prototype', { writable: false });

  const styleCode = { short: 1, medium: 2, long: 3, full: 4 };
  const dateComponentNames = ['weekday', 'era', 'year', 'month', 'day'];
  const timeComponentNames = [
    'dayPeriod', 'hour', 'minute', 'second', 'fractionalSecondDigits', 'timeZoneName'
  ];
  const allComponentNames = arrayConcat(dateComponentNames, timeComponentNames);

  // https://402.ecma-international.org/#sec-todatetimeoptions
  function toDateTimeOptions(options) {
    return objectCreate(options === undefined ? null : optionsObject(options));
  }

  function datePatternCode(components) {
    const hasYear = components.year !== undefined;
    const hasMonth = components.month !== undefined;
    const hasDay = components.day !== undefined;
    const hasWeekday = components.weekday !== undefined;
    if (components.era !== undefined) {
      throw new RangeError('era formatting is not currently supported');
    }
    if (hasYear && hasMonth && hasDay && !hasWeekday && components.year === 'numeric' &&
        components.month === 'numeric' &&
        components.day === 'numeric') return 5;
    if (!hasYear && !hasMonth && !hasDay && !hasWeekday) return 0;
    throw new RangeError('this date component combination is not currently supported');
  }

  // ECMA-402 gathers the component and style options before matching a locale pattern.
  // https://402.ecma-international.org/#sec-createdatetimeformat
  function initializeDateTimeFormat(instance, locales, options, required, defaults) {
    const locale = resolveLocale(locales);
    options = toDateTimeOptions(options);
    stringOption(options, 'localeMatcher', ['lookup', 'best fit'], 'best fit');
    const calendarOption = options.calendar;
    if (calendarOption !== undefined &&
        stringToLowerCase(stringConstructor(calendarOption)) !== 'gregory') {
      throw new RangeError('only the gregory calendar is currently supported');
    }
    const numberingSystemOption = options.numberingSystem;
    if (numberingSystemOption !== undefined &&
        stringToLowerCase(stringConstructor(numberingSystemOption)) !== 'latn') {
      throw new RangeError('only the latn numbering system is currently supported');
    }
    const hour12Option = options.hour12;
    if (hour12Option !== undefined) booleanConstructor(hour12Option);
    const hourCycleOption = stringOption(
      options, 'hourCycle', ['h11', 'h12', 'h23', 'h24'], undefined
    );
    if (hour12Option !== undefined || hourCycleOption !== undefined) {
      throw new RangeError('hour cycle overrides are not currently supported');
    }
    const timeZoneOption = options.timeZone;
    const timeZone = timeZoneOption === undefined ? 'UTC' : stringConstructor(timeZoneOption);
    if (stringToUpperCase(timeZone) !== 'UTC') {
      throw new RangeError('only UTC is currently supported');
    }

    const components = {
      weekday: stringOption(options, 'weekday', ['narrow', 'short', 'long'], undefined),
      era: stringOption(options, 'era', ['narrow', 'short', 'long'], undefined),
      year: stringOption(options, 'year', ['2-digit', 'numeric'], undefined),
      month: stringOption(
        options, 'month', ['2-digit', 'numeric', 'narrow', 'short', 'long'], undefined
      ),
      day: stringOption(options, 'day', ['2-digit', 'numeric'], undefined),
      dayPeriod: stringOption(options, 'dayPeriod', ['narrow', 'short', 'long'], undefined),
      hour: stringOption(options, 'hour', ['2-digit', 'numeric'], undefined),
      minute: stringOption(options, 'minute', ['2-digit', 'numeric'], undefined),
      second: stringOption(options, 'second', ['2-digit', 'numeric'], undefined),
      fractionalSecondDigits: options.fractionalSecondDigits === undefined
        ? undefined
        : digitOption(options, 'fractionalSecondDigits', undefined, 1, 3),
      timeZoneName: stringOption(
        options, 'timeZoneName',
        ['short', 'long', 'shortOffset', 'longOffset', 'shortGeneric', 'longGeneric'],
        undefined
      ),
    };
    stringOption(options, 'formatMatcher', ['basic', 'best fit'], 'best fit');
    const dateStyle = stringOption(
      options, 'dateStyle', ['full', 'long', 'medium', 'short'], undefined
    );
    const timeStyle = stringOption(
      options, 'timeStyle', ['full', 'long', 'medium', 'short'], undefined
    );
    if (dateStyle !== undefined || timeStyle !== undefined) {
      for (let index = 0; index < allComponentNames.length; index += 1) {
        const name = allComponentNames[index];
        if (components[name] !== undefined) {
          throw new TypeError('dateStyle/timeStyle cannot be combined with components');
        }
      }
      if (required === 'date' && timeStyle !== undefined) {
        throw new TypeError('timeStyle is not valid for date-only formatting');
      }
      if (required === 'time' && dateStyle !== undefined) {
        throw new TypeError('dateStyle is not valid for time-only formatting');
      }
    }

    let hasTimeComponents = false;
    let hasDateComponents = false;
    for (let index = 0; index < dateComponentNames.length; index += 1) {
      if (components[dateComponentNames[index]] !== undefined) hasDateComponents = true;
    }
    for (let index = 0; index < timeComponentNames.length; index += 1) {
      if (components[timeComponentNames[index]] !== undefined) hasTimeComponents = true;
    }
    // CreateDateTimeFormat lets only the component group named by `required`
    // veto the defaults, then applies the groups named by `defaults`.
    // https://402.ecma-international.org/#sec-createdatetimeformat
    const needDefaults = dateStyle === undefined && timeStyle === undefined &&
      (required === 'date'
        ? !hasDateComponents
        : required === 'time'
          ? !hasTimeComponents
          : !hasDateComponents && !hasTimeComponents);
    if (needDefaults) {
      if (defaults === 'date' || defaults === 'all') {
        components.year = components.month = components.day = 'numeric';
      }
      if (defaults === 'time' || defaults === 'all') {
        components.hour = components.minute = components.second = 'numeric';
        hasTimeComponents = true;
      }
    }
    if (hasTimeComponents && (components.hour === undefined || components.minute === undefined)) {
      throw new RangeError('time formatting currently requires hour and minute');
    }
    if (components.dayPeriod !== undefined || components.fractionalSecondDigits !== undefined ||
        components.timeZoneName !== undefined) {
      throw new RangeError('this time component is not currently supported');
    }
    const dateCode = dateStyle === undefined
      ? datePatternCode(components)
      : styleCode[dateStyle];
    const timeCode = timeStyle === undefined
      ? hasTimeComponents ? (components.second === undefined ? 1 : 2) : 0
      : styleCode[timeStyle];
    const usesHour = timeCode !== 0;
    const hour12 = locale === 'en-US' || locale === 'ko-KR';
    dateTimeSlotsSet(instance, {
      locale, dateStyle, timeStyle, timeZone: 'UTC', components,
      dateCode, timeCode, usesHour, hour12, boundFormat: null,
    });
  }

  // https://402.ecma-international.org/#sec-intl.datetimeformat
  function DateTimeFormat(locales, options) {
    if (!new.target) return new DateTimeFormat(locales, options);
    initializeDateTimeFormat(this, locales, options, 'any', 'date');
  }
  Object.defineProperty(DateTimeFormat, 'length', { value: 0 });

  Object.defineProperty(DateTimeFormat.prototype, 'format', {
    configurable: true,
    get: function() {
      const slots = dateTimeSlotsGet(this);
      if (!slots) throw new TypeError('incompatible DateTimeFormat receiver');
      if (!slots.boundFormat) {
        slots.boundFormat = value => {
          if (slots.timeStyle === 'long' || slots.timeStyle === 'full') {
            throw new RangeError('long time styles require unsupported time zone names');
          }
          const date = new dateConstructor(
            value === undefined ? dateNow() : +value
          );
          if (!numberIsFinite(call(dateValueOf, date))) throw new RangeError('invalid time value');
          return nativeFormatDateTime([
            slots.locale,
            call(dateGetUTCFullYear, date), call(dateGetUTCMonth, date) + 1,
            call(dateGetUTCDate, date), call(dateGetUTCHours, date),
            call(dateGetUTCMinutes, date), call(dateGetUTCSeconds, date),
            slots.dateCode, slots.timeCode
          ]);
        };
      }
      return slots.boundFormat;
    },
  });
  Object.defineProperty(
    Object.getOwnPropertyDescriptor(DateTimeFormat.prototype, 'format').get,
    'name',
    { value: 'get format' }
  );

  Object.defineProperty(DateTimeFormat.prototype, 'resolvedOptions', {
    value: function resolvedOptions() {
      const slots = dateTimeSlotsGet(this);
      if (!slots) throw new TypeError('incompatible DateTimeFormat receiver');
      const result = {
        locale: slots.locale,
        calendar: 'gregory',
        numberingSystem: 'latn',
        timeZone: slots.timeZone,
      };
      if (slots.usesHour) {
        result.hourCycle = slots.hour12 ? 'h12' : 'h23';
        result.hour12 = slots.hour12;
      }
      if (slots.dateStyle) result.dateStyle = slots.dateStyle;
      if (slots.timeStyle) result.timeStyle = slots.timeStyle;
      if (!slots.dateStyle && !slots.timeStyle) {
        for (let index = 0; index < allComponentNames.length; index += 1) {
          const name = allComponentNames[index];
          if (slots.components[name] !== undefined) result[name] = slots.components[name];
        }
      }
      return result;
    },
    writable: true,
    configurable: true,
  });
  Object.defineProperty(DateTimeFormat, 'supportedLocalesOf', {
    value: function supportedLocalesOf(locales) {
      return supportedLocales(locales, arguments[1]);
    },
    writable: true,
    configurable: true,
  });
  Object.defineProperty(DateTimeFormat.prototype, Symbol.toStringTag, {
    value: 'Intl.DateTimeFormat',
    configurable: true,
  });
  Object.defineProperty(DateTimeFormat, 'prototype', { writable: false });

  const IntlObject = {};
  Object.defineProperties(IntlObject, {
    NumberFormat: { value: NumberFormat, writable: true, configurable: true },
    DateTimeFormat: { value: DateTimeFormat, writable: true, configurable: true },
    getCanonicalLocales: {
      // https://402.ecma-international.org/#sec-intl.getcanonicallocales
      value: function getCanonicalLocales(locales) {
        return canonicalLocaleList(locales);
      },
      writable: true,
      configurable: true,
    },
  });
  Object.defineProperty(IntlObject, Symbol.toStringTag, {
    value: 'Intl', configurable: true,
  });
  Object.defineProperty(globalThis, 'Intl', {
    value: IntlObject,
    writable: true,
    configurable: true,
  });

  // https://402.ecma-international.org/#sup-number.prototype.tolocalestring
  const numberLocaleMethods = {
    toLocaleString() {
      const value = call(numberValueOf, this);
      return new NumberFormat(arguments[0], arguments[1]).format(value);
    },
  };
  // Firefox likewise keeps BigInt as an exact mathematical value until the
  // backend boundary instead of coercing it through Number.
  // https://searchfox.org/firefox-main/source/js/src/builtin/intl/NumberFormat.cpp#2023
  const bigintLocaleMethods = {
    toLocaleString() {
      const value = call(bigintValueOf, this);
      return new NumberFormat(arguments[0], arguments[1]).format(value);
    },
  };
  // https://402.ecma-international.org/#sup-date.prototype.tolocalestring
  const dateLocaleMethods = {
    toLocaleString() {
      const value = call(dateValueOf, this);
      if (!numberIsFinite(value)) return 'Invalid Date';
      const formatter = objectCreate(DateTimeFormat.prototype);
      initializeDateTimeFormat(formatter, arguments[0], arguments[1], 'any', 'all');
      return formatter.format(value);
    },
    toLocaleDateString() {
      const value = call(dateValueOf, this);
      if (!numberIsFinite(value)) return 'Invalid Date';
      const formatter = objectCreate(DateTimeFormat.prototype);
      initializeDateTimeFormat(formatter, arguments[0], arguments[1], 'date', 'date');
      return formatter.format(value);
    },
    toLocaleTimeString() {
      const value = call(dateValueOf, this);
      if (!numberIsFinite(value)) return 'Invalid Date';
      const formatter = objectCreate(DateTimeFormat.prototype);
      initializeDateTimeFormat(formatter, arguments[0], arguments[1], 'time', 'time');
      return formatter.format(value);
    },
  };
  Object.defineProperty(Number.prototype, 'toLocaleString', {
    value: numberLocaleMethods.toLocaleString, writable: true, configurable: true,
  });
  Object.defineProperty(BigInt.prototype, 'toLocaleString', {
    value: bigintLocaleMethods.toLocaleString, writable: true, configurable: true,
  });
  Object.defineProperties(Date.prototype, {
    toLocaleString: {
      value: dateLocaleMethods.toLocaleString, writable: true, configurable: true,
    },
    toLocaleDateString: {
      value: dateLocaleMethods.toLocaleDateString, writable: true, configurable: true,
    },
    toLocaleTimeString: {
      value: dateLocaleMethods.toLocaleTimeString, writable: true, configurable: true,
    },
  });

})();
";

struct IntlData {
    canonicalizer: Rc<LocaleCanonicalizer>,
    provider: Rc<IntlProvider>,
}

thread_local! {
    // `JsRealm::new` installs Intl once per realm, but the ICU payloads behind
    // canonicalization and fallback are immutable and independent of the realm.
    // Deserialize them once per renderer thread instead of once per document.
    static INTL_DATA: RefCell<Option<IntlData>> = const { RefCell::new(None) };
}

fn intl_data(ctx: &Ctx<'_>) -> Result<(Rc<LocaleCanonicalizer>, Rc<IntlProvider>)> {
    INTL_DATA.with(|cell| {
        let mut slot = cell.borrow_mut();
        if let Some(data) = slot.as_ref() {
            return Ok((Rc::clone(&data.canonicalizer), Rc::clone(&data.provider)));
        }
        let blob = BlobDataProvider::try_new_from_static_blob(ICU_DATA)
            .map_err(|err| Exception::throw_internal(ctx, &err.to_string()))?;
        let canonicalizer = Rc::new(
            LocaleCanonicalizer::try_new_common_with_buffer_provider(&blob)
                .map_err(|err| Exception::throw_internal(ctx, &err.to_string()))?,
        );
        let fallbacker = LocaleFallbacker::try_new_with_buffer_provider(&blob)
            .map_err(|err| Exception::throw_internal(ctx, &err.to_string()))?;
        let data = IntlData {
            canonicalizer,
            provider: Rc::new(LocaleFallbackProvider::new(blob, fallbacker)),
        };
        let handles = (Rc::clone(&data.canonicalizer), Rc::clone(&data.provider));
        *slot = Some(data);
        Ok(handles)
    })
}

pub(super) fn install(ctx: &Ctx<'_>) -> Result<()> {
    let (canonicalizer, provider) = intl_data(ctx)?;

    let number_provider = Rc::clone(&provider);
    let date_provider = Rc::clone(&provider);
    let currency_provider = Rc::clone(&provider);
    let locale_canonicalizer = Rc::clone(&canonicalizer);
    let globals = ctx.globals();
    globals.set(
        "__tbIntlCanonicalLocale",
        Func::from(move |tag: String| canonicalize_locale(&locale_canonicalizer, &tag)),
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

fn canonicalize_locale(canonicalizer: &LocaleCanonicalizer, tag: &str) -> String {
    let Ok(mut locale) = tag.parse::<Locale>() else {
        return "!".to_owned();
    };
    canonicalizer.canonicalize(&mut locale);
    locale.to_string()
}

fn format_number_args(ctx: &Ctx<'_>, provider: &IntlProvider, args: &Array<'_>) -> Result<String> {
    let number: f64 = args.get(1)?;
    let exact: String = args.get(2)?;
    let input = NumberFormatInput {
        locale: args.get(0)?,
        value: if exact.is_empty() {
            NumberFormatValue::Number(number)
        } else {
            NumberFormatValue::Exact(exact)
        },
        style: args.get(3)?,
        currency: args.get(4)?,
        currency_display: args.get(5)?,
        currency_sign: args.get(6)?,
        minimum_integer_digits: args.get(7)?,
        minimum_fraction_digits: args.get(8)?,
        maximum_fraction_digits: args.get(9)?,
        minimum_significant_digits: args.get(10)?,
        maximum_significant_digits: args.get(11)?,
        use_grouping: args.get(12)?,
        sign_display: args.get(13)?,
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

// ECMA-402 formats the exact mathematical value of strings parsed with the
// ECMA-262 `StringNumericLiteral` grammar, which accepts a decimal point with
// no digit on one side (`".5"`, `"1."`). `fixed_decimal` 0.7.2 requires a
// digit on both sides, so adapt the exact string at this boundary.
// https://402.ecma-international.org/#sec-tointlmathematicalvalue
// https://262.ecma-international.org/#sec-stringnumericliteral
fn parse_exact_decimal(ctx: &Ctx<'_>, value: &str) -> Result<Decimal> {
    let value = value.strip_prefix('+').unwrap_or(value);
    let mut normalized = String::with_capacity(value.len() + 1);
    match value.strip_prefix("-.") {
        Some(fraction) => {
            normalized.push_str("-0.");
            normalized.push_str(fraction);
        }
        None => match value.strip_prefix('.') {
            Some(fraction) => {
                normalized.push_str("0.");
                normalized.push_str(fraction);
            }
            None => normalized.push_str(value),
        },
    }
    if normalized.ends_with('.') {
        normalized.pop();
    }
    normalized
        .parse::<Decimal>()
        .map_err(|err| Exception::throw_internal(ctx, &err.to_string()))
}

fn format_number(
    ctx: &Ctx<'_>,
    provider: &IntlProvider,
    input: &NumberFormatInput,
) -> Result<String> {
    let locale = input
        .locale
        .parse::<Locale>()
        .map_err(|err| Exception::throw_internal(ctx, &err.to_string()))?;
    let mut decimal = match &input.value {
        NumberFormatValue::Number(value) if !value.is_finite() => {
            return format_nonfinite(ctx, provider, input, locale, *value);
        }
        NumberFormatValue::Number(value) => {
            Decimal::try_from_f64(*value, FloatPrecision::RoundTrip)
                .map_err(|err| Exception::throw_internal(ctx, &err.to_string()))?
        }
        NumberFormatValue::Exact(value) => parse_exact_decimal(ctx, value)?,
    };
    prepare_decimal(input, &mut decimal);
    format_prepared_decimal(ctx, provider, input, locale, &decimal)
}

fn prepare_decimal(input: &NumberFormatInput, decimal: &mut Decimal) {
    if input.style == "percent" {
        decimal.multiply_pow10(2);
        decimal.trim_start();
    }
    if input.maximum_significant_digits == 0 {
        let position = -i16::from(input.maximum_fraction_digits);
        decimal.round_with_mode(
            position,
            SignedRoundingMode::Unsigned(UnsignedRoundingMode::HalfExpand),
        );
        decimal.trim_end();
        decimal.pad_end(-i16::from(input.minimum_fraction_digits));
    } else {
        let position =
            decimal.nonzero_magnitude_start() - i16::from(input.maximum_significant_digits) + 1;
        decimal.round_with_mode(
            position,
            SignedRoundingMode::Unsigned(UnsignedRoundingMode::HalfExpand),
        );
        decimal.trim_end();
        let minimum_position =
            decimal.nonzero_magnitude_start() - i16::from(input.minimum_significant_digits) + 1;
        decimal.pad_end(minimum_position);
    }
    decimal.pad_start(i16::from(input.minimum_integer_digits));
    let display = match input.sign_display.as_str() {
        "never" => SignDisplay::Never,
        "always" => SignDisplay::Always,
        "exceptZero" => SignDisplay::ExceptZero,
        "negative" => SignDisplay::Negative,
        _ => SignDisplay::Auto,
    };
    decimal.apply_sign_display(display);
}

fn format_prepared_decimal(
    ctx: &Ctx<'_>,
    provider: &IntlProvider,
    input: &NumberFormatInput,
    locale: Locale,
    decimal: &Decimal,
) -> Result<String> {
    match input.style.as_str() {
        "currency" => format_currency(
            ctx,
            provider,
            locale,
            decimal,
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
            Ok(formatter.format(decimal).write_to_string().into_owned())
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
            Ok(formatter.format(decimal).write_to_string().into_owned())
        }
    }
}

fn format_nonfinite(
    ctx: &Ctx<'_>,
    provider: &IntlProvider,
    input: &NumberFormatInput,
    locale: Locale,
    value: f64,
) -> Result<String> {
    let mut placeholder = Decimal::from(0);
    if value.is_sign_negative() {
        placeholder.set_sign(Sign::Negative);
    }
    prepare_decimal(input, &mut placeholder);
    let formatted = format_prepared_decimal(ctx, provider, input, locale.clone(), &placeholder)?;

    let mut unsigned = placeholder;
    unsigned.set_sign(Sign::None);
    let number_formatter = DecimalFormatter::try_new_with_buffer_provider(
        provider,
        locale.into(),
        DecimalFormatterOptions::from(GroupingStrategy::Never),
    )
    .map_err(|err| Exception::throw_internal(ctx, &err.to_string()))?;
    let digits = number_formatter
        .format(&unsigned)
        .write_to_string()
        .into_owned();
    let replacement = if value.is_nan() { "NaN" } else { "∞" };
    if !formatted.contains(&digits) {
        return Err(Exception::throw_internal(
            ctx,
            "ICU4X non-finite placeholder did not contain its decimal digits",
        ));
    }
    Ok(formatted.replacen(&digits, replacement, 1))
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

    match (input.date_style, input.time_style) {
        (0, time_style) => format_time(ctx, provider, locale, time, time_style),
        (date_style, 0) => format_date(ctx, provider, locale, date, date_style),
        (date_style, time_style) => {
            format_combined(ctx, provider, locale, &datetime, date_style, time_style)
        }
    }
}

fn format_time(
    ctx: &Ctx<'_>,
    provider: &IntlProvider,
    locale: Locale,
    time: Time,
    style: u8,
) -> Result<String> {
    let output = if style == 1 {
        NoCalendarFormatter::try_new_with_buffer_provider(provider, locale.into(), T::hm())
            .map(|formatter| formatter.format(&time).write_to_string().into_owned())
    } else {
        NoCalendarFormatter::try_new_with_buffer_provider(provider, locale.into(), T::hms())
            .map(|formatter| formatter.format(&time).write_to_string().into_owned())
    }
    .map_err(|err| Exception::throw_internal(ctx, &err.to_string()))?;
    Ok(output)
}

fn format_date(
    ctx: &Ctx<'_>,
    provider: &IntlProvider,
    locale: Locale,
    date: Date<Gregorian>,
    style: u8,
) -> Result<String> {
    let output = match style {
        1 => FixedCalendarDateTimeFormatter::<Gregorian, _>::try_new_with_buffer_provider(
            provider,
            locale.into(),
            YMD::short(),
        )
        .map(|formatter| formatter.format(&date).write_to_string().into_owned()),
        2 => FixedCalendarDateTimeFormatter::<Gregorian, _>::try_new_with_buffer_provider(
            provider,
            locale.into(),
            YMD::medium(),
        )
        .map(|formatter| formatter.format(&date).write_to_string().into_owned()),
        3 => FixedCalendarDateTimeFormatter::<Gregorian, _>::try_new_with_buffer_provider(
            provider,
            locale.into(),
            YMD::long(),
        )
        .map(|formatter| formatter.format(&date).write_to_string().into_owned()),
        5 => FixedCalendarDateTimeFormatter::<Gregorian, _>::try_new_with_buffer_provider(
            provider,
            locale.into(),
            YMD::short().with_year_style(YearStyle::Full),
        )
        .map(|formatter| formatter.format(&date).write_to_string().into_owned()),
        _ => FixedCalendarDateTimeFormatter::<Gregorian, _>::try_new_with_buffer_provider(
            provider,
            locale.into(),
            YMDE::long(),
        )
        .map(|formatter| formatter.format(&date).write_to_string().into_owned()),
    }
    .map_err(|err| Exception::throw_internal(ctx, &err.to_string()))?;
    Ok(output)
}

fn format_combined(
    ctx: &Ctx<'_>,
    provider: &IntlProvider,
    locale: Locale,
    datetime: &DateTime<Gregorian>,
    date_style: u8,
    time_style: u8,
) -> Result<String> {
    let output = match (date_style, time_style) {
        (5, 1) => FixedCalendarDateTimeFormatter::<Gregorian, _>::try_new_with_buffer_provider(
            provider,
            locale.into(),
            YMD::short().with_year_style(YearStyle::Full).with_time_hm(),
        )
        .map(|formatter| formatter.format(datetime).write_to_string().into_owned()),
        (5, _) => FixedCalendarDateTimeFormatter::<Gregorian, _>::try_new_with_buffer_provider(
            provider,
            locale.into(),
            YMD::short()
                .with_year_style(YearStyle::Full)
                .with_time_hms(),
        )
        .map(|formatter| formatter.format(datetime).write_to_string().into_owned()),
        (4..=u8::MAX, 1) => {
            FixedCalendarDateTimeFormatter::<Gregorian, _>::try_new_with_buffer_provider(
                provider,
                locale.into(),
                YMDE::long().with_time_hm(),
            )
            .map(|formatter| formatter.format(datetime).write_to_string().into_owned())
        }
        (4..=u8::MAX, _) => {
            FixedCalendarDateTimeFormatter::<Gregorian, _>::try_new_with_buffer_provider(
                provider,
                locale.into(),
                YMDE::long().with_time_hms(),
            )
            .map(|formatter| formatter.format(datetime).write_to_string().into_owned())
        }
        (3, 1) => FixedCalendarDateTimeFormatter::<Gregorian, _>::try_new_with_buffer_provider(
            provider,
            locale.into(),
            YMD::long().with_time_hm(),
        )
        .map(|formatter| formatter.format(datetime).write_to_string().into_owned()),
        (3, _) => FixedCalendarDateTimeFormatter::<Gregorian, _>::try_new_with_buffer_provider(
            provider,
            locale.into(),
            YMD::long().with_time_hms(),
        )
        .map(|formatter| formatter.format(datetime).write_to_string().into_owned()),
        (2, 1) => FixedCalendarDateTimeFormatter::<Gregorian, _>::try_new_with_buffer_provider(
            provider,
            locale.into(),
            YMD::medium().with_time_hm(),
        )
        .map(|formatter| formatter.format(datetime).write_to_string().into_owned()),
        (2, _) => FixedCalendarDateTimeFormatter::<Gregorian, _>::try_new_with_buffer_provider(
            provider,
            locale.into(),
            YMD::medium().with_time_hms(),
        )
        .map(|formatter| formatter.format(datetime).write_to_string().into_owned()),
        (1, 1) => FixedCalendarDateTimeFormatter::<Gregorian, _>::try_new_with_buffer_provider(
            provider,
            locale.into(),
            YMD::short().with_time_hm(),
        )
        .map(|formatter| formatter.format(datetime).write_to_string().into_owned()),
        _ => FixedCalendarDateTimeFormatter::<Gregorian, _>::try_new_with_buffer_provider(
            provider,
            locale.into(),
            YMD::short().with_time_hms(),
        )
        .map(|formatter| formatter.format(datetime).write_to_string().into_owned()),
    }
    .map_err(|err| Exception::throw_internal(ctx, &err.to_string()))?;
    Ok(output)
}
