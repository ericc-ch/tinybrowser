//! Mutation records, observers, and the mutation delivery queue.

use super::{
    CollectionKind, child_value, live_collection, option_truthy, required_node, string_value,
    webidl_to_string, world, wrap_node,
};

use dom::NodeId;

use rquickjs::{
    Class, Ctx, Exception, Function, Object, Persistent, Result, Value, class::Trace, prelude::This,
};

use crate::js::world::{Handle, Observation, ObserverOptions, ObserverState, RecordData};

/// `MutationRecord` (<https://dom.spec.whatwg.org/#interface-mutationrecord>).
#[derive(Trace, rquickjs::JsLifetime)]
#[rquickjs::class(rename = "MutationRecord")]
pub struct JsMutationRecord {
    pub(crate) record: RecordData,
}

#[rquickjs::methods]
#[allow(
    clippy::needless_pass_by_value,
    clippy::unused_self,
    reason = "rquickjs method ABI passes Ctx by value"
)]
impl JsMutationRecord {
    #[qjs(constructor)]
    fn ctor(ctx: Ctx<'_>) -> Result<Self> {
        Err(Exception::throw_type(&ctx, "Illegal constructor"))
    }

    #[qjs(get, rename = "type")]
    fn record_type(&self) -> String {
        self.record.typ.clone()
    }

    #[qjs(get)]
    fn target<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        wrap_node(&ctx, self.record.target.0)
    }

    #[qjs(get, rename = "addedNodes")]
    fn added_nodes<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        node_list(&ctx, self.record.target.0, &self.record.added)
    }

    #[qjs(get, rename = "removedNodes")]
    fn removed_nodes<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        node_list(&ctx, self.record.target.0, &self.record.removed)
    }

    #[qjs(get, rename = "previousSibling")]
    fn previous_sibling<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        child_value(&ctx, self.record.previous.map(|handle| handle.0))
    }

    #[qjs(get, rename = "nextSibling")]
    fn next_sibling<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        child_value(&ctx, self.record.next.map(|handle| handle.0))
    }

    #[qjs(get, rename = "attributeName")]
    fn attribute_name<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        match &self.record.attribute_name {
            Some(name) => string_value(&ctx, name),
            None => Ok(Value::new_null(ctx)),
        }
    }

    #[qjs(get, rename = "attributeNamespace")]
    fn attribute_namespace<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        match &self.record.attribute_namespace {
            Some(namespace) => string_value(&ctx, namespace),
            None => Ok(Value::new_null(ctx)),
        }
    }

    #[qjs(get, rename = "oldValue")]
    fn old_value<'js>(&self, ctx: Ctx<'js>) -> Result<Value<'js>> {
        match &self.record.old_value {
            Some(value) => string_value(&ctx, value),
            None => Ok(Value::new_null(ctx)),
        }
    }
}

fn node_list<'js>(ctx: &Ctx<'js>, scope: NodeId, nodes: &[Handle]) -> Result<Value<'js>> {
    live_collection(ctx, scope, CollectionKind::Static(nodes.to_vec()), None)
}

/// `MutationObserver` (<https://dom.spec.whatwg.org/#interface-mutationobserver>).
#[derive(Trace, rquickjs::JsLifetime)]
#[rquickjs::class(rename = "MutationObserver")]
pub struct JsMutationObserver {
    pub(crate) id: u64,
}

