use crate::book::{self, Flip, Side, Theme};
use crate::covers::{self, CoverWorker};
use crate::epub::{self, Epub};
use crate::layout::{self, Item, Layout, Params};
use crate::pdfdoc::{Msg, PdfWorker};
use eframe::egui::{
    self, Align2, Color32, FontFamily, FontId, Key, Mesh, Pos2, Rect, Sense, Shape, TextureHandle,
    TextureOptions, Vec2, pos2, vec2,
};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Estado persistente

#[derive(Serialize, Deserialize, Clone, PartialEq)]
#[serde(default)]
pub struct Settings {
    pub theme: usize,
    pub font_size: f32,
    pub line_spacing: f32,
    pub margin: f32,
    pub sans: bool,
    pub justify: bool,
    /// 0 automático, 1 una página, 2 doble página
    pub page_mode: u8,
    pub cover_alone: bool,
    pub animate: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            theme: 0,
            font_size: 19.0,
            line_spacing: 1.42,
            margin: 0.10,
            sans: false,
            justify: true,
            page_mode: 0,
            cover_alone: true,
            animate: true,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(default)]
pub struct Entry {
    pub path: String,
    pub title: String,
    pub author: String,
    pub is_pdf: bool,
    pub page: usize,
    pub chapter: usize,
    pub block: usize,
    pub progress: f32,
    pub opened: bool,
    pub last_opened: u64,
    pub added: u64,
    pub meta_done: bool,
    pub bookmarks: Vec<(usize, usize)>,
}

#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
struct Saved {
    settings: Settings,
    library: Vec<Entry>,
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Documento abierto

struct PdfState {
    worker: PdfWorker,
    sizes: Vec<(f32, f32)>,
    tex: HashMap<usize, (TextureHandle, f32)>,
    pending: HashMap<usize, f32>,
    zoom: f32,
    pan: Vec2,
    tex_night: bool,
    error: Option<String>,
}

struct EpubState {
    epub: Epub,
    layout: Option<Layout>,
    want: Option<(Params, f64)>,
    images: HashMap<String, Option<TextureHandle>>,
    cache: HashMap<(usize, [i32; 4], usize, u8), Vec<Mesh>>,
}

enum Doc {
    Pdf(PdfState),
    Epub(Box<EpubState>),
}

#[derive(Clone, Copy)]
struct Turn {
    /// pliego inferior (el pliego superior es a + 1)
    a: usize,
    theta: f32,
    target: Option<f32>,
}

struct Reader {
    entry: usize,
    title: String,
    doc: Doc,
    /// página de referencia (primera visible)
    page: usize,
    /// ancla en EPUB (capítulo, bloque)
    anchor: (usize, usize),
    turn: Option<Turn>,
    show_toc: bool,
    show_settings: bool,
    opened_at: f64,
    scroll_acc: Vec2,
    last_turn: f64,
    slider: Option<f32>,
}

pub struct App {
    saved: Saved,
    reader: Option<Reader>,
    grain: Option<TextureHandle>,
    covers: CoverWorker,
    cover_tex: HashMap<String, Option<TextureHandle>>,
    requested: HashSet<String>,
    search: String,
    message: Option<(String, f64)>,
    initial: Vec<String>,
    cover_editor: Option<CoverEditor>,
}

struct CoverEditor {
    idx: usize,
    title: String,
    author: String,
    style: usize,
    custom: Option<image::DynamicImage>,
    preview: Option<TextureHandle>,
    dirty: bool,
    replacing: bool,
    /// tamaño de la página en puntos (PDF) o None (EPUB)
    page: Option<(f32, f32)>,
    error: Option<String>,
}

impl CoverEditor {
    fn size(&self, w: u32) -> (u32, u32) {
        match self.page {
            Some((pw, ph)) => (w, ((w as f32) * ph / pw).round() as u32),
            None => (w, w * 3 / 2),
        }
    }
    fn render(&self, w: u32) -> image::RgbImage {
        let (w, h) = self.size(w);
        match &self.custom {
            Some(img) => crate::cover::fit_image(img, w, h),
            None => crate::cover::design(&self.title, &self.author, self.style, w, h),
        }
    }
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, initial: Vec<String>) -> Self {
        let saved: Saved = cc
            .storage
            .and_then(|s| eframe::get_value(s, "estado"))
            .unwrap_or_default();
        cc.egui_ctx.set_theme(egui::Theme::Light);
        cc.egui_ctx
            .all_styles_mut(|s| s.spacing.button_padding = vec2(10.0, 5.0));
        App {
            saved,
            reader: None,
            grain: None,
            covers: CoverWorker::spawn(cc.egui_ctx.clone()),
            cover_tex: HashMap::new(),
            requested: HashSet::new(),
            search: String::new(),
            message: None,
            initial,
            cover_editor: None,
        }
    }

    fn add_path(&mut self, p: &Path) -> Option<usize> {
        let s = p.to_string_lossy().to_string();
        let lower = s.to_lowercase();
        let is_pdf = lower.ends_with(".pdf");
        if !is_pdf && !lower.ends_with(".epub") {
            return None;
        }
        if let Some(i) = self.saved.library.iter().position(|e| e.path == s) {
            return Some(i);
        }
        let title = p
            .file_stem()
            .map(|t| t.to_string_lossy().replace(['_', '-'], " "))
            .unwrap_or_default();
        self.saved.library.push(Entry {
            path: s,
            title,
            is_pdf,
            added: now(),
            ..Default::default()
        });
        Some(self.saved.library.len() - 1)
    }

    fn add_many(&mut self, paths: Vec<PathBuf>) {
        let before = self.saved.library.len();
        for p in paths {
            self.add_path(&p);
        }
        let n = self.saved.library.len() - before;
        self.flash(format!("{n} libros añadidos"));
    }

    fn flash(&mut self, s: String) {
        self.message = Some((s, 0.0));
    }

    fn open(&mut self, ctx: &egui::Context, idx: usize) {
        let e = &mut self.saved.library[idx];
        e.last_opened = now();
        e.opened = true;
        let path = PathBuf::from(&e.path);
        let doc = if e.is_pdf {
            match std::fs::read(&path) {
                Ok(data) => Doc::Pdf(PdfState {
                    worker: PdfWorker::spawn(data, ctx.clone()),
                    sizes: vec![],
                    tex: HashMap::new(),
                    pending: HashMap::new(),
                    zoom: 1.0,
                    pan: Vec2::ZERO,
                    tex_night: false,
                    error: None,
                }),
                Err(err) => {
                    self.flash(format!("No se puede abrir: {err}"));
                    return;
                }
            }
        } else {
            match epub::open(&path) {
                Ok(ep) => {
                    if !e.meta_done {
                        e.title = ep.title.clone();
                        e.author = ep.author.clone();
                        e.meta_done = true;
                    }
                    Doc::Epub(Box::new(EpubState {
                        epub: ep,
                        layout: None,
                        want: None,
                        images: HashMap::new(),
                        cache: HashMap::new(),
                    }))
                }
                Err(err) => {
                    self.flash(format!("No se puede abrir: {err}"));
                    return;
                }
            }
        };
        let e = &self.saved.library[idx];
        self.reader = Some(Reader {
            entry: idx,
            title: e.title.clone(),
            doc,
            page: e.page,
            anchor: (e.chapter, e.block),
            turn: None,
            show_toc: false,
            show_settings: false,
            opened_at: ctx.input(|i| i.time),
            scroll_acc: Vec2::ZERO,
            last_turn: 0.0,
            slider: None,
        });
    }

