use anyhow::Result;
use e_sh::ui;
use tokio::runtime::Runtime;
use tracing_subscriber::EnvFilter;

mod app;

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();

    let rt = Runtime::new()?;
    let handle = rt.handle().clone();

    let _guard = rt.enter();
    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 800.0])
            .with_min_inner_size([800.0, 500.0])
            .with_title("e-sh"),
        ..Default::default()
    };

    let _ = ui::dock::EshTabViewer;

    eframe::run_native(
        "e-sh",
        native_options,
        Box::new(move |cc| Ok(Box::new(app::EshApp::new(cc, handle)))),
    )
    .map_err(|e| anyhow::anyhow!("eframe error: {e}"))?;

    Ok(())
}