#[rquickjs::methods]
#[allow(
    clippy::needless_pass_by_value,
    clippy::unused_self,
    reason = "rquickjs method ABI passes Ctx by value"
)]
impl JsMutationObserver {
    // https://dom.spec.whatwg.org/#dom-mutationobserver-mutationobserver
    #[qjs(constructor)]
    fn new<'js>(ctx: Ctx<'js>, callback: Function<'js>) -> Result<Self> {
        let world_rc = world(&ctx)?;
        let mut world = world_rc.borrow_mut();
        world.next_observer_id += 1;
        let id = world.next_observer_id;
        world.observers.insert(
            id,
            ObserverState {
                callback: Persistent::save(&ctx, callback),
                object: None,
                observations: Vec::new(),
                queue: Vec::new(),
            },
        );
        Ok(Self { id })
    }

    // https://dom.spec.whatwg.org/#dom-mutationobserver-observe
    #[qjs(rename = "observe")]
    fn observe<'js>(
        &self,
        ctx: Ctx<'js>,
        this: This<Object<'js>>,
        target: Value<'js>,
        options: Object<'js>,
    ) -> Result<()> {
        let target = required_node(&ctx, &target)?;
        // Dictionary presence ignores explicit `undefined`: `{attributes:
        // undefined}` behaves as absent
        // (<https://webidl.spec.whatwg.org/#es-dictionary>).
        let attributes_present = !options.get::<_, Value>("attributes")?.is_undefined();
        let attributes = option_truthy(&ctx, &options, "attributes")?;
        let attribute_old_value_present =
            !options.get::<_, Value>("attributeOldValue")?.is_undefined();
        let attribute_old_value = option_truthy(&ctx, &options, "attributeOldValue")?;
        let character_data_present = !options.get::<_, Value>("characterData")?.is_undefined();
        let character_data = option_truthy(&ctx, &options, "characterData")?;
        let character_data_old_value_present =
            !options.get::<_, Value>("characterDataOldValue")?.is_undefined();
        let character_data_old_value = option_truthy(&ctx, &options, "characterDataOldValue")?;
        let attribute_filter = match options.get::<_, Value>("attributeFilter") {
            // `sequence<DOMString>` is not nullable: explicit `null` throws
            // instead of vanishing
            // (<https://dom.spec.whatwg.org/#dom-mutationobserver-observe>).
            Ok(value) if value.is_null() => {
                return Err(Exception::throw_type(
                    &ctx,
                    "attributeFilter must be a sequence",
                ));
            }
            Ok(value) if !value.is_undefined() => {
                let array = value.into_array().ok_or_else(|| {
                    Exception::throw_type(&ctx, "attributeFilter must be a sequence")
                })?;
                let mut filter = Vec::new();
                for entry in array.iter::<Value>() {
                    filter.push(webidl_to_string(&ctx, entry?)?);
                }
                Some(filter)
            }
            _ => None,
        };
        // Present-but-false option flags conflict with their companions
        // (<https://dom.spec.whatwg.org/#dom-mutationobserver-observe>).
        if attributes_present
            && !attributes
            && (attribute_old_value_present || attribute_filter.is_some())
        {
            return Err(Exception::throw_type(
                &ctx,
                "attributes is false but attributeOldValue/attributeFilter is present",
            ));
        }
        if character_data_present && !character_data && character_data_old_value_present {
            return Err(Exception::throw_type(
                &ctx,
                "characterData is false but characterDataOldValue is present",
            ));
        }
        let parsed = ObserverOptions {
            child_list: option_truthy(&ctx, &options, "childList")?,
            attributes: attributes
                || (!attributes_present
                    && (attribute_old_value_present || attribute_filter.is_some())),
            character_data: character_data
                || (!character_data_present && character_data_old_value_present),
            subtree: option_truthy(&ctx, &options, "subtree")?,
            attribute_old_value,
            character_data_old_value,
            attribute_filter,
        };
        if !(parsed.child_list || parsed.attributes || parsed.character_data) {
            return Err(Exception::throw_type(
                &ctx,
                "options must set childList, attributes, or characterData",
            ));
        }
        let world_rc = world(&ctx)?;
        let mut world = world_rc.borrow_mut();
        // Records already in the log belong to observers registered before
        // this call; this observer's stream starts at registration. The spec
        // queues records only to already-registered observers, so the defer
        // has to close the gap here.
        world.drain_mutations();
        let Some(observer) = world.observers.get_mut(&self.id) else {
            return Ok(());
        };
        // One registration per (observer, target): a repeated observe
        // replaces the options instead of adding a second registration
        // (<https://dom.spec.whatwg.org/#dom-mutationobserver-observe>).
        if let Some(existing) = observer
            .observations
            .iter_mut()
            .find(|observation| observation.target.0 == target)
        {
            existing.options = parsed;
        } else {
            observer.observations.push(Observation {
                target: Handle(target),
                options: parsed,
            });
        }
        observer.object = Some(Persistent::save(&ctx, this.0));
        world.set_recording(true);
        Ok(())
    }

    // https://dom.spec.whatwg.org/#dom-mutationobserver-disconnect
    #[qjs(rename = "disconnect")]
    fn disconnect(&self, ctx: Ctx<'_>) -> Result<()> {
        let world_rc = world(&ctx)?;
        let mut world = world_rc.borrow_mut();
        if let Some(observer) = world.observers.get_mut(&self.id) {
            observer.observations.clear();
            observer.queue.clear();
            observer.object = None;
        }
        // Recording costs nothing while nobody has a registration.
        if world
            .observers
            .values()
            .all(|observer| observer.observations.is_empty())
        {
            world.set_recording(false);
        }
        Ok(())
    }

    // https://dom.spec.whatwg.org/#dom-mutationobserver-takerecords
    #[qjs(rename = "takeRecords")]
    fn take_records<'js>(&self, ctx: Ctx<'js>) -> Result<Vec<Value<'js>>> {
        let world_rc = world(&ctx)?;
        let queue = {
            let mut world = world_rc.borrow_mut();
            world.drain_mutations();
            world.take_observer_queue(self.id)
        };
        queue
            .into_iter()
            .map(|record| {
                Ok(Class::instance(ctx.clone(), JsMutationRecord { record })?.into_value())
            })
            .collect()
    }
}