    fn open_dialog(&mut self, ctx: &egui::Context) {
        if let Some(files) = rfd::FileDialog::new()
            .add_filter("Libros", &["pdf", "epub"])
            .set_title("Abrir libro")
            .pick_files()
        {
            let mut last = None;
            for f in &files {
                last = self.add_path(f).or(last);
            }
            if files.len() == 1 {
                if let Some(i) = last {
                    self.open(ctx, i);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Pliegos

#[derive(Clone, Copy)]
struct Spreads {
    double: bool,
    cover_alone: bool,
    n: usize,
}

impl Spreads {
    fn of(&self, page: usize) -> usize {
        if !self.double {
            page
        } else if self.cover_alone {
            (page + 1) / 2
        } else {
            page / 2
        }
    }
    fn count(&self) -> usize {
        if self.n == 0 {
            0
        } else {
            self.of(self.n - 1) + 1
        }
    }
    fn pages(&self, s: usize) -> (Option<usize>, Option<usize>) {
        let ok = |p: isize| (p >= 0 && (p as usize) < self.n).then_some(p as usize);
        if !self.double {
            (None, ok(s as isize))
        } else if self.cover_alone {
            (ok(2 * s as isize - 1), ok(2 * s as isize))
        } else {
            (ok(2 * s as isize), ok(2 * s as isize + 1))
        }
    }
    fn first(&self, s: usize) -> usize {
        let (l, r) = self.pages(s);
        l.or(r).unwrap_or(0)
    }
}

/// Geometría de un pliego en pantalla.
#[derive(Clone, Copy)]
struct Geo {
    spine: f32,
    left: Option<Rect>,
    right: Option<Rect>,
}

// ---------------------------------------------------------------------------

impl eframe::App for App {
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        self.sync_entry();
        eframe::set_value(storage, "estado", &self.saved);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if self.grain.is_none() {
            let opts = TextureOptions {
                wrap_mode: egui::TextureWrapMode::Repeat,
                ..TextureOptions::LINEAR
            };
            self.grain = Some(ctx.load_texture("grano", book::grain_image(256), opts));
        }
        // archivos por línea de comandos
        for p in std::mem::take(&mut self.initial) {
            if let Some(i) = self.add_path(Path::new(&p)) {
                self.open(&ctx, i);
            }
        }
        // arrastrar y soltar
        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_path_buf())
                .collect()
        });
        if !dropped.is_empty() {
            let mut books = Vec::new();
            for p in dropped {
                if p.is_dir() && !p.to_string_lossy().to_lowercase().ends_with(".epub") {
                    covers::scan(&p, &mut books, 0);
                } else {
                    books.push(p);
                }
            }
            if books.len() == 1 {
                if let Some(i) = self.add_path(&books[0]) {
                    self.sync_entry();
                    self.open(&ctx, i);
                }
            } else {
                self.add_many(books);
            }
        }
        // resultados de portadas
        while let Ok(r) = self.covers.rx.try_recv() {
            if let Some(e) = self.saved.library.iter_mut().find(|e| e.path == r.path) {
                if let Some((t, a)) = r.meta {
                    if !t.is_empty() {
                        e.title = t;
                    }
                    if !a.is_empty() {
                        e.author = a;
                    }
                }
                e.meta_done = true;
            }
            let tex = r.image.map(|img| {
                ctx.load_texture(format!("portada:{}", r.path), img, TextureOptions::LINEAR)
            });
            self.cover_tex.insert(r.path, tex);
        }

        if self.reader.is_some() {
            self.reader_ui(ui);
        } else {
            self.library_ui(ui);
            self.cover_editor_ui(&ctx);
        }

        if let Some((msg, t0)) = &mut self.message {
            let t = ctx.input(|i| i.time);
            if *t0 == 0.0 {
                *t0 = t;
            }
            if t - *t0 > 3.0 {
                self.message = None;
            } else {
                egui::Area::new(egui::Id::NULL.with("mensaje"))
                    .anchor(Align2::CENTER_BOTTOM, vec2(0.0, -60.0))
                    .show(&ctx, |ui| {
                        egui::Frame::popup(ui.style()).show(ui, |ui| ui.label(msg.as_str()));
                    });
                ctx.request_repaint_after(std::time::Duration::from_millis(200));
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Biblioteca

impl App {
    fn library_ui(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        if ctx.theme() != egui::Theme::Light {
            ctx.set_theme(egui::Theme::Light);
        }
        let mut open_idx = None;
        let mut remove_idx = None;
        let mut create_idx = None;
        let mut do_open = false;
        let mut do_folder = false;
        let mut do_apple = false;

        egui::Panel::top("cabecera")
            .frame(
                egui::Frame::new()
                    .fill(Color32::from_rgb(250, 249, 247))
                    .inner_margin(egui::Margin::symmetric(28, 18)),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("Biblioteca")
                            .font(FontId::new(34.0, FontFamily::Name("serif-b".into())))
                            .color(Color32::from_rgb(30, 30, 30)),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .button("Importar Apple Books")
                            .on_hover_text("Añade los libros que tengas en la app Libros de macOS")
                            .clicked()
                        {
                            do_apple = true;
                        }
                        if ui.button("Añadir carpeta…").clicked() {
                            do_folder = true;
                        }
                        if ui.button("Abrir…").clicked() {
                            do_open = true;
                        }
                        ui.add(
                            egui::TextEdit::singleline(&mut self.search)
                                .hint_text("Buscar título o autor")
                                .desired_width(200.0),
                        );
                    });
                });
                let n_pdf = self.saved.library.iter().filter(|e| e.is_pdf).count();
                ui.label(
                    egui::RichText::new(format!(
                        "{} libros, {} archivos PDF",
                        self.saved.library.len() - n_pdf,
                        n_pdf
                    ))
                    .color(Color32::GRAY),
                );
            });

        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(Color32::from_rgb(250, 249, 247))
                    .inner_margin(egui::Margin::symmetric(28, 8)),
            )
            .show(ui, |ui| {
                if self.saved.library.is_empty() {
                    ui.add_space(80.0);
                    ui.vertical_centered(|ui| {
                        ui.label(
                            egui::RichText::new("Arrastra aquí tus PDF y EPUB")
                                .font(FontId::new(26.0, FontFamily::Name("serif".into())))
                                .color(Color32::GRAY),
                        );
                        ui.add_space(10.0);
                        ui.label(
                            egui::RichText::new(
                                "o usa «Abrir…», «Añadir carpeta…» o «Importar Apple Books»",
                            )
                            .color(Color32::GRAY),
                        );
                    });
                    return;
                }
                let q = self.search.to_lowercase();
                let mut order: Vec<usize> = (0..self.saved.library.len())
                    .filter(|&i| {
                        let e = &self.saved.library[i];
                        q.is_empty()
                            || e.title.to_lowercase().contains(&q)
                            || e.author.to_lowercase().contains(&q)
                    })
                    .collect();
                order.sort_by(|&a, &b| {
                    let (ea, eb) = (&self.saved.library[a], &self.saved.library[b]);
                    eb.last_opened
                        .cmp(&ea.last_opened)
                        .then(eb.added.cmp(&ea.added))
                });

                let cell = vec2(170.0, 300.0);
                let cols = ((ui.available_width() + 24.0) / (cell.x + 24.0))
                    .floor()
                    .max(1.0) as usize;
                let rows = order.len().div_ceil(cols);
                egui::ScrollArea::vertical().auto_shrink(false).show_rows(
                    ui,
                    cell.y,
                    rows,
                    |ui, range| {
                        for r in range {
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 24.0;
                                for c in 0..cols {
                                    let Some(&i) = order.get(r * cols + c) else {
                                        break;
                                    };
                                    let (resp, want_cover) = self.book_cell(ui, i, cell);
                                    if want_cover {
                                        create_idx = Some(i);
                                    } else if resp.clicked() {
                                        open_idx = Some(i);
                                    }
                                    let has_cover = matches!(
                                        self.cover_tex.get(&self.saved.library[i].path),
                                        Some(Some(_))
                                    );
                                    resp.context_menu(|ui| {
                                        if ui.button("Abrir").clicked() {
                                            open_idx = Some(i);
                                        }
                                        #[cfg(target_os = "macos")]
                                        if ui.button("Mostrar en Finder").clicked() {
                                            let _ = std::process::Command::new("open")
                                                .arg("-R")
                                                .arg(&self.saved.library[i].path)
                                                .spawn();
                                        }
                                        if ui
                                            .button(if self.saved.library[i].is_pdf {
                                                "Añadir portada…"
                                            } else if has_cover {
                                                "Cambiar portada…"
                                            } else {
                                                "Crear portada…"
                                            })
                                            .clicked()
                                        {
                                            create_idx = Some(i);
                                            ui.close();
                                        }
                                        if ui.button("Volver a buscar la portada").clicked() {
                                            let path = self.saved.library[i].path.clone();
                                            self.requested.remove(&path);
                                            self.cover_tex.remove(&path);
                                            ui.close();
                                        }
                                        if ui.button("Quitar de la biblioteca").clicked() {
                                            remove_idx = Some(i);
                                        }
                                    });
                                }
                            });
                        }
                    },
                );
            });

        if do_open {
            self.open_dialog(&ctx);
        }
        if do_folder {
            if let Some(dir) = rfd::FileDialog::new()
                .set_title("Añadir carpeta")
                .pick_folder()
            {
                let mut v = Vec::new();
                covers::scan(&dir, &mut v, 0);
                self.add_many(v);
            }
        }
        if do_apple {
            let mut v = Vec::new();
            for d in covers::apple_books_dirs() {
                covers::scan(&d, &mut v, 0);
            }
            if v.is_empty() {
                self.flash(
                    "No se encontraron libros de Apple Books (puede que macOS pida permiso)".into(),
                );
            } else {
                self.add_many(v);
            }
        }
        if let Some(i) = create_idx {
            let e = &self.saved.library[i];
            let page = if e.is_pdf {
                Some(crate::pdfdoc::first_page_size(&e.path).unwrap_or((595.0, 842.0)))
            } else {
                None
            };
            self.cover_editor = Some(CoverEditor {
                idx: i,
                title: e.title.clone(),
                author: e.author.clone(),
                style: 0,
                custom: None,
                preview: None,
                dirty: true,
                replacing: matches!(self.cover_tex.get(&e.path), Some(Some(_))),
                page,
                error: None,
            });
        }
        if let Some(i) = remove_idx {
            self.saved.library.remove(i);
        } else if let Some(i) = open_idx {
            self.open(&ctx, i);
        }
    }

    fn book_cell(&mut self, ui: &mut egui::Ui, i: usize, cell: Vec2) -> (egui::Response, bool) {
        let (rect, resp) = ui.allocate_exact_size(cell, Sense::click());
        if !ui.is_rect_visible(rect) {
            return (resp, false);
        }
        let mut want_cover = false;
        let e = &self.saved.library[i];
        if !self.requested.contains(&e.path) {
            self.requested.insert(e.path.clone());
            self.covers.request(&e.path, !e.meta_done);
        }
        let p = ui.painter();
        let cover_area = Rect::from_min_size(rect.min, vec2(cell.x, 240.0));
        let tex = self.cover_tex.get(&e.path).and_then(|t| t.as_ref());
        let crect = match tex {
            Some(t) => {
                let s = t.size_vec2();
                let k = (cover_area.width() / s.x).min(cover_area.height() / s.y);
                Rect::from_min_size(
                    pos2(cover_area.left(), cover_area.bottom() - s.y * k),
                    s * k,
                )
            }
            None => Rect::from_min_size(
                pos2(cover_area.left(), cover_area.bottom() - 228.0),
                vec2(cell.x, 228.0),
            ),
        };
        let lift = if resp.hovered() { -3.0 } else { 0.0 };
        let crect = crect.translate(vec2(0.0, lift));
        book::book_shadow(p, crect.shrink(4.0), false);
        match tex {
            Some(t) => {
                p.image(
                    t.id(),
                    crect,
                    Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                    Color32::WHITE,
                );
            }
            None => {
                let hue = (e.title.bytes().map(|b| b as u32).sum::<u32>() % 6) as usize;
                let cols = [
                    (120, 60, 50),
                    (40, 70, 110),
                    (60, 100, 70),
                    (110, 90, 50),
                    (80, 60, 110),
                    (50, 90, 100),
                ];
                let (r, g, b) = cols[hue];
                p.rect_filled(crect, 2.0, Color32::from_rgb(r, g, b));
                let galley = p.layout(
                    e.title.clone(),
                    FontId::new(17.0, FontFamily::Name("serif-b".into())),
                    Color32::from_rgb(245, 238, 220),
                    crect.width() - 24.0,
                );
                p.galley(crect.min + vec2(12.0, 22.0), galley, Color32::WHITE);
            }
        }
        // lomo
        p.add(book::hgrad(
            Rect::from_min_size(crect.min, vec2(7.0, crect.height())),
            Color32::from_black_alpha(70),
            Color32::TRANSPARENT,
        ));
        let mut title = e.title.clone();
        if title.chars().count() > 26 {
            title = title.chars().take(24).collect::<String>() + "…";
        }
        p.text(
            pos2(rect.left(), cover_area.bottom() + 10.0),
            Align2::LEFT_TOP,
            title,
            FontId::proportional(13.5),
            Color32::from_rgb(30, 30, 30),
        );
        let sub = if !e.opened {
            None
        } else if e.progress >= 0.995 {
            Some("Terminado".to_string())
        } else {
            Some(format!("{:.0} %", (e.progress * 100.0).max(1.0)))
        };
        let y = cover_area.bottom() + 32.0;
        match sub {
            None => {
                let r = Rect::from_min_size(pos2(rect.left(), y), vec2(52.0, 18.0));
                p.rect_filled(r, 9.0, Color32::from_rgb(20, 60, 140));
                p.text(
                    r.center(),
                    Align2::CENTER_CENTER,
                    "NUEVO",
                    FontId::proportional(11.0),
                    Color32::WHITE,
                );
            }
            Some(s) => {
                p.text(
                    pos2(rect.left(), y),
                    Align2::LEFT_TOP,
                    s,
                    FontId::proportional(12.0),
                    Color32::GRAY,
                );
            }
        }
        if e.is_pdf {
            p.text(
                pos2(rect.right(), y),
                Align2::RIGHT_TOP,
                "PDF",
                FontId::proportional(11.0),
                Color32::GRAY,
            );
        }
        // sin portada: botón para crearla
        let no_cover = matches!(self.cover_tex.get(&e.path), Some(None));
        if no_cover && resp.hovered() {
            let b = Rect::from_center_size(
                pos2(crect.center().x, crect.bottom() - 26.0),
                vec2(124.0, 28.0),
            );
            p.rect_filled(b, 14.0, Color32::from_rgb(20, 90, 200));
            p.text(
                b.center(),
                Align2::CENTER_CENTER,
                "Crear portada",
                FontId::proportional(13.0),
                Color32::WHITE,
            );
            if resp.clicked()
                && resp
                    .interact_pointer_pos()
                    .is_some_and(|q| b.expand(4.0).contains(q))
            {
                want_cover = true;
            }
        }
        (
            resp.on_hover_text(format!("{}\n{}", e.title, e.author)),
            want_cover,
        )
    }

    fn sync_entry(&mut self) {
        let Some(r) = &self.reader else { return };
        let total = match &r.doc {
            Doc::Pdf(p) => p.sizes.len(),
            Doc::Epub(e) => e.layout.as_ref().map(|l| l.pages.len()).unwrap_or(0),
        };
        if let Some(e) = self.saved.library.get_mut(r.entry) {
            e.page = r.page;
            e.chapter = r.anchor.0;
            e.block = r.anchor.1;
            if total > 0 {
                e.progress = ((r.page + 1) as f32 / total as f32).min(1.0);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Lector

fn tint_for(theme: usize) -> Color32 {
    match theme {
        1 => Color32::from_rgb(250, 236, 208),
        2 => Color32::from_rgb(222, 222, 218),
        3 => Color32::from_rgb(205, 200, 192),
        _ => Color32::WHITE,
    }
}

impl App {
    fn reader_ui(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let settings = self.saved.settings.clone();
        let th: Theme = book::theme(settings.theme);
        let want_theme = if th.night {
            egui::Theme::Dark
        } else {
            egui::Theme::Light
        };
        if ctx.theme() != want_theme {
            ctx.set_theme(want_theme);
        }
        let grain = self.grain.as_ref().map(|g| g.id()).unwrap_or_default();
        let ppp = ctx.pixels_per_point();
        let time = ctx.input(|i| i.time);
        let dt = ctx.input(|i| i.stable_dt).min(0.05);

        let mut back_to_library = false;
        let mut toggle_bookmark = false;
        let mut jump: Option<usize> = None;
        let mut jump_anchor: Option<(usize, usize)> = None;

        let reader = self.reader.as_mut().unwrap();
        let entry = &self.saved.library[reader.entry];
        let bookmarks = entry.bookmarks.clone();

        // ---- mensajes del PDF
        if let Doc::Pdf(pdf) = &mut reader.doc {
            if pdf.tex_night != th.night {
                pdf.tex.clear();
                pdf.pending.clear();
                pdf.tex_night = th.night;
            }
            while let Ok(m) = pdf.worker.rx.try_recv() {
                match m {
                    Msg::Info { sizes, title } => {
                        pdf.sizes = sizes;
                        if !title.is_empty() && !self.saved.library[reader.entry].meta_done {
                            self.saved.library[reader.entry].title = title.clone();
                            reader.title = title;
                        }
                        reader.page = reader.page.min(pdf.sizes.len().saturating_sub(1));
                    }
                    Msg::Page(r) => {
                        pdf.pending.remove(&r.page);
                        let mut rgba = r.rgba;
                        if th.night {
                            for px in rgba.chunks_exact_mut(4) {
                                // invierte manteniendo tonos
                                let l = 255
                                    - ((px[0] as u32 * 3 + px[1] as u32 * 6 + px[2] as u32) / 10)
                                        as u8;
                                let k =
                                    |c: u8| ((255 - c) as u16 * 3 / 10 + l as u16 * 7 / 10) as u8;
                                px[0] = k(px[0]);
                                px[1] = k(px[1]);
                                px[2] = k(px[2]);
                            }
                        }
                        let img =
                            egui::ColorImage::from_rgba_premultiplied([r.width, r.height], &rgba);
                        let t =
                            ctx.load_texture(format!("pdf{}", r.page), img, TextureOptions::LINEAR);
                        pdf.tex.insert(r.page, (t, r.scale));
                    }
                    Msg::Error(e) => pdf.error = Some(e),
                }
            }
        }

        let full = ui.max_rect();
        let painter = ui.painter_at(full);
        book::desk(&painter, full, &th);

        // ---- área útil y modo de página
        let area = full.shrink2(vec2(24.0, 36.0));
        let double = match settings.page_mode {
            1 => false,
            2 => true,
            _ => area.width() > area.height() * 1.15,
        };

        // ---- EPUB: paginación
        let is_pdf = matches!(reader.doc, Doc::Pdf(_));
        let epub_page = |area: Rect, double: bool| -> Vec2 {
            let mut ph = area.height();
            let mut pw = ph * 0.68;
            let maxw = if double {
                area.width() / 2.0
            } else {
                area.width()
            };
            if pw > maxw {
                pw = maxw;
                ph = (pw / 0.68).min(area.height());
            }
            vec2(pw, ph)
        };
        let margins = |size: Vec2| -> (f32, f32, f32) {
            // (lateral, superior, inferior)
            (size.x * settings.margin, size.y * 0.085, size.y * 0.095)
        };
        if let Doc::Epub(ep) = &mut reader.doc {
            let size = epub_page(area, double);
            let (mx, mt, mb) = margins(size);
            let params = Params {
                width: (size.x - 2.0 * mx).round(),
                height: (size.y - mt - mb).round(),
                font_size: settings.font_size,
                line_spacing: settings.line_spacing,
                sans: settings.sans,
                justify: settings.justify,
                ppp,
            };
            let need = ep
                .layout
                .as_ref()
                .map(|l| l.params != params)
                .unwrap_or(true);
            if need {
                let ready = match &ep.want {
                    _ if ep.layout.is_none() => true,
                    Some((p, t)) if *p == params => time - *t > 0.25,
                    _ => {
                        ep.want = Some((params, time));
                        false
                    }
                };
                if ready {
                    let t0 = std::time::Instant::now();
                    let l = layout::paginate(&mut ep.epub, &ctx, params);
                    #[cfg(debug_assertions)]
                    eprintln!(
                        "maquetación: {} páginas en {:?}",
                        l.pages.len(),
                        t0.elapsed()
                    );
                    let _ = t0;
                    reader.page = l.page_of(reader.anchor.0, reader.anchor.1);
                    ep.layout = Some(l);
                    ep.cache.clear();
                    ep.want = None;
                } else {
                    ctx.request_repaint_after(std::time::Duration::from_millis(100));
                }
            }
        }

        let n_pages = match &reader.doc {
            Doc::Pdf(p) => p.sizes.len(),
            Doc::Epub(e) => e.layout.as_ref().map(|l| l.pages.len()).unwrap_or(0),
        };
        if n_pages == 0 {
            let msg = match &reader.doc {
                Doc::Pdf(p) => p.error.clone().unwrap_or_else(|| "Abriendo…".into()),
                _ => "Maquetando…".into(),
            };
            painter.text(
                full.center(),
                Align2::CENTER_CENTER,
                msg,
                FontId::proportional(20.0),
                Color32::from_gray(220),
            );
            if ctx.input(|i| i.key_pressed(Key::Escape))
                || matches!(&reader.doc, Doc::Pdf(p) if p.error.is_some())
                    && ctx.input(|i| i.pointer.any_click())
            {
                self.reader = None;
            }
            return;
        }
        reader.page = reader.page.min(n_pages - 1);
        let sp = Spreads {
            double,
            cover_alone: settings.cover_alone,
            n: n_pages,
        };
        let cur_spread = sp.of(reader.page);

        // ---- geometría
        let zoom = if let Doc::Pdf(p) = &reader.doc {
            p.zoom
        } else {
            1.0
        };
        let pan = if let Doc::Pdf(p) = &reader.doc {
            p.pan
        } else {
            Vec2::ZERO
        };
        let pdf_sizes: Option<Vec<(f32, f32)>> = match &reader.doc {
            Doc::Pdf(p) => Some(p.sizes.clone()),
            _ => None,
        };
        let geo = |s: usize| -> Geo {
            let (l, r) = sp.pages(s);
            let size_of = |p: usize| -> Vec2 {
                match &pdf_sizes {
                    Some(sz) => vec2(sz[p].0, sz[p].1),
                    None => epub_page(area, double),
                }
            };
            let sl = l.map(size_of);
            let sr = r.map(size_of);
            let asp = |s: Option<Vec2>, o: Option<Vec2>| -> f32 {
                let v = s.or(o).unwrap_or(vec2(0.7, 1.0));
                v.x / v.y
            };
            let (al, ar) = (asp(sl, sr), asp(sr, sl));
            let c = area.center() + pan;
            if double {
                let h = area.height().min(area.width() / 2.0 / al.max(ar)) * zoom;
                let spine = c.x;
                let top = c.y - h / 2.0;
                Geo {
                    spine,
                    left: l
                        .map(|_| Rect::from_min_size(pos2(spine - al * h, top), vec2(al * h, h))),
                    right: r.map(|_| Rect::from_min_size(pos2(spine, top), vec2(ar * h, h))),
                }
            } else {
                let h = area.height().min(area.width() / ar) * zoom;
                let w = ar * h;
                let left = c.x - w / 2.0;
                Geo {
                    spine: left,
                    left: None,
                    right: r.map(|_| Rect::from_min_size(pos2(left, c.y - h / 2.0), vec2(w, h))),
                }
            }
        };
        let g = geo(cur_spread);

        // ---- PDF: peticiones de renderizado
        if let Doc::Pdf(pdf) = &mut reader.doc {
            let mut want: Vec<(usize, f32)> = Vec::new();
            let mut push = |p: Option<usize>, rect_h: f32, pdf: &PdfState| {
                if let Some(p) = p {
                    let (w, h) = pdf.sizes[p];
                    let mut s = rect_h * ppp / h;
                    s = s.min(5000.0 / w.max(h)).max(0.05);
                    if !want.iter().any(|(q, _)| *q == p) {
                        want.push((p, s));
                    }
                }
            };
            let page_h = g.left.or(g.right).map(|r| r.height()).unwrap_or(800.0);
            let (l, r) = sp.pages(cur_spread);
            push(l, page_h, pdf);
            push(r, page_h, pdf);
            for s in [cur_spread + 1, cur_spread.saturating_sub(1), cur_spread + 2] {
                if s < sp.count() {
                    let (l, r) = sp.pages(s);
                    push(l, page_h / zoom, pdf);
                    push(r, page_h / zoom, pdf);
                }
            }
            let list: Vec<(usize, f32)> = want
                .iter()
                .filter(|(p, s)| {
                    let have = pdf.tex.get(p).map(|t| t.1);
                    let pend = pdf.pending.get(p).copied();
                    let ok = |v: Option<f32>| {
                        v.is_some_and(|v| (v / s - 1.0).abs() < 0.2 || (v > *s && v / s < 1.6))
                    };
                    !ok(have) && !ok(pend)
                })
                .copied()
                .collect();
            if !list.is_empty() {
                for (p, s) in &list {
                    pdf.pending.insert(*p, *s);
                }
                pdf.worker.request(list);
            }
            // liberar texturas lejanas
            let keep: HashSet<usize> = want.iter().map(|w| w.0).collect();
            if pdf.tex.len() > 10 {
                pdf.tex.retain(|k, _| keep.contains(k));
            }
        }

        // ---- entrada
        let resp = ui.interact(
            area.expand(20.0),
            ui.scope_id().with("libro"),
            Sense::click_and_drag(),
        );
        let input = ctx.input(|i| i.clone());
        let pointer = input.pointer.interact_pos();
        let can_next = cur_spread + 1 < sp.count();
        let can_prev = cur_spread > 0;
        let zoomed = zoom > 1.01;
        let typing = ctx.egui_wants_keyboard_input();

        // termina un giro en curso si llega otra orden
        let finish = |reader: &mut Reader| {
            if let Some(t) = reader.turn.take() {
                let s = if t.target.unwrap_or(t.theta) > 1.5 {
                    t.a + 1
                } else {
                    t.a
                };
                reader.page = sp.first(s);
            }
        };
        let start_turn = |reader: &mut Reader, forward: bool| {
            finish(reader);
            let s = sp.of(reader.page);
            if forward && s + 1 < sp.count() {
                if settings.animate {
                    reader.turn = Some(Turn {
                        a: s,
                        theta: 0.0,
                        target: Some(std::f32::consts::PI),
                    });
                } else {
                    reader.page = sp.first(s + 1);
                }
            } else if !forward && s > 0 {
                if settings.animate {
                    reader.turn = Some(Turn {
                        a: s - 1,
                        theta: std::f32::consts::PI,
                        target: Some(0.0),
                    });
                } else {
                    reader.page = sp.first(s - 1);
                }
            }
            reader.last_turn = time;
        };

        if !typing {
            if input.key_pressed(Key::ArrowRight)
                || input.key_pressed(Key::Space)
                || input.key_pressed(Key::PageDown)
            {
                start_turn(reader, true);
            }
            if input.key_pressed(Key::ArrowLeft)
                || input.key_pressed(Key::PageUp)
                || input.key_pressed(Key::Backspace)
            {
                start_turn(reader, false);
            }
            if input.key_pressed(Key::Home) {
                finish(reader);
                reader.page = 0;
            }
            if input.key_pressed(Key::End) {
                finish(reader);
                reader.page = n_pages - 1;
            }
            if input.key_pressed(Key::Escape) {
                if input.viewport().fullscreen.unwrap_or(false) {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
                } else if reader.show_settings || reader.show_toc {
                    reader.show_settings = false;
                    reader.show_toc = false;
                } else {
                    back_to_library = true;
                }
            }
            if input.key_pressed(Key::F) {
                let fs = input.viewport().fullscreen.unwrap_or(false);
                ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(!fs));
            }
            if input.key_pressed(Key::T) {
                reader.show_toc = !reader.show_toc;
            }
            if input.key_pressed(Key::B) {
                toggle_bookmark = true;
            }
            let plus = input.modifiers.command
                && (input.key_pressed(Key::Plus) || input.key_pressed(Key::Equals));
            let minus = input.modifiers.command && input.key_pressed(Key::Minus);
            if plus || minus {
                match &mut reader.doc {
                    Doc::Epub(_) => {
                        let f = &mut self.saved.settings.font_size;
                        *f = (*f + if plus { 1.0 } else { -1.0 }).clamp(12.0, 40.0);
                    }
                    Doc::Pdf(p) => {
                        p.zoom = (p.zoom * if plus { 1.25 } else { 0.8 }).clamp(1.0, 6.0);
                        if p.zoom <= 1.01 {
                            p.pan = Vec2::ZERO;
                        }
                    }
                }
            }
        }

        // zoom con pellizco / ctrl+rueda (PDF)
        if let Doc::Pdf(p) = &mut reader.doc {
            let zd = input.zoom_delta();
            if (zd - 1.0).abs() > 0.001 && resp.hovered() {
                let old = p.zoom;
                p.zoom = (p.zoom * zd).clamp(1.0, 6.0);
                if let Some(ptr) = pointer {
                    let c = area.center() + p.pan;
                    p.pan += (ptr - c) * (1.0 - p.zoom / old);
                }
                if p.zoom <= 1.01 {
                    p.pan = Vec2::ZERO;
                }
            }
            if resp.double_clicked() {
                if p.zoom > 1.01 {
                    p.zoom = 1.0;
                    p.pan = Vec2::ZERO;
                } else if let Some(ptr) = pointer {
                    p.zoom = 2.2;
                    p.pan = (area.center() - ptr) * 1.2;
                }
            }
            if p.zoom > 1.01 {
                if resp.dragged() {
                    p.pan += resp.drag_delta();
                }
                p.pan += input.smooth_scroll_delta;
            }
        }

        if !zoomed {
            // rueda / gesto de dos dedos
            if resp.hovered() && reader.turn.is_none() {
                reader.scroll_acc += input.smooth_scroll_delta;
                if input.smooth_scroll_delta == Vec2::ZERO && input.time - reader.last_turn > 0.3 {
                    reader.scroll_acc *= 0.8;
                }
                if time - reader.last_turn > 0.35 {
                    let a = reader.scroll_acc;
                    let v = if a.x.abs() > a.y.abs() { a.x } else { a.y };
                    if v < -70.0 {
                        start_turn(reader, true);
                        reader.scroll_acc = Vec2::ZERO;
                    } else if v > 70.0 {
                        start_turn(reader, false);
                        reader.scroll_acc = Vec2::ZERO;
                    }
                }
            }
            // arrastrar la esquina
            if resp.drag_started() {
                if let Some(ptr) = resp.interact_pointer_pos() {
                    finish(reader);
                    let s = sp.of(reader.page);
                    let g = geo(s);
                    let on_right = ptr.x
                        > g.spine
                            + if double {
                                0.0
                            } else {
                                g.right.map(|r| r.width() * 0.4).unwrap_or(0.0)
                            };
                    if on_right && s + 1 < sp.count() {
                        reader.turn = Some(Turn {
                            a: s,
                            theta: 0.15,
                            target: None,
                        });
                    } else if !on_right && s > 0 {
                        reader.turn = Some(Turn {
                            a: s - 1,
                            theta: std::f32::consts::PI - 0.15,
                            target: None,
                        });
                    }
                }
            }
            if let Some(t) = reader.turn.as_mut() {
                if t.target.is_none() {
                    if let Some(ptr) = pointer {
                        let gw = geo(t.a);
                        let w = gw.right.map(|r| r.width()).unwrap_or(400.0);
                        t.theta = ((ptr.x - gw.spine) / w)
                            .clamp(-1.0, 1.0)
                            .acos()
                            .clamp(0.02, std::f32::consts::PI - 0.02);
                    }
                    if !resp.dragged() {
                        let vx = input.pointer.velocity().x;
                        let pi = std::f32::consts::PI;
                        t.target = Some(if vx < -250.0 {
                            pi
                        } else if vx > 250.0 {
                            0.0
                        } else if t.theta > pi / 2.0 {
                            pi
                        } else {
                            0.0
                        });
                        reader.last_turn = time;
                    }
                }
            }
            if resp.clicked() && reader.turn.is_none() {
                if let Some(ptr) = resp.interact_pointer_pos() {
                    let near_edge = ptr.y < full.top() + 80.0 || ptr.y > full.bottom() - 80.0;
                    if !near_edge {
                        let fwd = if double {
                            ptr.x > g.spine
                        } else {
                            ptr.x > area.left() + area.width() * 0.35
                        };
                        if fwd && can_next {
                            start_turn(reader, true);
                        } else if !fwd && can_prev {
                            start_turn(reader, false);
                        }
                    }
                }
            }
        }

        // ---- animación
        if let Some(t) = reader.turn.as_mut() {
            if let Some(target) = t.target {
                let d = target - t.theta;
                let step = d * (1.0 - (-8.0 * dt).exp()) + d.signum() * dt * 1.5;
                if d.abs() <= step.abs() || d.abs() < 0.004 {
                    let s = if target > 1.5 { t.a + 1 } else { t.a };
                    reader.page = sp.first(s);
                    reader.turn = None;
                } else {
                    t.theta += step;
                }
            }
            ctx.request_repaint();
        }

        // ---- ancla EPUB
        if let Doc::Epub(ep) = &reader.doc {
            if reader.turn.is_none() {
                if let Some(l) = &ep.layout {
                    let s = sp.of(reader.page);
                    let p = sp.first(s);
                    if let Some(pg) = l.pages.get(p) {
                        if l.page_of(reader.anchor.0, reader.anchor.1) != p {
                            reader.anchor = (pg.chapter, pg.block);
                        }
                    }
                }
            }
        }

        // ---- dibujo
        let cur_spread = sp.of(reader.page);
        let (statics_l, statics_r, sheet): (
            Option<(usize, Rect)>,
            Option<(usize, Rect)>,
            Option<(Turn, Option<(usize, Rect)>, Option<(usize, Rect)>, Geo)>,
        ) = match reader.turn {
            Some(t) => {
                let ga = geo(t.a);
                let gb = geo(t.a + 1);
                let (la, ra) = sp.pages(t.a);
                let (lb, rb) = sp.pages(t.a + 1);
                (
                    la.zip(ga.left),
                    rb.zip(gb.right),
                    Some((t, ra.zip(ga.right), lb.zip(gb.left), ga)),
                )
            }
            None => {
                let (l, r) = sp.pages(cur_spread);
                (l.zip(g.left), r.zip(g.right), None)
            }
        };

        // sombra y canto
        let mut bounds: Option<Rect> = None;
        let mut add_b = |r: Rect| bounds = Some(bounds.map_or(r, |b| b.union(r)));
        if let Some((_, r)) = statics_l {
            add_b(r);
        }
        if let Some((_, r)) = statics_r {
            add_b(r);
        }
        if let Some((_, a, b, _)) = &sheet {
            if let Some((_, r)) = a {
                add_b(*r);
            }
            if let Some((_, r)) = b {
                add_b(*r);
            }
        }
        let progress_page = reader.page;
        if let Some(b) = bounds {
            book::book_shadow(&painter, b, th.night);
            let before = ((progress_page as f32 / n_pages as f32) * 7.0).ceil() as usize;
            let after =
                (((n_pages - 1 - progress_page) as f32 / n_pages as f32) * 7.0).ceil() as usize;
            if double {
                if let Some((_, r)) = statics_l {
                    book::page_stack(&painter, r, Side::Left, before, &th);
                }
                if let Some((_, r)) = statics_r {
                    book::page_stack(&painter, r, Side::Right, after, &th);
                }
            } else if let Some((_, r)) = statics_r {
                book::page_stack(&painter, r, Side::Right, after, &th);
            }
        }

        // mallas de una página
        let title = reader.title.clone();
        let theme_idx = settings.theme;
        let page_meshes = |reader: &mut Reader, page: usize, rect: Rect, side: Side| -> Vec<Mesh> {
            let mut out = Vec::new();
            match &mut reader.doc {
                Doc::Pdf(pdf) => {
                    let tint = tint_for(theme_idx);
                    out.push(book::strip(
                        rect,
                        side,
                        egui::TextureId::default(),
                        Rect::ZERO,
                        if th.night {
                            Color32::from_gray(28)
                        } else {
                            tint
                        },
                        th.night,
                        true,
                        24,
                    ));
                    if let Some((t, _)) = pdf.tex.get(&page) {
                        out.push(book::strip(
                            rect,
                            side,
                            t.id(),
                            Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                            tint,
                            th.night,
                            true,
                            24,
                        ));
                    }
                }
                Doc::Epub(ep) => {
                    let key = (
                        page,
                        [
                            rect.min.x as i32,
                            rect.min.y as i32,
                            rect.width() as i32,
                            rect.height() as i32,
                        ],
                        theme_idx,
                        side as u8,
                    );
                    if let Some(m) = ep.cache.get(&key) {
                        return m.clone();
                    }
                    out.push(book::strip(
                        rect,
                        side,
                        egui::TextureId::default(),
                        Rect::ZERO,
                        th.paper,
                        th.night,
                        true,
                        24,
                    ));
                    let k = th.grain;
                    let uv = Rect::from_min_size(
                        pos2(rect.min.x / 256.0, rect.min.y / 256.0),
                        rect.size() / 256.0,
                    );
                    out.push(book::strip(
                        rect,
                        side,
                        grain,
                        uv,
                        Color32::from_rgba_premultiplied(k, k, k, k),
                        th.night,
                        false,
                        24,
                    ));
                    let Some(l) = &ep.layout else { return out };
                    let Some(pg) = l.pages.get(page) else {
                        return out;
                    };
                    let (mx, mt, _mb) = margins(rect.size());
                    let text = Rect::from_min_size(
                        rect.min + vec2(mx, mt),
                        vec2(l.params.width, l.params.height),
                    );
                    let mut shapes: Vec<(Rect, Shape)> = Vec::new();
                    for it in &pg.items {
                        match it {
                            Item::Text {
                                galley,
                                offset,
                                clip,
                            } => {
                                let clip_r = Rect::from_x_y_ranges(
                                    rect.x_range(),
                                    (text.top() + clip.0 - 1.0)..=(text.top() + clip.1 + 1.0),
                                );
                                let ts = egui::epaint::TextShape::new(
                                    text.min + *offset,
                                    galley.clone(),
                                    th.ink,
                                )
                                .with_override_text_color(th.ink);
                                shapes.push((clip_r, Shape::Text(ts)));
                            }
                            Item::Image { path, rect: ir } => {
                                let tex = ep.images.entry(path.clone()).or_insert_with(|| {
                                    let bytes = ep.epub.image_bytes(path)?;
                                    let img = image::load_from_memory(&bytes).ok()?;
                                    let img = if img.width() > 2400 {
                                        img.resize(
                                            2400,
                                            4000,
                                            image::imageops::FilterType::Triangle,
                                        )
                                    } else {
                                        img
                                    };
                                    let rgba = img.to_rgba8();
                                    let ci = egui::ColorImage::from_rgba_unmultiplied(
                                        [rgba.width() as usize, rgba.height() as usize],
                                        rgba.as_raw(),
                                    );
                                    Some(ctx.load_texture(path.clone(), ci, TextureOptions::LINEAR))
                                });
                                if let Some(t) = tex {
                                    let r = ir.translate(text.min.to_vec2());
                                    let c = if th.night {
                                        Color32::from_gray(190)
                                    } else {
                                        Color32::WHITE
                                    };
                                    shapes.push((
                                        rect,
                                        Shape::image(
                                            t.id(),
                                            r,
                                            Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                                            c,
                                        ),
                                    ));
                                }
                            }
                            Item::Ornament { y } => {
                                let galley = ctx.fonts_mut(|f| {
                                    f.layout_no_wrap(
                                        "⁂".into(),
                                        FontId::new(
                                            settings.font_size,
                                            FontFamily::Name("serif".into()),
                                        ),
                                        th.faint,
                                    )
                                });
                                let pos =
                                    pos2(text.center().x - galley.size().x / 2.0, text.top() + y);
                                shapes.push((rect, Shape::galley(pos, galley, th.faint)));
                            }
                        }
                    }
                    // cabecera y folio
                    let small = FontId::new(
                        (settings.font_size * 0.68).max(11.0),
                        FontFamily::Name("serif-i".into()),
                    );
                    if !pg.plain {
                        let head = if side == Side::Left || !double {
                            title.clone()
                        } else {
                            ep.epub
                                .chapter_title(pg.chapter)
                                .unwrap_or(&title)
                                .to_string()
                        };
                        let head = if head.chars().count() > 60 {
                            head.chars().take(58).collect::<String>() + "…"
                        } else {
                            head
                        };
                        let galley =
                            ctx.fonts_mut(|f| f.layout_no_wrap(head, small.clone(), th.faint));
                        let pos = pos2(
                            text.center().x - galley.size().x / 2.0,
                            rect.top() + mt * 0.42,
                        );
                        shapes.push((rect, Shape::galley(pos, galley, th.faint)));
                    }
                    let num = FontId::new(
                        (settings.font_size * 0.72).max(11.0),
                        FontFamily::Name("serif".into()),
                    );
                    let galley =
                        ctx.fonts_mut(|f| f.layout_no_wrap(format!("{}", page + 1), num, th.faint));
                    let pos = pos2(
                        text.center().x - galley.size().x / 2.0,
                        rect.bottom() - margins(rect.size()).2 * 0.55,
                    );
                    shapes.push((rect, Shape::galley(pos, galley, th.faint)));
                    out.extend(book::shapes_to_meshes(&ctx, shapes, ppp));
                    if ep.cache.len() > 12 {
                        ep.cache.clear();
                    }
                    ep.cache.insert(key, out.clone());
                }
            }
            out
        };

        let paint = |meshes: Vec<Mesh>| {
            for m in meshes {
                painter.add(Shape::mesh(m));
            }
        };
        let is_marked = |p: usize, reader: &Reader| -> bool {
            match &reader.doc {
                Doc::Pdf(_) => bookmarks.contains(&(p, 0)),
                Doc::Epub(ep) => {
                    ep.layout
                        .as_ref()
                        .and_then(|l| l.pages.get(p))
                        .is_some_and(|pg| {
                            bookmarks
                                .iter()
                                .any(|b| l_page(ep, *b) == Some(p) || *b == (pg.chapter, pg.block))
                        })
                }
            }
        };

        if let Some((p, r)) = statics_l {
            let m = page_meshes(reader, p, r, Side::Left);
            paint(m);
            if is_marked(p, reader) {
                book::ribbon(&painter, r);
            }
        }
        if let Some((p, r)) = statics_r {
            let m = page_meshes(reader, p, r, Side::Right);
            paint(m);
            if is_marked(p, reader) {
                book::ribbon(&painter, r);
            }
        }
        if let Some((t, front, backp, ga)) = sheet {
            let w_front = front
                .map(|f| f.1.width())
                .or(backp.map(|b| b.1.width()))
                .unwrap_or(300.0);
            let h_ref = front.map(|f| f.1).or(backp.map(|b| b.1)).unwrap();
            let flip_f = Flip {
                spine: ga.spine,
                width: w_front,
                cy: h_ref.center().y,
                theta: t.theta,
            };
            let w_back = backp.map(|b| b.1.width()).unwrap_or(w_front);
            let flip_b = Flip {
                width: w_back,
                ..flip_f
            };
            let pi = std::f32::consts::PI;
            let s = t.theta.sin();
            // sombra proyectada sobre la página de debajo
            if t.theta < pi / 2.0 {
                if let Some((_, r)) = statics_r {
                    let ex = flip_f.edge_x();
                    let sw = r.width() * 0.35 * s;
                    painter.with_clip_rect(r).add(book::hgrad(
                        Rect::from_x_y_ranges(ex..=ex + sw.max(1.0), r.y_range()),
                        Color32::from_black_alpha((110.0 * s) as u8),
                        Color32::TRANSPARENT,
                    ));
                }
            } else if let Some((_, r)) = statics_l {
                let ex = flip_b.edge_x();
                let sw = r.width() * 0.35 * s;
                painter.with_clip_rect(r).add(book::hgrad(
                    Rect::from_x_y_ranges(ex - sw.max(1.0)..=ex, r.y_range()),
                    Color32::TRANSPARENT,
                    Color32::from_black_alpha((110.0 * s) as u8),
                ));
            }
            if t.theta < pi / 2.0 {
                if let Some((p, r)) = front {
                    let m = page_meshes(reader, p, r, Side::Right);
                    paint(flip_f.transform(&m, false));
                }
            } else {
                match backp {
                    Some((p, r)) => {
                        let m = page_meshes(reader, p, r, Side::Left);
                        paint(flip_b.transform(&m, true));
                    }
                    None => {
                        // reverso en blanco (modo una página o contraportada)
                        let r = Rect::from_min_size(
                            pos2(ga.spine - w_front, h_ref.top()),
                            h_ref.size(),
                        );
                        let m = vec![book::strip(
                            r,
                            Side::Left,
                            egui::TextureId::default(),
                            Rect::ZERO,
                            th.paper,
                            th.night,
                            true,
                            24,
                        )];
                        paint(flip_b.transform(&m, true));
                    }
                }
            }
        }
        // sombra central del lomo (sobre ambas páginas)
        if double && reader.turn.is_none() {
            if let (Some(l), Some(r)) = (g.left, g.right) {
                let w = 10.0;
                painter.add(book::hgrad(
                    Rect::from_x_y_ranges(
                        g.spine - w..=g.spine,
                        l.top().max(r.top())..=l.bottom().min(r.bottom()),
                    ),
                    Color32::TRANSPARENT,
                    Color32::from_black_alpha(if th.night { 60 } else { 34 }),
                ));
            }
        }

        // ---- barras
        let ptr_y = input.pointer.hover_pos().map(|p| p.y);
        let near = ptr_y.is_some_and(|y| y < full.top() + 70.0 || y > full.bottom() - 70.0);
        let show_bars = near
            || reader.show_settings
            || reader.show_toc
            || time - reader.opened_at < 2.5
            || reader.slider.is_some();
        let alpha = ctx.animate_bool(egui::Id::NULL.with("barras"), show_bars);
        if time - reader.opened_at < 2.6 {
            ctx.request_repaint_after(std::time::Duration::from_millis(300));
        }
        let bar_fill = if th.night {
            Color32::from_rgba_unmultiplied(20, 20, 22, 230)
        } else {
            Color32::from_rgba_unmultiplied(250, 249, 246, 235)
        };
        let txt = if th.night {
            Color32::from_gray(210)
        } else {
            Color32::from_gray(40)
        };

        let mut open_settings = reader.show_settings;
        let mut open_toc = reader.show_toc;
        if alpha > 0.01 {
            egui::Area::new(egui::Id::NULL.with("barra_sup"))
                .fixed_pos(full.left_top())
                .order(egui::Order::Foreground)
                .show(&ctx, |ui| {
                    ui.set_opacity(alpha);
                    egui::Frame::new()
                        .fill(bar_fill)
                        .inner_margin(egui::Margin::symmetric(14, 8))
                        .show(ui, |ui| {
                            ui.set_width(full.width() - 28.0);
                            ui.visuals_mut().override_text_color = Some(txt);
                            ui.horizontal(|ui| {
                                if ui.button("‹ Biblioteca").clicked() {
                                    back_to_library = true;
                                }
                                if ui.selectable_label(open_toc, "Índice").clicked() {
                                    open_toc = !open_toc;
                                }
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        let fs = input.viewport().fullscreen.unwrap_or(false);
                                        if ui
                                            .button(if fs {
                                                "Salir de pantalla completa"
                                            } else {
                                                "Pantalla completa"
                                            })
                                            .on_hover_text("Pantalla completa (F)")
                                            .clicked()
                                        {
                                            ctx.send_viewport_cmd(
                                                egui::ViewportCommand::Fullscreen(!fs),
                                            );
                                        }
                                        if ui
                                            .button(if double {
                                                "Ver una página"
                                            } else {
                                                "Ver dos páginas"
                                            })
                                            .on_hover_text(
                                                "Cambiar entre lectura a una o dos páginas",
                                            )
                                            .clicked()
                                        {
                                            self.saved.settings.page_mode =
                                                if double { 1 } else { 2 };
                                        }
                                        if ui
                                            .selectable_label(open_settings, "Aa")
                                            .on_hover_text("Apariencia")
                                            .clicked()
                                        {
                                            open_settings = !open_settings;
                                        }
                                        if ui
                                            .button("Marcar")
                                            .on_hover_text("Marcador (B)")
                                            .clicked()
                                        {
                                            toggle_bookmark = true;
                                        }
                                        if let Doc::Pdf(p) = &mut reader.doc {
                                            if ui.button("+").on_hover_text("Ampliar").clicked() {
                                                p.zoom = (p.zoom * 1.25).min(6.0);
                                            }
                                            if ui.button("−").on_hover_text("Reducir").clicked() {
                                                p.zoom = (p.zoom * 0.8).max(1.0);
                                                if p.zoom <= 1.01 {
                                                    p.pan = Vec2::ZERO;
                                                }
                                            }
                                        }
                                        ui.add_space(ui.available_width() / 2.0 - 160.0);
                                        let mut t = title.clone();
                                        if t.chars().count() > 50 {
                                            t = t.chars().take(48).collect::<String>() + "…";
                                        }
                                        ui.label(egui::RichText::new(t).font(FontId::new(
                                            16.0,
                                            FontFamily::Name("serif-b".into()),
                                        )));
                                    },
                                );
                            });
                        });
                });

            egui::Area::new(egui::Id::NULL.with("barra_inf"))
                .anchor(Align2::LEFT_BOTTOM, vec2(0.0, 0.0))
                .order(egui::Order::Foreground)
                .show(&ctx, |ui| {
                    ui.set_opacity(alpha);
                    egui::Frame::new()
                        .fill(bar_fill)
                        .inner_margin(egui::Margin::symmetric(18, 8))
                        .show(ui, |ui| {
                            ui.set_width(full.width() - 36.0);
                            ui.visuals_mut().override_text_color = Some(txt);
                            ui.horizontal(|ui| {
                                let shown = reader.page;
                                let info = match &reader.doc {
                                    Doc::Epub(ep) => {
                                        let l = ep.layout.as_ref().unwrap();
                                        let ch = l.pages[shown].chapter;
                                        let left = l.pages[shown..]
                                            .iter()
                                            .take_while(|p| p.chapter == ch)
                                            .count()
                                            .saturating_sub(1);
                                        format!(
                                            "Página {} de {}  ·  {} %  ·  {}",
                                            shown + 1,
                                            n_pages,
                                            ((shown + 1) * 100 / n_pages),
                                            if left == 0 {
                                                "última página del capítulo".to_string()
                                            } else {
                                                format!("quedan {left} págs. en el capítulo")
                                            }
                                        )
                                    }
                                    Doc::Pdf(_) => format!(
                                        "Página {} de {}  ·  {} %",
                                        shown + 1,
                                        n_pages,
                                        ((shown + 1) * 100 / n_pages)
                                    ),
                                };
                                ui.label(info);
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        let mut v = reader.slider.unwrap_or(reader.page as f32);
                                        ui.spacing_mut().slider_width =
                                            (ui.available_width() - 40.0).max(80.0);
                                        let r = ui.add(
                                            egui::Slider::new(&mut v, 0.0..=(n_pages - 1) as f32)
                                                .show_value(false)
                                                .step_by(1.0),
                                        );
                                        if r.dragged() {
                                            reader.slider = Some(v);
                                        }
                                        if r.drag_stopped() || (r.changed() && !r.dragged()) {
                                            jump = Some(v as usize);
                                            reader.slider = None;
                                        }
                                        if let Some(v) = reader.slider {
                                            r.on_hover_text(format!("Página {}", v as usize + 1));
                                        }
                                    },
                                );
                            });
                        });
                });
        }

        // ---- índice
        if open_toc {
            egui::Area::new(egui::Id::NULL.with("indice"))
                .fixed_pos(full.left_top() + vec2(0.0, 48.0))
                .order(egui::Order::Foreground)
                .show(&ctx, |ui| {
                    egui::Frame::new()
                        .fill(bar_fill)
                        .inner_margin(egui::Margin::same(14))
                        .show(ui, |ui| {
                            ui.set_width(280.0);
                            ui.set_height(full.height() - 110.0);
                            ui.visuals_mut().override_text_color = Some(txt);
                            egui::ScrollArea::vertical().show(ui, |ui| {
                                if let Doc::Epub(ep) = &reader.doc {
                                    ui.heading("Índice");
                                    ui.add_space(6.0);
                                    let cur = sp.first(sp.of(reader.page));
                                    for t in &ep.epub.toc {
                                        let p = ep
                                            .layout
                                            .as_ref()
                                            .map(|l| l.page_of(t.chapter, t.block))
                                            .unwrap_or(0);
                                        ui.horizontal(|ui| {
                                            ui.add_space(t.level as f32 * 14.0);
                                            let sel = cur >= p
                                                && ep.layout.as_ref().is_some_and(|l| {
                                                    l.pages[cur].chapter == t.chapter
                                                });
                                            if ui.selectable_label(sel, &t.title).clicked() {
                                                jump_anchor = Some((t.chapter, t.block));
                                            }
                                            ui.with_layout(
                                                egui::Layout::right_to_left(egui::Align::Center),
                                                |ui| {
                                                    ui.weak(format!("{}", p + 1));
                                                },
                                            );
                                        });
                                    }
                                    if ep.epub.toc.is_empty() {
                                        ui.weak("Este libro no incluye índice.");
                                    }
                                    ui.add_space(14.0);
                                }
                                ui.heading("Marcadores");
                                ui.add_space(6.0);
                                if bookmarks.is_empty() {
                                    ui.weak("Pulsa «Marcar» o la tecla B para marcar la página.");
                                }
                                for b in &bookmarks {
                                    let (label, target) = match &reader.doc {
                                        Doc::Pdf(_) => (format!("Página {}", b.0 + 1), b.0),
                                        Doc::Epub(ep) => {
                                            let p = l_page(ep, *b).unwrap_or(0);
                                            let ch = ep
                                                .epub
                                                .chapter_title(b.0)
                                                .unwrap_or("")
                                                .to_string();
                                            (format!("Pág. {} — {}", p + 1, ch), p)
                                        }
                                    };
                                    if ui.selectable_label(false, label).clicked() {
                                        jump = Some(target);
                                    }
                                }
                            });
                        });
                });
        }

        // ---- apariencia
        if open_settings {
            let s = &mut self.saved.settings;
            egui::Area::new(egui::Id::NULL.with("ajustes"))
                .anchor(Align2::RIGHT_TOP, vec2(-12.0, 52.0))
                .order(egui::Order::Foreground)
                .show(&ctx, |ui| {
                    egui::Frame::popup(ui.style())
                        .inner_margin(egui::Margin::same(14))
                        .show(ui, |ui| {
                            ui.set_width(280.0);
                            ui.label(egui::RichText::new("Tema").strong());
                            ui.horizontal(|ui| {
                                for (i, n) in book::THEME_NAMES.iter().enumerate() {
                                    let t = book::theme(i);
                                    let (r, resp) =
                                        ui.allocate_exact_size(vec2(56.0, 40.0), Sense::click());
                                    ui.painter().rect_filled(r, 6.0, t.paper);
                                    ui.painter().rect_stroke(
                                        r,
                                        6.0,
                                        egui::Stroke::new(
                                            if s.theme == i { 2.5 } else { 1.0 },
                                            if s.theme == i {
                                                Color32::from_rgb(20, 90, 200)
                                            } else {
                                                Color32::GRAY
                                            },
                                        ),
                                        egui::StrokeKind::Inside,
                                    );
                                    ui.painter().text(
                                        r.center(),
                                        Align2::CENTER_CENTER,
                                        *n,
                                        FontId::proportional(13.0),
                                        t.ink,
                                    );
                                    if resp.clicked() {
                                        s.theme = i;
                                    }
                                }
                            });
                            if !is_pdf {
                                ui.add_space(8.0);
                                ui.label(egui::RichText::new("Texto").strong());
                                ui.horizontal(|ui| {
                                    ui.selectable_value(
                                        &mut s.sans,
                                        false,
                                        egui::RichText::new("Serif").font(FontId::new(
                                            16.0,
                                            FontFamily::Name("serif".into()),
                                        )),
                                    );
                                    ui.selectable_value(&mut s.sans, true, "Sans");
                                    ui.checkbox(&mut s.justify, "Justificar");
                                });
                                ui.add(
                                    egui::Slider::new(&mut s.font_size, 12.0..=40.0)
                                        .text("Tamaño")
                                        .step_by(0.5),
                                );
                                ui.add(
                                    egui::Slider::new(&mut s.line_spacing, 1.1..=2.0)
                                        .text("Interlineado"),
                                );
                                ui.add(
                                    egui::Slider::new(&mut s.margin, 0.04..=0.18).text("Márgenes"),
                                );
                            }
                            ui.add_space(8.0);
                            ui.label(egui::RichText::new("Páginas").strong());
                            ui.horizontal(|ui| {
                                ui.selectable_value(&mut s.page_mode, 0, "Auto");
                                ui.selectable_value(&mut s.page_mode, 1, "Una");
                                ui.selectable_value(&mut s.page_mode, 2, "Doble");
                            });
                            ui.checkbox(
                                &mut s.cover_alone,
                                "Portada sola (como un libro o revista)",
                            );
                            ui.checkbox(&mut s.animate, "Animación al pasar página");
                        });
                });
        }

        reader.show_settings = open_settings;
        reader.show_toc = open_toc;

        if let Some(a) = jump_anchor {
            if let Doc::Epub(ep) = &reader.doc {
                if let Some(l) = &ep.layout {
                    jump = Some(l.page_of(a.0, a.1));
                }
            }
        }
        if let Some(p) = jump {
            reader.turn = None;
            reader.page = p.min(n_pages - 1);
            if let Doc::Epub(ep) = &reader.doc {
                if let Some(pg) = ep.layout.as_ref().and_then(|l| l.pages.get(reader.page)) {
                    reader.anchor = (pg.chapter, pg.block);
                }
            }
        }
        if toggle_bookmark {
            let key = match &reader.doc {
                Doc::Pdf(_) => (reader.page, 0),
                Doc::Epub(ep) => ep
                    .layout
                    .as_ref()
                    .and_then(|l| l.pages.get(reader.page))
                    .map(|p| (p.chapter, p.block))
                    .unwrap_or((0, 0)),
            };
            let e = &mut self.saved.library[reader.entry];
            if let Some(i) = e.bookmarks.iter().position(|b| *b == key) {
                e.bookmarks.remove(i);
            } else {
                e.bookmarks.push(key);
                e.bookmarks.sort();
            }
        }
        self.sync_entry();
        if back_to_library {
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
            self.reader = None;
        }
    }
}

