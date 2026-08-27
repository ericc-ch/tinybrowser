# 03: Template contents on Dom

Type: interview

Question: Do `<template>` contents (and other parse leftovers) live on `Dom`, or on `Parsed` beside the tree?

Answer:

On `Dom`. `HTMLTemplateElement.content` is document state. A handle-to-fragment map on `browser::Parsed` dies when you keep only the tree.

- Contents stay a `Fragment`, **not** in the `<template>` element's child list (WHATWG, already true in the sink).
- `Dom` owns the association: template `NodeId` → contents fragment `NodeId`. The sink writes through `Dom` methods. `Parsed` does not carry a second map. html5lib dump reads `Dom`.
- Parse-only flags stay in the sink. HTML integration points (`integration_points`) answer the tree builder during parse; they are not `template.content` and do not move onto `Dom`.
- Quirks mode is already on `Dom`. Leave it there.
