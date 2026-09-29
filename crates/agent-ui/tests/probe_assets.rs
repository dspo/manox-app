//! The app asset source must resolve every icon a shipped card references:
//! gpui-kit-assets' generated `IconName` is a full Lucide catalog, but its
//! embedded set is a small subset — icons outside it must ship in the
//! manox-local overlay (assets.rs LocalAssets), or the render logs a
//! missing-asset error and draws nothing.

#[test]
fn the_plan_review_discuss_icon_ships() {
    use agent_ui::assets::ExtrasAssetSource;
    use gpui::AssetSource as _;
    let source = ExtrasAssetSource::new();
    let loaded = source.load("icons/pen-line.svg");
    assert!(
        matches!(loaded, Ok(Some(_))),
        "pen-line.svg must resolve through the app's asset source"
    );
}
