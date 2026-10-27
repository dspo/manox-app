//! gpui-shell Settings prototype wiring (the assembly layer's host seam).
//!
//! Steer embeds the gpui-kit train's scriptable application runtime
//! (gpui-shell 0.8.1): the main window's root becomes a [`ShellRoot`] — the
//! dialog / sheet / toast overlay layers over the chrome shell — and Settings
//! is a gpui-shell SCRIPT application (`settings-app/main.js`) opened as a
//! native floating dialog (`open_dialog_with`, Escape and backdrop
//! dismissable). Data flows through the `settingsNative` HostModule, so the
//! script never touches the filesystem or the app state directly.
//!
//! Prototype scope: the settings app directory resolves from the source tree
//! (`CARGO_MANIFEST_DIR/settings-app`), overridable with
//! `STEER_SETTINGS_APP_DIR`. The upstream `SettingItem` value bindings are
//! still "deferred" in the 0.8.1 catalog, so the script composes pages from
//! primitive components — a real cost datum for the migration assessment.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use gpui::{
    App, AppContext as _, Context, Entity, IntoElement, ParentElement, Render, Styled, Window, div,
    px,
};
use gpui_shell::{DialogOptions, HostModule, HostValue, ShellRoot, ShellRuntime, with_current_app};

/// Floating dialog size. The loaded application sizes itself; this host
/// wrapper pins the dialog footprint.
const DIALOG_W: f32 = 780.;
const DIALOG_H: f32 = 540.;

thread_local! {
    static RUNTIME: RefCell<Option<Rc<ShellRuntime>>> = const { RefCell::new(None) };
    static ROOT: RefCell<Option<Entity<ShellRoot>>> = const { RefCell::new(None) };
    static HOST: RefCell<Option<Entity<SettingsDialogHost>>> = const { RefCell::new(None) };
}

/// One-time process setup: the `settingsNative` host module, the script
/// runtime, and the Settings launcher that replaces the in-app overlay. Call
/// after `gpui_component::init` — the component catalog assumes it ran.
pub fn init(cx: &mut App) {
    let module = HostModule::new("settingsNative")
        .declarations(
            "export function getLanguage(): string;\n\
             export function setLanguage(token: string): string;\n\
             export function listMcp(): string;\n\
             export function setMcpEnabled(name: string, enabled: string): string;\n\
             export function about(): string;",
        )
        .function("getLanguage", |_args| {
            Ok(HostValue::from(
                steer_i18n::load_ui_language().token().to_string(),
            ))
        })
        .function("setLanguage", |args| {
            let token = args.string(0)?;
            let Some(lang) = steer_i18n::Language::from_token(token) else {
                return Err(format!("unknown language token: {token}").into());
            };
            steer_i18n::persist_ui_language(lang)
                .map_err(|e| format!("failed to save ui_language: {e}"))?;
            // Live refresh: the ambient App exists once a window is up; a
            // call before that (none today) persists only.
            let _ = with_current_app(|cx| {
                steer_i18n::set_ui_language(lang);
                gpui_component::set_locale(lang.token());
                crate::menu::rebuild_menus(cx);
                cx.refresh_windows();
            });
            Ok(HostValue::from("ok".to_string()))
        })
        .function("listMcp", |_args| Ok(HostValue::from(mcp_json())))
        .function("setMcpEnabled", |args| {
            let name = args.string(0)?.to_string();
            let enabled = args.string(1)? == "true";
            let mut disabled: std::collections::HashSet<String> =
                manox_agent::settings::mcp_disabled().into_iter().collect();
            if enabled {
                disabled.remove(&name);
            } else {
                disabled.insert(name);
            }
            let list: Vec<String> = disabled.into_iter().collect();
            manox_agent::settings::set_mcp_disabled(list)
                .map_err(|e| format!("failed to persist MCP switch: {e}"))?;
            Ok(HostValue::from("ok".to_string()))
        })
        .function("about", |_args| {
            Ok(HostValue::from(format!(
                "{{\"app\":\"{}\",\"runtime\":\"gpui-shell 0.8.1\"}}",
                env!("CARGO_PKG_VERSION")
            )))
        });
    gpui_shell::export_module(module).expect("register settingsNative");

    let components = gpui_shell::components::components().expect("component catalog");
    let runtime = ShellRuntime::new_with_components(cx, components).expect("script runtime");
    RUNTIME.with(|slot| *slot.borrow_mut() = Some(runtime));

    crate::set_settings_launcher(Rc::new(|window: &mut Window, cx: &mut App| {
        open(window, cx);
    }));
}

