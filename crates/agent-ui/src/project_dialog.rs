//! The create/edit-project dialog's state machine and write path (design
//! §2.4): the draft (`name` input, `roots`, `primary`) lives here, the
//! chrome's [`steer_agent_chrome_ui::ProjectDialog`] renders it, and a
//! confirmed save lands through [`crate::projects`] — creation via
//! `create_project`, editing via `update_name` + `set_roots`, removal via
//! `delete_project` behind the shell's alert-dialog confirmation. The
//! dialog closes itself on a successful save; a failure stays inline (the
//! red line) because the picker only yields existing directories, so a
//! refusal is rare and needs no popup.
//!
//! The name field's placeholder follows the primary root's basename until
//! the user touches the field — the same follow-until-overridden rule the
//! registry's display name keeps, previewed live as「设为主要」moves the
//! badge.

use std::path::{Path, PathBuf};
use std::rc::Rc;

use gpui::{AppContext as _, Context, Entity, IntoElement, Render, Subscription, Window};
use gpui_component::WindowExt as _;
use gpui_component::input::{InputEvent, InputState};
use steer_agent_chrome_ui::project_dialog::{OnRootPath, ProjectDialog, ProjectRootRow};
use steer_agent_chrome_ui::session_list::{OnUnit, OnWindowApp};

/// Which registry operation the open dialog commits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Mode {
    Create,
    Edit { project_id: String },
}

/// The dialog draft — one instance per open dialog, mounted into the shell's
/// dialog slot by [`crate::chrome_assembly::open_project_dialog`].
pub struct ProjectDialogState {
    mode: Mode,
    name: Entity<InputState>,
    /// The user typed in the name field at least once; until then the
    /// placeholder follows the primary root's basename.
    name_touched: bool,
    /// The picked roots, insertion order — the primary is an attribute on
    /// `primary`, never a list position.
    roots: Vec<PathBuf>,
    primary: Option<PathBuf>,
    /// The last save failure's inline line (cleared on the next attempt).
    error: Option<String>,
    _name_sub: Subscription,
}

impl ProjectDialogState {
    /// The create-mode draft: no roots yet (the confirm action stays inert
    /// until the picker fills them).
    pub fn for_create(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let (name, sub) = Self::build_name_field(None, placeholder_text(None), window, cx);
        Self {
            mode: Mode::Create,
            name,
            name_touched: false,
            roots: Vec::new(),
            primary: None,
            error: None,
            _name_sub: sub,
        }
    }

    /// The edit-mode draft, prefilled from the registry. The name field
    /// carries the custom name only when the project is overridden — a
    /// following project keeps the field empty so the placeholder keeps
    /// previewing the primary's basename.
    pub fn for_edit(
        project: crate::projects::Project,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let primary = project
            .roots
            .iter()
            .find(|root| root.primary)
            .map(|root| root.path.clone());
        let roots: Vec<PathBuf> = project.roots.iter().map(|root| root.path.clone()).collect();
        let prefill = project.name_overridden.then(|| project.name.clone());
        let (name, sub) =
            Self::build_name_field(prefill, placeholder_text(primary.as_deref()), window, cx);
        Self {
            mode: Mode::Edit {
                project_id: project.id,
            },
            name,
            name_touched: false,
            roots,
            primary,
            error: None,
            _name_sub: sub,
        }
    }

