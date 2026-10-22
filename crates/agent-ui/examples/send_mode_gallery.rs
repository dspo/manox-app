//! Eyeball surface for the composer's mid-turn send modes (steer / queue /
//! interrupt): the real chrome assembly driven against a detached store —
//! no host, no model, no wire. The top bar fires the settle/retire paths;
//! Enter in the composer rides the real mode routing.
//!
//! ```bash
//! cargo run -p agent-ui --features test-support --example send_mode_gallery
//! ```
fn main() {
    let app = gpui_platform::application().with_assets(agent_ui::assets::ExtrasAssetSource::new());
    agent_ui::workspace::gallery::launch(app);
}
