//! Portadas y metadatos para la biblioteca, en segundo plano y con caché en disco.

use eframe::egui::{self, ColorImage};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};

pub struct CoverResult {
    pub path: String,
    pub image: Option<ColorImage>,
    pub meta: Option<crate::epub::Info>,
    pub meta_only: bool,
}

pub struct CoverWorker {
    tx: Sender<(String, bool, bool)>,
    pub rx: Receiver<CoverResult>,
}

const THUMB_W: u32 = 320;

fn cache_dir() -> Option<PathBuf> {
    let d = dirs::cache_dir()?.join("lector").join("portadas");
    std::fs::create_dir_all(&d).ok()?;
    Some(d)
}

fn cache_file(path: &str) -> Option<PathBuf> {
    let mut h = DefaultHasher::new();
    path.hash(&mut h);
    if let Ok(m) = std::fs::metadata(path) {
        if let Ok(t) = m.modified() {
            t.hash(&mut h);
        }
    }
    Some(cache_dir()?.join(format!("{:016x}.png", h.finish())))
}

fn to_color_image(img: image::RgbaImage) -> ColorImage {
    let size = [img.width() as usize, img.height() as usize];
    ColorImage::from_rgba_unmultiplied(size, img.as_raw())
}

fn make_meta(path: &str) -> Option<crate::epub::Info> {
    let p = Path::new(path);
    if crate::pdfdoc::is_cbz(path) {
        return Some(crate::epub::Info {
            title: String::new(),
            author: String::new(),
            subjects: vec![],
        });
    }
    if p.extension().is_some_and(|e| e.eq_ignore_ascii_case("pdf")) {
        let data = std::fs::read(p).ok()?;
        let title = crate::pdfdoc::title(&data).unwrap_or_default();
        Some(crate::epub::Info {
            title,
            author: String::new(),
            subjects: vec![],
        })
    } else {
        crate::epub::quick_info(p).map(|(i, _)| i)
    }
}

fn make_cover(path: &str) -> (Option<image::RgbaImage>, Option<crate::epub::Info>) {
    let p = Path::new(path);
    let is_pdf = p.extension().is_some_and(|e| e.eq_ignore_ascii_case("pdf"));
    if crate::pdfdoc::is_cbz(path) {
        let img = std::fs::read(p)
            .ok()
            .and_then(|d| crate::pdfdoc::cbz_images(&d))
            .and_then(|v| v.into_iter().next())
            .and_then(|b| image::load_from_memory(&b).ok())
            .map(|i| i.thumbnail(THUMB_W, THUMB_W * 2).to_rgba8());
        return (
            img,
            Some(crate::epub::Info {
                title: String::new(),
                author: String::new(),
                subjects: vec![],
            }),
        );
    }
    if is_pdf {
        let Ok(data) = std::fs::read(p) else {
            return (None, None);
        };
        let title = crate::pdfdoc::title(&data).unwrap_or_default();
        let img = crate::pdfdoc::thumbnail(data, THUMB_W as f32)
            .and_then(|(w, h, rgba)| image::RgbaImage::from_raw(w as u32, h as u32, rgba));
        (
            img,
            Some(crate::epub::Info {
                title,
                author: String::new(),
                subjects: vec![],
            }),
        )
    } else {
        let Some((info, cover)) = crate::epub::quick_info(p) else {
            return (None, None);
        };
        let img = cover
            .and_then(|b| image::load_from_memory(&b).ok())
            .map(|i| i.thumbnail(THUMB_W, THUMB_W * 2).to_rgba8());
        (img, Some(info))
    }
}

fn process(path: &str, need_meta: bool) -> CoverResult {
    let cf = cache_file(path);
    let cached = cf
        .as_ref()
        .and_then(|f| image::open(f).ok())
        .map(|i| i.to_rgba8());
    let (img, meta) = match cached {
        Some(i) => (Some(i), if need_meta { make_meta(path) } else { None }),
        None => {
            let (img, meta) = make_cover(path);
            if let (Some(i), Some(f)) = (&img, &cf) {
                let _ = i.save(f);
            }
            (img, meta)
        }
    };
    CoverResult {
        path: path.to_string(),
        image: img.map(to_color_image),
        meta,
        meta_only: false,
    }
}

impl CoverWorker {
    pub fn spawn(ctx: egui::Context) -> Self {
        let (tx, req) = channel::<(String, bool, bool)>();
        let req = std::sync::Arc::new(std::sync::Mutex::new(req));
        let (res, rx) = channel();
        let threads = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(2)
            .clamp(2, 4);
        for _ in 0..threads {
            let req = req.clone();
            let res = res.clone();
            let ctx = ctx.clone();
            std::thread::spawn(move || {
                loop {
                    let msg = req.lock().ok().and_then(|r| r.recv().ok());
                    let Some((path, need_meta, meta_only)) = msg else {
                        return;
                    };
                    let r = std::panic::catch_unwind(|| {
                        if meta_only {
                            CoverResult {
                                path: path.clone(),
                                image: None,
                                meta: make_meta(&path),
                                meta_only: true,
                            }
                        } else {
                            process(&path, need_meta)
                        }
                    })
                    .unwrap_or(CoverResult {
                        path: path.clone(),
                        image: None,
                        meta: None,
                        meta_only,
                    });
                    if res.send(r).is_err() {
                        return;
                    }
                    ctx.request_repaint();
                }
            });
        }
        CoverWorker { tx, rx }
    }

    pub fn request(&self, path: &str, need_meta: bool) {
        let _ = self.tx.send((path.to_string(), need_meta, false));
    }

    pub fn request_meta(&self, path: &str) {
        let _ = self.tx.send((path.to_string(), true, true));
    }
}

/// Busca libros (PDF/EPUB) en una carpeta. Los EPUB "desempaquetados" de
/// Apple Books son carpetas con extensión .epub.
pub fn scan(dir: &Path, out: &mut Vec<PathBuf>, depth: usize) {
    if depth > 6 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        let name = p
            .file_name()
            .map(|n| n.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        if name.starts_with('.') {
            continue;
        }
        if [".epub", ".pdf", ".mobi", ".azw", ".azw3", ".prc", ".cbz"]
            .iter()
            .any(|e| name.ends_with(e))
        {
            out.push(p);
        } else if p.is_dir() {
            scan(&p, out, depth + 1);
        }
    }
}

/// Carpetas donde Apple Books guarda los libros en macOS.
pub fn apple_books_dirs() -> Vec<PathBuf> {
    let Some(home) = dirs::home_dir() else {
        return vec![];
    };
    vec![
        home.join("Library/Containers/com.apple.BKAgentService/Data/Documents/iBooks/Books"),
        home.join("Library/Mobile Documents/iCloud~com~apple~iBooks/Documents"),
    ]
}