impl App {
    fn cover_editor_ui(&mut self, ctx: &egui::Context) {
        let Some(ed) = self.cover_editor.as_mut() else {
            return;
        };
        if ed.dirty {
            let img = ed.render(360);
            let ci = egui::ColorImage::from_rgb(
                [img.width() as usize, img.height() as usize],
                img.as_raw(),
            );
            ed.preview = Some(ctx.load_texture("portada-previa", ci, TextureOptions::LINEAR));
            ed.dirty = false;
        }
        let mut open = true;
        let mut apply = false;
        let mut cancel = false;
        egui::Window::new(if ed.replacing {
            "Cambiar portada"
        } else {
            "Crear portada"
        })
        .collapsible(false)
        .resizable(false)
        .open(&mut open)
        .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
        .show(ctx, |ui| {
            ui.horizontal_top(|ui| {
                if let Some(t) = &ed.preview {
                    let s = t.size_vec2();
                    let k = 380.0 / s.y;
                    ui.add(egui::Image::new((t.id(), s * k)));
                }
                ui.add_space(12.0);
                ui.vertical(|ui| {
                    ui.set_width(280.0);
                    ui.label(egui::RichText::new("Diseño automático").strong());
                    ui.label("Título");
                    if ui.text_edit_singleline(&mut ed.title).changed() {
                        ed.dirty = true;
                    }
                    ui.label("Autor");
                    if ui.text_edit_singleline(&mut ed.author).changed() {
                        ed.dirty = true;
                    }
                    ui.add_space(6.0);
                    ui.label("Estilo");
                    ui.horizontal_wrapped(|ui| {
                        for (i, st) in crate::cover::STYLES.iter().enumerate() {
                            let (r, resp) =
                                ui.allocate_exact_size(vec2(40.0, 40.0), Sense::click());
                            let [br, bg, bb] = st.bg;
                            let [ar, ag, ab] = st.accent;
                            ui.painter()
                                .rect_filled(r, 4.0, Color32::from_rgb(br, bg, bb));
                            ui.painter().rect_stroke(
                                r.shrink(5.0),
                                2.0,
                                egui::Stroke::new(2.0, Color32::from_rgb(ar, ag, ab)),
                                egui::StrokeKind::Inside,
                            );
                            if ed.style == i && ed.custom.is_none() {
                                ui.painter().rect_stroke(
                                    r.expand(2.0),
                                    5.0,
                                    egui::Stroke::new(2.5, Color32::from_rgb(20, 90, 200)),
                                    egui::StrokeKind::Outside,
                                );
                            }
                            if resp.on_hover_text(st.name).clicked() {
                                ed.style = i;
                                ed.custom = None;
                                ed.dirty = true;
                            }
                        }
                    });
                    ui.add_space(10.0);
                    ui.separator();
                    ui.label(egui::RichText::new("O usa tu propia imagen").strong());
                    ui.horizontal(|ui| {
                        if ui.button("Elegir imagen…").clicked() {
                            if let Some(f) = rfd::FileDialog::new()
                                .add_filter("Imagen", &["jpg", "jpeg", "png", "webp", "gif"])
                                .pick_file()
                            {
                                match image::open(&f) {
                                    Ok(img) => {
                                        ed.custom = Some(img);
                                        ed.dirty = true;
                                        ed.error = None;
                                    }
                                    Err(e) => {
                                        ed.error = Some(format!("No se puede abrir la imagen: {e}"))
                                    }
                                }
                            }
                        }
                        if ed.custom.is_some() && ui.button("Quitar").clicked() {
                            ed.custom = None;
                            ed.dirty = true;
                        }
                    });
                    ui.add_space(10.0);
                    ui.separator();
                    ui.weak(if ed.page.is_some() {
                        "Se añadirá como primera página del PDF."
                    } else {
                        "Se guardará dentro del EPUB como portada."
                    });
                    ui.weak("Antes se guarda una copia del original.");
                    if let Some(e) = &ed.error {
                        ui.colored_label(Color32::from_rgb(180, 30, 30), e);
                    }
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui
                            .add(
                                egui::Button::new(egui::RichText::new("Añadir al libro").strong())
                                    .fill(Color32::from_rgb(20, 90, 200)),
                            )
                            .clicked()
                        {
                            apply = true;
                        }
                        if ui.button("Cancelar").clicked() {
                            cancel = true;
                        }
                    });
                });
            });
        });
        if apply {
            let ed = self.cover_editor.as_mut().unwrap();
            let e = &self.saved.library[ed.idx];
            let path = PathBuf::from(&e.path);
            let img = ed.render(1400);
            let result = crate::cover::jpeg(&img).and_then(|jpg| match ed.page {
                Some((pw, ph)) => {
                    crate::cover::attach_pdf(&path, &jpg, img.width(), img.height(), pw, ph)
                }
                None => crate::cover::attach_epub(&path, &jpg),
            });
            match result {
                Ok(()) => {
                    let is_pdf = ed.page.is_some();
                    let new_page = is_pdf || !ed.replacing;
                    let e = &mut self.saved.library[ed.idx];
                    if new_page && e.opened {
                        if is_pdf {
                            e.page += 1;
                            for b in &mut e.bookmarks {
                                b.0 += 1;
                            }
                        } else {
                            e.chapter += 1;
                            for b in &mut e.bookmarks {
                                b.0 += 1;
                            }
                        }
                    }
                    let p = e.path.clone();
                    self.requested.remove(&p);
                    self.cover_tex.remove(&p);
                    self.cover_editor = None;
                    self.flash("Portada añadida al libro".into());
                }
                Err(err) => ed.error = Some(err),
            }
        } else if cancel || !open {
            self.cover_editor = None;
        }
    }
}

fn l_page(ep: &EpubState, b: (usize, usize)) -> Option<usize> {
    ep.layout.as_ref().map(|l| l.page_of(b.0, b.1))
}

#[allow(dead_code)]
fn _unused(_: Pos2) {}
