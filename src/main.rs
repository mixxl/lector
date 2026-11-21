#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod book;
mod cover;
mod covers;
mod epub;
mod layout;
mod pdfdoc;

use eframe::egui::{self, FontData, FontDefinitions, FontFamily};
use std::sync::Arc;

fn fonts() -> FontDefinitions {
    let mut f = FontDefinitions::default();
    let add = |f: &mut FontDefinitions, name: &str, data: &'static [u8]| {
        f.font_data
            .insert(name.into(), Arc::new(FontData::from_static(data)));
        let mut fam = vec![name.to_string()];
        // reserva para símbolos que Crimson no tenga
        fam.extend(f.families[&FontFamily::Proportional].iter().cloned());
        f.families.insert(
            FontFamily::Name(name.replace("crimson", "serif").into()),
            fam,
        );
    };
    add(
        &mut f,
        "crimson",
        include_bytes!("../assets/fonts/CrimsonText-Regular.ttf"),
    );
    add(
        &mut f,
        "crimson-i",
        include_bytes!("../assets/fonts/CrimsonText-Italic.ttf"),
    );
    add(
        &mut f,
        "crimson-b",
        include_bytes!("../assets/fonts/CrimsonText-Bold.ttf"),
    );
    add(
        &mut f,
        "crimson-bi",
        include_bytes!("../assets/fonts/CrimsonText-BoldItalic.ttf"),
    );
    f
}

fn main() -> eframe::Result {
    let opts = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Lector")
            .with_inner_size([1280.0, 820.0])
            .with_min_inner_size([520.0, 420.0])
            .with_drag_and_drop(true),
        ..Default::default()
    };
    let initial: Vec<String> = std::env::args().skip(1).collect();
    eframe::run_native(
        "Lector",
        opts,
        Box::new(move |cc| {
            cc.egui_ctx.set_fonts(fonts());
            Ok(Box::new(app::App::new(cc, initial)))
        }),
    )
}
