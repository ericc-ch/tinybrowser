// https://dom.spec.whatwg.org/#interface-processinginstruction
[Exposed=Window, Rust=JsNode]
partial interface ProcessingInstruction {
    [Rust=processing_instruction_target] readonly attribute DOMString target;
};