    /// The name field: built, prefilled, focused, and subscribed — Change
    /// marks the name touched (the placeholder stops following), Enter
    /// confirms from the field. The prefill lands BEFORE the subscription
    /// exists, so an edit-mode prefill never counts as a touch.
    fn build_name_field(
        prefill: Option<String>,
        placeholder: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> (Entity<InputState>, Subscription) {
        let name = cx.new(|cx| {
            let mut state = InputState::new(window, cx).placeholder(placeholder);
            if let Some(value) = prefill {
                state.set_value(value, window, cx);
            }
            state
        });
        let sub = cx.subscribe_in(
            &name,
            window,
            |this, _input, event: &InputEvent, _window, cx| match event {
                InputEvent::Change => this.name_touched = true,
                InputEvent::PressEnter { .. } => this.confirm(cx),
                _ => {}
            },
        );
        name.update(cx, |state, cx| state.focus(window, cx));
        (name, sub)
    }

    /// Dismiss without saving (✕ / 取消 / Escape).
    pub fn cancel(&mut self, cx: &mut Context<Self>) {
        crate::chrome_assembly::close_project_dialog(cx);
    }

    /// The ＋ 添加文件夹… row: the native multi-select folder picker, as an
    /// ASYNC dialog. The panel is a modal loop of its own: spinning it inside
    /// the click's event dispatch panics the loop, so the panel runs through
    /// rfd's ModalFuture — dispatched to the main thread after this dispatch
    /// settles — and the result lands back on the entity. Duplicate picks are
    /// silent no-ops.
    pub fn add_folder(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let this = cx.entity().downgrade();
        cx.spawn(async move |_, cx| {
            let Some(handles) = rfd::AsyncFileDialog::new().pick_folders().await else {
                return;
            };
            let picked: Vec<PathBuf> = handles
                .iter()
                .map(|handle| handle.path().to_path_buf())
                .collect();
            if picked.is_empty() {
                return;
            }
            let _ = this.update_in(cx, |state, window, cx| {
                if absorb_roots(&mut state.roots, &mut state.primary, picked) {
                    state.sync_placeholder(window, cx);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Move the primary badge to `path` (list order untouched); the
    /// placeholder previews the display-name change while following.
    pub fn set_primary(&mut self, path: &str, window: &mut Window, cx: &mut Context<Self>) {
        let path = PathBuf::from(path);
        if self.roots.iter().any(|root| is_same_path(root, &path)) {
            self.primary = Some(canonical_for_display(&path));
            self.sync_placeholder(window, cx);
            cx.notify();
        }
    }

    /// Remove a root row. Dropping the primary hands the badge to the first
    /// remaining root; dropping the last root clears it (and confirm goes
    /// inert with it).
    pub fn remove_root(&mut self, path: &str, window: &mut Window, cx: &mut Context<Self>) {
        if drop_root(&mut self.roots, &mut self.primary, Path::new(path)) {
            self.sync_placeholder(window, cx);
            cx.notify();
        }
    }

    /// Commit the draft. Creation is `create_project` (blank name → the
    /// display name follows the primary — the registry trims); editing is
    /// `update_name` + `set_roots`. Success closes the dialog; failure keeps
    /// it open with the inline red line.
    pub fn confirm(&mut self, cx: &mut Context<Self>) {
        self.error = None;
        let name = self.name.read(cx).value().trim().to_string();
        let result = match &self.mode {
            Mode::Create => crate::projects::create_project(
                Some(name),
                self.roots.clone(),
                self.primary.clone(),
            )
            .map(|_| ()),
            Mode::Edit { project_id } => crate::projects::update_name(project_id, Some(name))
                .and_then(|_| {
                    crate::projects::set_roots(project_id, self.roots.clone(), self.primary.clone())
                }),
        };
        match result {
            Ok(()) => crate::chrome_assembly::close_project_dialog(cx),
            Err(e) => {
                self.error = Some(steer_i18n::t_str(
                    "project-error-save",
                    &[("err", &format!("{e:#}"))],
                ));
                cx.notify();
            }
        }
    }

    /// The edit-mode remove row: the registry delete behind the shell's
    /// alert-dialog confirmation (the app's one confirm face). The
    /// confirmation outlives this entity's frame, so it holds a weak handle.
    pub fn begin_remove(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Mode::Edit { project_id } = self.mode.clone() else {
            return;
        };
        let dialog = cx.entity().downgrade();
        window.open_alert_dialog(cx, move |alert, _, _| {
            // The builder may re-run: every face clones its own handles, the
            // on_ok closure moves them out of THIS builder run.
            let project_id = project_id.clone();
            let dialog = dialog.clone();
            alert
                .title(steer_i18n::t("project-remove-confirm-title"))
                .description(steer_i18n::t("project-remove-confirm-body"))
                .show_cancel(true)
                .ok_text(steer_i18n::t("project-remove-confirm-ok"))
                .cancel_text(steer_i18n::t("project-cancel"))
                .on_ok(move |_, _, cx| {
                    crate::projects::delete_project(&project_id);
                    if let Some(dialog) = dialog.upgrade() {
                        dialog.update(cx, |_, cx| crate::chrome_assembly::close_project_dialog(cx));
                    }
                    true
                })
        });
    }

    /// Preview the follow-the-primary rule in the placeholder while the
    /// field is untouched (a touched field keeps whatever the user typed).
    fn sync_placeholder(&self, window: &mut Window, cx: &mut Context<Self>) {
        if self.name_touched {
            return;
        }
        let placeholder = placeholder_text(self.primary.as_deref());
        self.name.update(cx, |state, cx| {
            state.set_placeholder(placeholder, window, cx)
        });
    }
}

impl Render for ProjectDialogState {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let edit = matches!(self.mode, Mode::Edit { .. });
        let rows: Vec<ProjectRootRow> = self
            .roots
            .iter()
            .map(|path| ProjectRootRow {
                path: path.to_string_lossy().into_owned(),
                label: root_label(path),
                primary: self.primary.as_deref() == Some(path.as_path()),
            })
            .collect();

        let on_cancel: OnWindowApp = {
            let this = cx.entity().downgrade();
            Rc::new(move |_, cx| {
                let _ = this.update(cx, |state, cx| state.cancel(cx));
            })
        };
        let on_add_folder: OnUnit = {
            let this = cx.entity();
            Rc::new(move |_, window, cx| this.update(cx, |state, cx| state.add_folder(window, cx)))
        };
        let on_set_primary: OnRootPath = {
            let this = cx.entity();
            Rc::new(move |path, window, cx| {
                let path = path.to_string();
                this.update(cx, |state, cx| state.set_primary(&path, window, cx))
            })
        };
        let on_remove_root: OnRootPath = {
            let this = cx.entity();
            Rc::new(move |path, window, cx| {
                let path = path.to_string();
                this.update(cx, |state, cx| state.remove_root(&path, window, cx))
            })
        };
        let on_confirm: OnUnit = {
            let this = cx.entity();
            Rc::new(move |_, _, cx| this.update(cx, |state, cx| state.confirm(cx)))
        };
        let on_remove: Option<OnUnit> = edit.then(|| {
            let this = cx.entity();
            let cb: OnUnit = Rc::new(move |_, window, cx| {
                this.update(cx, |state, cx| state.begin_remove(window, cx))
            });
            cb
        });

        ProjectDialog {
            edit,
            name_input: self.name.clone(),
            roots: rows,
            error: self.error.clone(),
            confirm_disabled: self.roots.is_empty(),
            on_add_folder,
            on_set_primary,
            on_remove_root,
            on_cancel,
            on_confirm,
            on_remove,
        }
    }
}

/// Best-effort canonical form for the dialog's own comparisons; the registry
/// canonicalizes for real at save time. An uncanonicalizable path (a picked
/// path that vanished mid-dialog) compares as its raw spelling.
fn canonical_for_display(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

fn is_same_path(a: &Path, b: &Path) -> bool {
    canonical_for_display(a) == canonical_for_display(b)
}

/// The row's display label: the basename, falling back to the whole path
/// for a root with no file name (the same rule the registry's display name
/// keeps).
fn root_label(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

/// The name field's placeholder: the primary root's basename (the display
/// name an empty field implies), the whole path when the root has no file
/// name, and the localized fallback while there is no root at all.
fn placeholder_text(primary: Option<&Path>) -> String {
    match primary {
        None => steer_i18n::t("project-name-placeholder"),
        Some(path) => match path.file_name() {
            Some(base) => base.to_string_lossy().into_owned(),
            None => path.to_string_lossy().into_owned(),
        },
    }
}

/// Append the picked folders, skipping every path already among the roots
/// (canonical compare — a duplicate pick is a no-op). The first added root
/// adopts the primary badge when none is set. Returns whether anything
/// changed.
fn absorb_roots(
    roots: &mut Vec<PathBuf>,
    primary: &mut Option<PathBuf>,
    picked: Vec<PathBuf>,
) -> bool {
    let mut changed = false;
    for path in picked {
        if roots.iter().any(|root| is_same_path(root, &path)) {
            continue;
        }
        if primary.is_none() {
            *primary = Some(canonical_for_display(&path));
        }
        roots.push(path);
        changed = true;
    }
    changed
}

/// Drop `path` from the roots. Dropping the primary hands the badge to the
/// first remaining root; dropping the last root clears it. Returns whether
/// anything changed (an unknown path is a no-op).
fn drop_root(roots: &mut Vec<PathBuf>, primary: &mut Option<PathBuf>, path: &Path) -> bool {
    let Some(at) = roots.iter().position(|root| is_same_path(root, path)) else {
        return false;
    };
    roots.remove(at);
    if primary
        .as_deref()
        .is_some_and(|current| is_same_path(current, path))
    {
        *primary = roots.first().cloned();
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn init_i18n() {
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(steer_i18n::init);
    }

    fn real_dir(label: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().join(label);
        std::fs::create_dir(&root).expect("mkdir");
        (dir, root)
    }

    /// The picker's dedup: a second pick of the same directory (raw or
    /// canonical spelling) is a no-op, the first added root adopts the
    /// primary badge, and later picks never steal it.
    #[test]
    fn absorb_roots_dedups_and_adopts_the_first_primary() {
        let (_dir, root) = real_dir("widget");
        let canonical = root.canonicalize().expect("canonicalize");
        let mut roots: Vec<PathBuf> = Vec::new();
        let mut primary: Option<PathBuf> = None;

        assert!(absorb_roots(&mut roots, &mut primary, vec![root.clone()]));
        assert_eq!(roots, vec![root.clone()]);
        assert_eq!(primary.as_deref(), Some(canonical.as_path()));

        // The same directory again — raw and canonical spellings — is
        // silent; the badge stays on the first root.
        assert!(!absorb_roots(
            &mut roots,
            &mut primary,
            vec![root.clone(), canonical.clone()]
        ));
        assert_eq!(roots.len(), 1);
        assert_eq!(primary.as_deref(), Some(canonical.as_path()));

        // A new root lands without touching the badge.
        let (_dir_b, other) = real_dir("anvil");
        assert!(absorb_roots(&mut roots, &mut primary, vec![other.clone()]));
        assert_eq!(roots, vec![root, other]);
        assert_eq!(primary.as_deref(), Some(canonical.as_path()));
    }

    /// Dropping the primary hands the badge to the first remaining root,
    /// dropping the last root clears it, and an unknown path is a no-op.
    #[test]
    fn drop_root_moves_the_badge_and_clears_on_empty() {
        let (_dir_a, a) = real_dir("a");
        let (_dir_b, b) = real_dir("b");
        let mut roots = vec![a.clone(), b.clone()];
        let mut primary = Some(a.clone());

        assert!(!drop_root(&mut roots, &mut primary, Path::new("/nowhere")));
        assert_eq!(roots.len(), 2, "unknown path is a no-op");

        assert!(drop_root(&mut roots, &mut primary, &a));
        assert_eq!(roots, vec![b.clone()]);
        assert_eq!(
            primary.as_deref(),
            Some(b.as_path()),
            "badge falls to first"
        );

        assert!(drop_root(&mut roots, &mut primary, &b));
        assert!(roots.is_empty());
        assert_eq!(primary, None, "the last root clears the badge");

        assert!(!drop_root(&mut roots, &mut primary, &b), "already empty");
    }

    /// The placeholder previews the display-name rule: the primary's
    /// basename, the whole path for a root with no file name, and the
    /// localized fallback with no root at all.
    #[test]
    fn placeholder_follows_the_primary_basename() {
        init_i18n();
        let (_dir, root) = real_dir("widget");
        assert_eq!(placeholder_text(Some(&root)), "widget");
        assert_eq!(placeholder_text(Some(Path::new("/"))), "/");
        assert_eq!(
            placeholder_text(None),
            steer_i18n::t("project-name-placeholder")
        );
    }

    /// The row label matches the registry's display-name fallback rule:
    /// basename first, whole path when there is no file name.
    #[test]
    fn root_label_prefers_the_basename() {
        let (_dir, root) = real_dir("widget");
        assert_eq!(root_label(&root), "widget");
        assert_eq!(root_label(Path::new("/")), "/");
    }
}
