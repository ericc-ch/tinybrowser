//! `DOMException` payload and behavior. Its JS surface lives in Web IDL.

use rquickjs::{Ctx, Result, String, class::Trace};

/// <https://webidl.spec.whatwg.org/#idl-DOMException>
#[derive(Trace, rquickjs::JsLifetime)]
pub struct JsDomException<'js> {
    pub(crate) name: String<'js>,
    pub(crate) message: String<'js>,
}

impl<'js> JsDomException<'js> {
    // https://webidl.spec.whatwg.org/#dom-domexception-domexception
    #[allow(
        clippy::unnecessary_wraps,
        reason = "generated constructors share one fallible call shape; this payload cannot fail"
    )]
    fn new(_ctx: &Ctx<'js>, message: String<'js>, name: String<'js>) -> Result<Self> {
        Ok(Self { name, message })
    }

    // https://webidl.spec.whatwg.org/#dom-domexception-code
    fn get_code(&self, _ctx: &Ctx<'js>) -> Result<u16> {
        dom_exception_generated::legacy_code(&self.name)
    }
}

include!(concat!(env!("OUT_DIR"), "/DOMException.rs"));
