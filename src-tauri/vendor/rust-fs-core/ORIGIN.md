# rust-fs-core

Upstream <https://github.com/antimatter-studios/rust-fs-core>, MIT,
version 0.3.14 / tag `v0.3.14`, commit
`19449ad4926d09447d19440c6312ba544b50bf4b`.
This is the sibling version pinned by rust-fs-bcachefs's `chores.yml`.

Source and MIT license are retained; local packaging sets `autobins = false`,
`autoexamples = false`, `crate-type = ["rlib"]` and a standalone workspace.
No framework implementation is changed. Upstream integration/CI scripts are
not part of this vendor subset. The application uses only the positioned
`BlockRead` trait and its error type, through a bounded immutable evidence
adapter; it does not enable this crate's `cli` feature or use host callbacks.
No C filesystem library or mount implementation is linked.
