(function() {
  // String compilation is a CSP surface
  // (<https://w3c.github.io/webappsec-csp/#can-compile-strings>): wrap
  // `eval` and the `Function` constructor so an enforced policy without
  // `'unsafe-eval'` (or a matching hash) throws `EvalError` and reports.
  const nativeEval = globalThis.eval;
  const nativeFunction = globalThis.Function;

  function denied() {
    return new EvalError(
      "Refused to evaluate a string as JavaScript because 'unsafe-eval' is " +
      'not an allowed source of script in the following Content Security ' +
      "Policy directive: \"script-src 'none'\"");
  }

  function cspEval(source) {
    // `eval` of a non-string returns the argument unchanged.
    if (typeof source !== 'string') {
      return source;
    }
    if (!host.__tbCspAllowsEval(source)) {
      throw denied();
    }
    return __tbApply(nativeEval, undefined, [source]);
  }

  function cspFunction(...args) {
    const body = args.length > 0 ? __tbStringCtor(args[args.length - 1]) : '';
    if (!host.__tbCspAllowsEval(body)) {
      throw denied();
    }
    return __tbConstruct(nativeFunction, args);
  }
  __tbDefineProperty(cspFunction, 'prototype', {
    value: nativeFunction.prototype,
    writable: false,
    enumerable: false,
    configurable: false,
  });
  globalThis.eval = cspEval;
  globalThis.Function = cspFunction;
})();
