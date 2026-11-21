//! Renderizado de PDF en un hilo aparte con hayro (Rust puro, CPU).

use hayro::hayro_interpret::InterpreterSettings;
use hayro::hayro_syntax::Pdf;
use hayro::vello_cpu::color::palette::css::WHITE;
use hayro::{PixmapSettings, RenderCache, RenderSettings, render};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};

pub struct Rendered {
    pub page: usize,
    pub scale: f32,
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
}

pub enum Msg {
    Info {
        sizes: Vec<(f32, f32)>,
        title: String,
    },
    Page(Rendered),
    Error(String),
}

pub struct PdfWorker {
    tx: Sender<Vec<(usize, f32)>>,
    pub rx: Receiver<Msg>,
}

impl PdfWorker {
    pub fn spawn(data: Vec<u8>, ctx: eframe::egui::Context) -> Self {
        let (tx, req_rx) = channel::<Vec<(usize, f32)>>();
        let (res_tx, rx) = channel();
        std::thread::spawn(move || {
            let pdf = match Pdf::new(Arc::new(data)) {
                Ok(p) => p,
                Err(e) => {
                    let _ = res_tx.send(Msg::Error(match e {
                        hayro::hayro_syntax::LoadPdfError::Decryption(_) => {
                            "PDF protegido con contraseña (no soportado)".into()
                        }
                        _ => "PDF dañado o no válido".into(),
                    }));
                    ctx.request_repaint();
                    return;
                }
            };
            let pages = pdf.pages();
            let sizes: Vec<(f32, f32)> = pages.iter().map(|p| p.render_dimensions()).collect();
            let title = pdf
                .metadata()
                .title
                .as_ref()
                .map(|t| pdf_string(t))
                .filter(|s| s.chars().count() > 1 && !s.eq_ignore_ascii_case("untitled"))
                .unwrap_or_default();
            let _ = res_tx.send(Msg::Info { sizes, title });
            ctx.request_repaint();

            let mut cache = RenderCache::new();
            let mut rendered = 0usize;
            let settings = InterpreterSettings::default();
            let rsettings = RenderSettings::default();
            let mut queue: Vec<(usize, f32)> = Vec::new();
            loop {
                if queue.is_empty() {
                    match req_rx.recv() {
                        Ok(q) => queue = q,
                        Err(_) => return,
                    }
                }
                // Coge siempre la petición más reciente
                while let Ok(q) = req_rx.try_recv() {
                    queue = q;
                }
                if queue.is_empty() {
                    continue;
                }
                let (idx, scale) = queue.remove(0);
                // hayro no permite vaciar la caché: se renueva de vez en cuando
                rendered += 1;
                if rendered % 40 == 0 {
                    cache = RenderCache::new();
                }
                let Some(page) = pages.get(idx) else { continue };
                let pix = render(
                    page,
                    &cache,
                    &settings,
                    &rsettings,
                    &PixmapSettings {
                        x_scale: scale,
                        y_scale: scale,
                        bg_color: WHITE,
                    },
                );
                let r = Rendered {
                    page: idx,
                    scale,
                    width: pix.width() as usize,
                    height: pix.height() as usize,
                    rgba: pix.data_as_u8_slice().to_vec(),
                };
                if res_tx.send(Msg::Page(r)).is_err() {
                    return;
                }
                ctx.request_repaint();
            }
        });
        PdfWorker { tx, rx }
    }

    /// Envía la lista priorizada de páginas a renderizar (sustituye a la anterior).
    pub fn request(&self, list: Vec<(usize, f32)>) {
        let _ = self.tx.send(list);
    }
}

/// Miniatura de la primera página (para la biblioteca).
pub fn thumbnail(data: Vec<u8>, width: f32) -> Option<(usize, usize, Vec<u8>)> {
    let pdf = Pdf::new(Arc::new(data)).ok()?;
    let page = pdf.pages().first()?;
    let (w, _) = page.render_dimensions();
    let s = width / w.max(1.0);
    let cache = RenderCache::new();
    let pix = render(
        page,
        &cache,
        &InterpreterSettings::default(),
        &RenderSettings::default(),
        &PixmapSettings {
            x_scale: s,
            y_scale: s,
            bg_color: WHITE,
        },
    );
    Some((
        pix.width() as usize,
        pix.height() as usize,
        pix.data_as_u8_slice().to_vec(),
    ))
}

pub fn title(data: &[u8]) -> Option<String> {
    let pdf = Pdf::new(Arc::new(data.to_vec())).ok()?;
    let t = pdf.metadata().title.as_ref()?;
    let s = pdf_string(t);
    let bad = s.chars().count() < 2
        || s.eq_ignore_ascii_case("untitled")
        || s.to_lowercase().starts_with("microsoft ");
    (!bad).then_some(s)
}

fn pdf_string(b: &[u8]) -> String {
    if b.len() >= 2 && b[0] == 0xFE && b[1] == 0xFF {
        let u: Vec<u16> = b[2..]
            .chunks_exact(2)
            .map(|c| u16::from_be_bytes([c[0], c[1]]))
            .collect();
        String::from_utf16_lossy(&u).trim().to_string()
    } else {
        String::from_utf8_lossy(b).trim().to_string()
    }
}
