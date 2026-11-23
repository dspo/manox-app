//! The sheet-kind registry: the canvas model's one extension seam.
//!
//! A [`SheetKindSpec`] answers everything the canvas asks about a sheet id of
//! its kind — its title (and the tab-only title a program may rename), its
//! glyph, whether it can be closed, whether it may name its container, whether
//! its resource still exists on restore, its parsed payload, and its body.
//!
//! Registration happens at the assembly point, before the first render (v1
//! registers `home`, `manox-session` and `terminal` there; the pi sheet's kind
//! joins them in its own plan). A kind is claimed by exactly one module: a
//! second registration panics, because two owners would make `title` and `body`
//! ambiguous (fail fast rather than pick one silently).
//!
//! **The body is a view factory, never a state owner.** Switching tabs unmounts
//! a body, so whatever a kind remembers lives in its own store (the AHP store,
//! a terminal resource map, …) and is looked up by sheet id. The registry's own
//! state is the registry itself.

use std::collections::HashMap;
use std::sync::Arc;

use gpui::{AnyView, App, SharedString, Window};

use crate::id::sheet_kind;

/// A sheet's title (and the container title's input).
pub type TitleFn = Arc<dyn Fn(&str, &App) -> SharedString + Send + Sync>;
/// A sheet's tab-only title, when the kind projects one.
pub type TabTitleFn = Arc<dyn Fn(&str, &App) -> Option<SharedString> + Send + Sync>;
/// Whether the resource behind a sheet id still exists (layout restore).
pub type RestoreFn = Arc<dyn Fn(&str, &App) -> bool + Send + Sync>;
/// The kind's sole interpretation of its ids.
pub type ParseFn = Arc<dyn Fn(&str) -> ParsedSheet + Send + Sync>;
/// The sheet's body.
pub type BodyFn = Arc<dyn Fn(&str, &mut Window, &mut App) -> AnyView + Send + Sync>;

/// What a kind's `parse` hands the body: which container owns the sheet (when
/// the id encodes one) and the kind's own instance key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedSheet {
    /// The owning container, when the id names it.
    pub container_id: Option<String>,
    /// The kind's instance key (a session id, a resource index, …).
    pub instance: String,
}

impl ParsedSheet {
    /// A sheet the id places in a container.
    pub fn in_container(container_id: impl Into<String>, instance: impl Into<String>) -> Self {
        Self {
            container_id: Some(container_id.into()),
            instance: instance.into(),
        }
    }

    /// A sheet whose container the workspace resolves.
    pub fn workspace_wide(instance: impl Into<String>) -> Self {
        Self {
            container_id: None,
            instance: instance.into(),
        }
    }
}

/// One kind a module owns.
#[derive(Clone)]
pub struct SheetKindSpec {
    /// The id prefix (`home`, `session`, `terminal`).
    pub kind: &'static str,
    /// The title; also feeds the container title.
    pub title: TitleFn,
    /// The tab's own title, when the kind projects one (an agent's program
    /// title renames its tab, never its container). `None` falls back to
    /// `title`.
    pub tab_title: Option<TabTitleFn>,
    /// The tab glyph.
    pub glyph: &'static str,
    /// Whether the tab may be closed.
    pub closable: bool,
    /// Whether this sheet may name its container (an unnamed session may
    /// not).
    pub names_container: bool,
    /// Layout restore: does the resource behind the id still exist? `None`
    /// means the layout is the fact (no check).
    pub restore: Option<RestoreFn>,
    /// The kind's own id interpretation, when the body needs a payload.
    pub parse: Option<ParseFn>,
    /// The body — a view factory over state the kind's store owns.
    ///
    /// `None` means the **host renders this kind's body itself** (a view the
    /// shell already owns, like the conversation column). The kind still owns
    /// its existence, title, restore and closability — the registry is the
    /// single source for "does this sheet exist and are you allowed to show
    /// it"; only the last mile of painting is the host's.
    pub body: Option<BodyFn>,
}

