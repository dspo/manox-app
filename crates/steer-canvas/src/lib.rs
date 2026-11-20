//! The canvas model: a container owns a canvas, a canvas is a layout tree, and
//! the tree's leaves are sheets.
//!
//! The geometry layer under the window — the sheet host, the tab strips and the
//! gestures are a separate crate's job (PLAN-ABSORB-04). What lives here is the
//! data those surfaces read and write:
//!
//! | module | what it is | gpui |
//! |---|---|---|
//! | [`id`] | sheet identity (`kind:instance`) and id decoding | no |
//! | [`layout`] | the layout tree and its pure transforms | no |
//! | [`state`] | per-container canvas state: shown, parked, tickets, order | no |
//! | [`persist`] | `<steer_home>/layouts/{id}.json` read/write | no |
//! | [`registry`] | the sheet-kind seam (titles, restore, body factory) | yes |
//!
//! `gpui` enters only through [`registry`] (its bodies are `AnyView`
//! factories), behind the default-on `registry` feature. Every other module —
//! and the whole crate with `default-features = false` — is gpui-free, because
//! the state model is also what a headless assembly and the next harness's
//! sheets consume.
//!
//! Three rules the model is built around:
//!
//! 1. **A sheet exists because its kind is registered** — the layout only says
//!    where it is placed. Removing a node never ends a resource; closing a
//!    sheet is a separate edit.
//! 2. **Layout writes have one writer** ([`CanvasState::set_layout`]), and every
//!    intent carries a ticket taken before the intent formed. A late commit is
//!    dropped, not merged — the geometry-layer counterpart of the chrome
//!    boundary's effect-queue rule.
//! 3. **No state lives in a render component.** A kind's state lives in its own
//!    store; the registry hands out view factories only.

pub mod id;
pub mod layout;
pub mod persist;
#[cfg(feature = "registry")]
pub mod registry;
pub mod state;

pub use id::{
    TerminalResource, home_sheet_id, session_of_sheet, session_sheet_id, sheet_container,
    sheet_kind, terminal_resource, terminal_sheet_id,
};
pub use layout::{Axis, DockLayout, DockNode};
#[cfg(feature = "registry")]
pub use registry::{ParsedSheet, SheetKindSpec, SheetRegistry};
pub use state::{
    CanvasSnapshot, CanvasState, HeldLayouts, PARKED_CANVASES, SCRATCH_CONTAINER, SetLayoutOutcome,
    SwitchOutcome,
};
