use std::sync::mpsc;
use std::time::Duration;

use rquickjs::loader::{ImportAttributes, Loader, Resolver};
use rquickjs::module::Declared;
use rquickjs::{Ctx, Error, Module, Result};
use url::Url;

use crate::protocol::{DialKind, DialRequest};

pub(super) struct WebModuleResolver;

impl Resolver for WebModuleResolver {
    fn resolve<'js>(
        &mut self,
        _ctx: &Ctx<'js>,
        base: &str,
        name: &str,
        _attributes: Option<ImportAttributes<'js>>,
    ) -> Result<String> {
        let base = Url::parse(base).map_err(|_| Error::new_resolving(base, name))?;
        let resolved = Url::parse(name)
            .or_else(|_| base.join(name))
            .map_err(|_| Error::new_resolving(base.as_str(), name))?;
        match resolved.scheme() {
            "http" | "https" => Ok(resolved.into()),
            _ => Err(Error::new_resolving(base.as_str(), name)),
        }
    }
}

pub(super) struct WebModuleLoader;

impl Loader for WebModuleLoader {
    fn load<'js>(
        &mut self,
        ctx: &Ctx<'js>,
        name: &str,
        attributes: Option<ImportAttributes<'js>>,
    ) -> Result<Module<'js, Declared>> {
        if attributes.is_some() {
            return Err(Error::new_loading(name));
        }
        load_module(ctx, name)
    }
}

pub(super) fn load_module<'js>(ctx: &Ctx<'js>, name: &str) -> Result<Module<'js, Declared>> {
    // A JavaScript module graph is fetched before evaluation
    // (<https://html.spec.whatwg.org/multipage/webappapis.html#fetch-a-single-module-script>).
    // QuickJS's host loader is synchronous, so the renderer waits here while
    // the browser process performs the network operation on its I/O runtime.
    let world = super::bindings::world(ctx)?;
    let world = world.borrow();
    let services = world.runtime.services.clone();
    let initiator = world.document_url.to_string();
    drop(world);

    let (send, receive) = mpsc::sync_channel(1);
    services.start_dial(
        DialRequest {
            kind: DialKind::ModuleScript,
            url: name.to_owned(),
            initiator,
            read_body: true,
        },
        Box::new(move |outcome| {
            let _result = send.send(outcome);
        }),
    );
    let outcome = receive
        .recv_timeout(Duration::from_secs(30))
        .map_err(|_| Error::new_loading(name))?
        .map_err(|_| Error::new_loading(name))?;
    if !(200..300).contains(&outcome.status)
        || !super::javascript_module_mime(outcome.content_type.as_deref())
    {
        return Err(Error::new_loading(name));
    }
    Module::declare(ctx.clone(), name, outcome.body)
}