/// Delivers queued records to observer callbacks; installed as a global and
/// scheduled as a microtask after every mutation
/// (<https://dom.spec.whatwg.org/#notify-mutation-observers>).
#[allow(
    clippy::needless_pass_by_value,
    reason = "rquickjs Func ABI passes Ctx by value"
)]
pub(crate) fn deliver_mutations(ctx: Ctx<'_>) -> Result<()> {
    let result = deliver_ready(&ctx);
    // Release the schedule slot even when a callback threw; otherwise a
    // throwing observer would silently stop every later delivery.
    world(&ctx)?.borrow_mut().delivery_scheduled = false;
    result
}

fn deliver_ready(ctx: &Ctx<'_>) -> Result<()> {
    let world_rc = world(ctx)?;
    // A callback can mutate again and queue more records; keep draining until
    // nothing is left.
    let mut first_error = None;
    loop {
        let ready = {
            let mut world = world_rc.borrow_mut();
            world.drain_mutations();
            world.take_ready()
        };
        if ready.is_empty() {
            break;
        }
        for observer in ready {
            // One observer's broken wrapper must not discard the records
            // already dequeued for the rest: setup failures skip that
            // observer and are reported at the end with callback errors.
            let step: Result<()> = (|| {
                let array = rquickjs::Array::new(ctx.clone())?;
                for (index, record) in observer.records.into_iter().enumerate() {
                    array.set(
                        index,
                        Class::instance(ctx.clone(), JsMutationRecord { record })?.into_value(),
                    )?;
                }
                let object = observer.object.restore(ctx)?;
                let callback = observer.callback.restore(ctx)?;
                // The callback's `this` value and second argument are the
                // observer object. A throwing callback is reported and does not
                // stop the remaining observers.
                if let Err(error) = callback.call::<_, ()>((This(object.clone()), array, object))
                    && first_error.is_none()
                {
                    first_error = Some(error);
                }
                Ok(())
            })();
            if let Err(error) = step
                && first_error.is_none()
            {
                first_error = Some(error);
            }
        }
    }
    match first_error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

/// Schedules one microtask that delivers queued records, if needed.
///
/// Matching runs here, not at delivery time: a mutation's observer scope is
/// decided by the tree shape it happened in, and a moved or newly attached
/// node must not retroactively pull an earlier mutation into a different
/// observer (`<https://dom.spec.whatwg.org/#queue-a-mutation-record>` picks
/// interested observers when the mutation is queued).
pub(crate) fn schedule_mutation_delivery(ctx: &Ctx<'_>) -> Result<()> {
    let world_rc = world(ctx)?;
    let mut world = world_rc.borrow_mut();
    world.drain_mutations();
    if world.observers.is_empty() || world.delivery_scheduled {
        return Ok(());
    }
    world.delivery_scheduled = true;
    // The pristine entry points, captured at install: a page that deleted
    // `__tb_deliver_mutations` or `queueMicrotask` must not turn every DOM
    // mutation into an exception.
    let deliver = world.deliver_mutations_fn.clone();
    let queue = world.pristine_queue_microtask.clone();
    drop(world);
    if let (Some(deliver), Some(queue)) = (deliver, queue) {
        let deliver: Function = deliver.restore(ctx)?;
        let queue: Function = queue.restore(ctx)?;
        queue.call::<_, ()>((deliver,))?;
        return Ok(());
    }
    let deliver: Function = ctx.globals().get("__tb_deliver_mutations")?;
    let queue: Function = ctx.globals().get("queueMicrotask")?;
    queue.call::<_, ()>((deliver,))?;
    Ok(())
}