/// Every registered kind, by id prefix.
#[derive(Default)]
pub struct SheetRegistry {
    kinds: HashMap<&'static str, SheetKindSpec>,
}

impl SheetRegistry {
    /// An empty registry (the assembly point registers the v1 kinds).
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers one kind.
    ///
    /// # Panics
    ///
    /// If another module already owns `spec.kind` — two owners would leave
    /// `title`/`body` ambiguous.
    pub fn register(&mut self, spec: SheetKindSpec) {
        if self.kinds.contains_key(spec.kind) {
            panic!("sheet kind `{}` is already registered", spec.kind);
        }
        self.kinds.insert(spec.kind, spec);
    }

    /// The builder form of [`SheetRegistry::register`].
    #[must_use]
    pub fn with(mut self, spec: SheetKindSpec) -> Self {
        self.register(spec);
        self
    }

    /// How many kinds are registered.
    pub fn len(&self) -> usize {
        self.kinds.len()
    }

    /// Whether no kind is registered.
    pub fn is_empty(&self) -> bool {
        self.kinds.is_empty()
    }

    /// The registered spec for a sheet id's kind.
    pub fn spec(&self, sheet_id: &str) -> Option<&SheetKindSpec> {
        self.kinds.get(sheet_kind(sheet_id))
    }

    /// Every registered kind, in no particular order.
    pub fn kinds(&self) -> impl Iterator<Item = &SheetKindSpec> {
        self.kinds.values()
    }

    /// A sheet's title, or `None` when no kind owns it.
    pub fn title(&self, sheet_id: &str, cx: &App) -> Option<SharedString> {
        let spec = self.spec(sheet_id)?;
        Some((spec.title)(sheet_id, cx))
    }

    /// A sheet's tab title: the kind's tab-only projection when it has one,
    /// else its title. `None` when no kind owns the id.
    pub fn tab_title(&self, sheet_id: &str, cx: &App) -> Option<SharedString> {
        let spec = self.spec(sheet_id)?;
        match &spec.tab_title {
            Some(tab_title) => {
                Some((tab_title)(sheet_id, cx).unwrap_or_else(|| (spec.title)(sheet_id, cx)))
            }
            None => Some((spec.title)(sheet_id, cx)),
        }
    }