/// Wraps the window's content (the chrome shell) as a [`ShellRoot`], so the
/// window carries the native dialog / sheet / toast layers.
pub fn wrap_root<V: Render>(
    view: Entity<V>,
    window: &mut Window,
    cx: &mut App,
) -> Entity<ShellRoot> {
    let root = cx.new(|cx| ShellRoot::new(view.into(), window, cx));
    ROOT.with(|slot| *slot.borrow_mut() = Some(root.clone()));
    root
}

/// Opens the Settings floating dialog on the mounted ShellRoot.
pub fn open(window: &mut Window, cx: &mut App) {
    let Some(root) = ROOT.with(|slot| slot.borrow().clone()) else {
        return;
    };
    // The settings script loads once; `load` (not try_load) keeps a broken
    // script visible as the runtime's own failure surface inside the dialog.
    let host = HOST.with(|slot| {
        if let Some(host) = slot.borrow().as_ref() {
            return host.clone();
        }
        let runtime = RUNTIME
            .with(|slot| slot.borrow().clone())
            .expect("settings_shell::init ran");
        let dir = app_dir();
        let loaded = match runtime.try_load(&dir, window, cx) {
            Ok(loaded) => loaded,
            Err(error) => {
                // A broken script still opens: the runtime's failure surface
                // renders inside the dialog, with the message logged here.
                eprintln!("settings shell: script load failed: {error:#}");
                runtime.load(&dir, window, cx)
            }
        };
        let host = cx.new(|_| SettingsDialogHost { app: loaded });
        *slot.borrow_mut() = Some(host.clone());
        host
    });
    root.update(cx, |root, cx| {
        root.open_dialog_with(host.into(), DialogOptions::default(), window, cx);
    });
}

/// Drops the host's thread-local handles while the thread is still alive.
/// The runtime's Drop touches thread-locals, so letting them drop during TLS
/// destruction (a test binary returning from `main`) aborts the process.
/// The app itself quits through `process::exit`, which skips TLS drops.
pub fn teardown() {
    HOST.with(|slot| *slot.borrow_mut() = None);
    ROOT.with(|slot| *slot.borrow_mut() = None);
    RUNTIME.with(|slot| *slot.borrow_mut() = None);
}

/// Test/diagnostic access: whether the settings dialog is mounted. (Whether
/// the script itself loaded is not observable from outside the crate — a
/// broken script renders the runtime's failure surface inside the dialog.)
pub fn dialog_open(cx: &App) -> bool {
    ROOT.with(|slot| slot.borrow().clone())
        .is_some_and(|root| root.read(cx).dialog_count() > 0)
}

fn app_dir() -> PathBuf {
    std::env::var_os("STEER_SETTINGS_APP_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("settings-app"))
}

/// The dialog's footprint around the loaded script application.
struct SettingsDialogHost {
    app: Entity<ShellRoot>,
}

impl Render for SettingsDialogHost {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .w(px(DIALOG_W))
            .h(px(DIALOG_H))
            .child(self.app.clone())
    }
}

/// MCP servers as a JSON array the script can parse: name, enabled, and the
/// live registry status (tool count, or why there is none).
fn mcp_json() -> String {
    let disabled: std::collections::HashSet<String> =
        manox_agent::settings::mcp_disabled().into_iter().collect();
    let connected: std::collections::HashMap<String, usize> = manox_agent::mcp::try_global()
        .map(|registry| {
            registry
                .servers()
                .iter()
                .map(|s| (s.name.clone(), s.tools.len()))
                .collect()
        })
        .unwrap_or_default();
    let mut rows: Vec<String> = manox_agent::mcp::load_merged_config()
        .mcp_servers
        .keys()
        .map(|name| {
            let enabled = !disabled.contains(name);
            let tools = connected.get(name).copied().unwrap_or(0);
            format!("{{\"name\":{name:?},\"enabled\":{enabled},\"tools\":{tools}}}")
        })
        .collect();
    rows.sort();
    format!("[{}]", rows.join(","))
}
