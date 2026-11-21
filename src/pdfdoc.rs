//! Renderizado de PDF en un hilo aparte con hayro (Rust puro, CPU).

use hayro::hayro_interpret::InterpreterSettings;
use hayro::hayro_syntax::Pdf;
use hayro::vello_cpu::color::palette::css::WHITE;
use hayro::{PixmapSettings, RenderCache, RenderSettings, render};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};

pub struct Rendered {
    /// zona con contenido (x0, y0, x1, y1) normalizada, para recortar márgenes
    pub crop: [f32; 4],
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

/// Imágenes de un cómic CBZ, ordenadas por nombre.
pub fn cbz_images(data: &[u8]) -> Option<Vec<Vec<u8>>> {
    use std::io::Read;
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(data)).ok()?;
    let mut names: Vec<String> = z
        .file_names()
        .filter_map(|n| n.ok())
        .filter(|n| {
            let l = n.to_lowercase();
            !l.contains("__macosx")
                && [".jpg", ".jpeg", ".png", ".webp", ".gif"]
                    .iter()
                    .any(|e| l.ends_with(e))
        })
        .map(|s| s.to_string())
        .collect();
    names.sort_by_key(|n| natural_key(n));
    let mut out = Vec::new();
    for n in names {
        let mut f = z.by_name(&n).ok()?;
        let mut b = Vec::new();
        f.read_to_end(&mut b).ok()?;
        out.push(b);
    }
    (!out.is_empty()).then_some(out)
}

/// Orden natural (pág2 < pág10).
fn natural_key(s: &str) -> Vec<(u64, String)> {
    let mut out = Vec::new();
    let mut num = String::new();
    let mut txt = String::new();
    for c in s.to_lowercase().chars() {
        if c.is_ascii_digit() {
            if !txt.is_empty() {
                out.push((0, std::mem::take(&mut txt)));
            }
            num.push(c);
        } else {
            if !num.is_empty() {
                out.push((num.parse().unwrap_or(0), String::new()));
                num.clear();
            }
            txt.push(c);
        }
    }
    if !num.is_empty() {
        out.push((num.parse().unwrap_or(0), String::new()));
    }
    if !txt.is_empty() {
        out.push((0, txt));
    }
    out
}

pub fn is_cbz(path: &str) -> bool {
    path.to_lowercase().ends_with(".cbz")
}

impl PdfWorker {
    /// Cómic: cada imagen es una página.
    pub fn spawn_images(images: Vec<Vec<u8>>, title: String, ctx: eframe::egui::Context) -> Self {
        let (tx, req_rx) = channel::<Vec<(usize, f32)>>();
        let (res_tx, rx) = channel();
        std::thread::spawn(move || {
            let sizes: Vec<(f32, f32)> = images
                .iter()
                .map(|b| {
                    image::ImageReader::new(std::io::Cursor::new(b))
                        .with_guessed_format()
                        .ok()
                        .and_then(|r| r.into_dimensions().ok())
                        .map(|(w, h)| (w as f32, h as f32))
                        .unwrap_or((800.0, 1200.0))
                })
                .collect();
            let _ = res_tx.send(Msg::Info {
                sizes: sizes.clone(),
                title,
            });
            ctx.request_repaint();
            let mut queue: Vec<(usize, f32)> = Vec::new();
            loop {
                if queue.is_empty() {
                    match req_rx.recv() {
                        Ok(q) => queue = q,
                        Err(_) => return,
                    }
                }
                while let Ok(q) = req_rx.try_recv() {
                    queue = q;
                }
                if queue.is_empty() {
                    continue;
                }
                let (idx, scale) = queue.remove(0);
                let Some(bytes) = images.get(idx) else {
                    continue;
                };
                let Ok(img) = image::load_from_memory(bytes) else {
                    continue;
                };
                let (w, h) = sizes[idx];
                let tw = ((w * scale) as u32).clamp(1, 6000);
                let th = ((h * scale) as u32).clamp(1, 6000);
                let img = if tw < img.width() {
                    img.resize(tw, th, image::imageops::FilterType::Triangle)
                } else {
                    img
                };
                let rgba = img.to_rgba8();
                let (rw, rh) = (rgba.width() as usize, rgba.height() as usize);
                let raw = rgba.into_raw();
                let crop = content_box(&raw, rw, rh);
                let r = Rendered {
                    crop,
                    page: idx,
                    scale,
                    width: rw,
                    height: rh,
                    rgba: raw,
                };
                if res_tx.send(Msg::Page(r)).is_err() {
                    return;
                }
                ctx.request_repaint();
            }
        });
        PdfWorker { tx, rx }
    }

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
                let crop = content_box(
                    pix.data_as_u8_slice(),
                    pix.width() as usize,
                    pix.height() as usize,
                );
                let r = Rendered {
                    crop,
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

pub fn first_page_size(path: &str) -> Option<(f32, f32)> {
    let data = std::fs::read(path).ok()?;
    let pdf = Pdf::new(Arc::new(data)).ok()?;
    let p = pdf.pages().first()?;
    Some(p.render_dimensions())
}

/// Caja del contenido no blanco de una página renderizada.
pub fn content_box(rgba: &[u8], w: usize, h: usize) -> [f32; 4] {
    if w == 0 || h == 0 {
        return [0.0, 0.0, 1.0, 1.0];
    }
    let step = ((w * h) / 400_000).max(1);
    let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0usize, 0usize);
    for y in (0..h).step_by(step) {
        let row = &rgba[y * w * 4..(y + 1) * w * 4];
        for x in (0..w).step_by(step) {
            let p = &row[x * 4..x * 4 + 3];
            if p[0] < 230 || p[1] < 230 || p[2] < 230 {
                x0 = x0.min(x);
                x1 = x1.max(x);
                y0 = y0.min(y);
                y1 = y1.max(y);
            }
        }
    }
    if x1 <= x0 || y1 <= y0 {
        return [0.0, 0.0, 1.0, 1.0];
    }
    let pad = 0.025;
    let fx = |v: usize| v as f32 / w as f32;
    let fy = |v: usize| v as f32 / h as f32;
    let c = [
        (fx(x0) - pad).max(0.0),
        (fy(y0) - pad).max(0.0),
        (fx(x1) + pad).min(1.0),
        (fy(y1) + pad).min(1.0),
    ];
    // no recortar si apenas gana espacio o el contenido es diminuto
    if (c[2] - c[0]) < 0.25 || (c[3] - c[1]) < 0.25 {
        [0.0, 0.0, 1.0, 1.0]
    } else {
        c
    }
}