    /// A sheet's glyph, when a kind owns it.
    pub fn glyph(&self, sheet_id: &str) -> Option<&'static str> {
        self.spec(sheet_id).map(|spec| spec.glyph)
    }

    /// Whether the sheet may be closed. An unowned id is closable — the canvas
    /// never strands a tab it cannot name.
    pub fn closable(&self, sheet_id: &str) -> bool {
        self.spec(sheet_id).is_none_or(|spec| spec.closable)
    }

    /// Whether the sheet may name its container. An unowned id may.
    pub fn names_container(&self, sheet_id: &str) -> bool {
        self.spec(sheet_id).is_none_or(|spec| spec.names_container)
    }

    /// The title face for [`crate::state::CanvasState::container_title`]: the
    /// sheet's title when its kind may name a container, else `None`.
    pub fn container_title_of(&self, sheet_id: &str, cx: &App) -> Option<String> {
        let spec = self.spec(sheet_id)?;
        spec.names_container
            .then(|| (spec.title)(sheet_id, cx).to_string())
    }

    /// Whether the sheet's resource still exists. A kind without a `restore`
    /// hook keeps its sheets (the layout is the fact).
    pub fn restore(&self, sheet_id: &str, cx: &App) -> bool {
        match self.spec(sheet_id).and_then(|spec| spec.restore.as_ref()) {
            Some(restore) => restore(sheet_id, cx),
            None => true,
        }
    }

    /// The kind's parsed payload, when it parses its ids.
    pub fn parse(&self, sheet_id: &str) -> Option<ParsedSheet> {
        let parse = self.spec(sheet_id)?.parse.as_ref()?;
        Some(parse(sheet_id))
    }

    /// The sheet's body, when a kind owns it *and* the host asked the registry
    /// to build it (`None` = the host paints this kind itself).
    pub fn body(&self, sheet_id: &str, window: &mut Window, cx: &mut App) -> Option<AnyView> {
        let body = self.spec(sheet_id)?.body.as_ref()?;
        Some(body(sheet_id, window, cx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(kind: &'static str) -> SheetKindSpec {
        SheetKindSpec {
            kind,
            title: Arc::new(|id, _| SharedString::from(format!("title {id}"))),
            tab_title: None,
            glyph: "document",
            closable: true,
            names_container: true,
            restore: None,
            parse: None,
            body: Some(Arc::new(|_, _, _| unreachable!("no view in a unit test"))),
        }
    }

    #[test]
    fn a_kind_is_owned_by_one_module_only() {
        let registry = SheetRegistry::new().with(spec("home"));
        assert_eq!(registry.len(), 1);
        let duplicate = std::panic::catch_unwind(|| {
            let _ = SheetRegistry::new().with(spec("home")).with(spec("home"));
        });
        assert!(duplicate.is_err(), "a second owner must fail fast");
    }

    #[test]
    fn unowned_ids_fall_back_to_the_permissive_answers() {
        let registry = SheetRegistry::new().with(spec("home"));
        assert!(registry.spec("session:s1").is_none());
        assert!(registry.closable("session:s1"));
        assert!(registry.names_container("session:s1"));
        assert_eq!(registry.parse("session:s1"), None);
        assert_eq!(registry.glyph("session:s1"), None);
    }

    #[test]
    fn declared_answers_win_over_the_defaults() {
        let mut silent = spec("session");
        silent.names_container = false;
        silent.parse = Some(Arc::new(|id| ParsedSheet::workspace_wide(id)));
        let registry = SheetRegistry::new().with(spec("terminal")).with(silent);

        assert!(registry.closable("terminal:c1:1"));
        assert!(!registry.names_container("session:s1"));
        assert_eq!(
            registry.parse("session:s1"),
            Some(ParsedSheet::workspace_wide("session:s1"))
        );
        assert_eq!(registry.glyph("terminal:c1:1"), Some("document"));
    }

    #[gpui::test]
    fn titles_tab_titles_and_restore_read_the_registry(cx: &mut gpui::TestAppContext) {
        let mut projected = spec("session");
        projected.tab_title = Some(Arc::new(|id, _| {
            Some(SharedString::from(format!("tab {id}")))
        }));
        let silent = spec("terminal");
        let mut declining = spec("browser");
        declining.names_container = false;
        declining.restore = Some(Arc::new(|id, _| id.ends_with(":alive")));
        let registry = SheetRegistry::new()
            .with(projected)
            .with(silent)
            .with(declining);

        let titles = cx.update(|cx| {
            (
                registry.title("session:s1", cx),
                registry.tab_title("session:s1", cx),
                // A kind without a tab-only projection falls back to its title.
                registry.tab_title("terminal:c1:1", cx),
                registry.title("nobody", cx),
                registry.container_title_of("session:s1", cx),
                // A kind that declines to name its container says nothing.
                registry.container_title_of("browser:b1", cx),
                registry.restore("browser:b1:alive", cx),
                registry.restore("browser:b1:dead", cx),
                // A kind without a restore hook keeps its sheets.
                registry.restore("session:s1", cx),
            )
        });
        let (title, tab, fallback_tab, unowned, names, declines, alive, dead, unchecked) = titles;

        assert_eq!(title.as_deref(), Some("title session:s1"));
        assert_eq!(tab.as_deref(), Some("tab session:s1"));
        assert_eq!(fallback_tab.as_deref(), Some("title terminal:c1:1"));
        assert_eq!(unowned, None);
        assert_eq!(names.as_deref(), Some("title session:s1"));
        assert_eq!(declines, None);
        assert!(alive);
        assert!(!dead);
        assert!(unchecked);
    }
}
