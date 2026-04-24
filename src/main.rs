use anyhow::Result;
use e_sh::ui;
use tokio::runtime::Runtime;
use tracing_subscriber::EnvFilter;

mod app;

const APP_ICON_PNG: &[u8] = include_bytes!("../assets/icon-1024.png");

fn load_app_icon() -> Option<egui::IconData> {
    let img = image::load_from_memory_with_format(APP_ICON_PNG, image::ImageFormat::Png).ok()?;
    let rgba = img.to_rgba8();
    let (width, height) = rgba.dimensions();
    Some(egui::IconData { rgba: rgba.into_raw(), width, height })
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();

    let rt = Runtime::new()?;
    let handle = rt.handle().clone();

    let _guard = rt.enter();
    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([1280.0, 800.0])
        .with_min_inner_size([800.0, 500.0])
        .with_title("e-sh");
    if let Some(icon) = load_app_icon() {
        viewport = viewport.with_icon(icon);
    }
    let native_options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    let _ = ui::dock::EshTabViewer::default();

    eframe::run_native(
        "e-sh",
        native_options,
        Box::new(move |cc| Ok(Box::new(app::EshApp::new(cc, handle)))),
    )
    .map_err(|e| anyhow::anyhow!("eframe error: {e}"))?;

    Ok(())
}
