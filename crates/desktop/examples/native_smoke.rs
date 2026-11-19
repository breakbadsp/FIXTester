//! Opens a native window and checks that the renderer produced a frame.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use eframe::egui::{self, ViewportCommand};

struct NativeSmoke {
    requested_screenshot: bool,
    rendered: Arc<AtomicBool>,
}

impl eframe::App for NativeSmoke {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ui, |ui| {
            ui.heading("FIXTester");
            ui.label("Native display check");
        });

        if !self.requested_screenshot {
            self.requested_screenshot = true;
            let context = ui.ctx().clone();
            let rendered = Arc::clone(&self.rendered);
            ui.ctx().request_screenshot(move |image| {
                let has_content = image
                    .pixels
                    .first()
                    .is_some_and(|first| image.pixels.iter().any(|pixel| pixel != first));
                rendered.store(has_content, Ordering::Relaxed);
                context.send_viewport_cmd(ViewportCommand::Close);
            });
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([480.0, 240.0]),
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };
    let rendered = Arc::new(AtomicBool::new(false));
    let result = Arc::clone(&rendered);
    eframe::run_native(
        "FIXTester display check",
        options,
        Box::new(move |_context| {
            Ok(Box::new(NativeSmoke {
                requested_screenshot: false,
                rendered: result,
            }))
        }),
    )?;
    if !rendered.load(Ordering::Relaxed) {
        return Err("The window closed before a frame was verified.".into());
    }
    println!("Verified native OpenGL frame.");
    Ok(())
}
