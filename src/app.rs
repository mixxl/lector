use crate::book::{self, Flip, Side, Theme};
use crate::covers::{self, CoverWorker};
use crate::epub::{self, Epub};
use crate::layout::{self, Item, Layout, Params, TextPos};
use crate::pdfdoc::{Msg, PdfWorker};
use crate::tools;
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
    pub hyphenate: bool,
    pub publisher: bool,
    pub tts_rate: u32,
    pub pdf_crop: bool,
    pub pdf_scroll: bool,
    pub goal_min: u32,
    /// PDF a pantalla completa: ajustar al ancho y recortar márgenes
    pub pdf_fill: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            theme: 0,
            font_size: 19.0,
            line_spacing: 1.42,
            margin: 0.07,
            sans: false,
            justify: true,
            page_mode: 0,
            cover_alone: true,
            animate: true,
            hyphenate: true,
            publisher: false,
            tts_rate: 185,
            pdf_crop: false,
            pdf_scroll: false,
            goal_min: 20,
            pdf_fill: true,
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
    /// temas del libro (dc:subject)
    pub subjects: Vec<String>,
    /// colecciones del usuario
    pub collections: Vec<String>,
    pub finished: bool,
    pub meta_v: u8,
    pub notes: Vec<Note>,
    pub finished_at: u64,
}

/// Subrayado con nota opcional. En PDF: nota de página (chapter = página).
#[derive(Serialize, Deserialize, Clone, Default, PartialEq)]
#[serde(default)]
pub struct Note {
    pub chapter: usize,
    pub block: usize,
    pub start: usize,
    pub end_block: usize,
    pub end: usize,
    pub color: u8,
    pub text: String,
    pub note: String,
    pub created: u64,
}

impl Note {
    fn from(&self) -> TextPos {
        (self.chapter, self.block, self.start)
    }
    fn to(&self) -> TextPos {
        (self.chapter, self.end_block, self.end)
    }
}

pub const NOTE_COLORS: [(u8, u8, u8); 4] = [
    (255, 214, 80),
    (140, 220, 120),
    (130, 190, 255),
    (255, 150, 190),
];

fn note_fill(c: u8, night: bool) -> Color32 {
    let (r, g, b) = NOTE_COLORS[c as usize % 4];
    if night {
        Color32::from_rgba_unmultiplied(r, g, b, 70)
    } else {
        Color32::from_rgba_unmultiplied(r, g, b, 110)
    }
}

impl Entry {
    fn is_finished(&self) -> bool {
        self.finished || self.progress >= 0.995
    }
    fn is_reading(&self) -> bool {
        self.opened && !self.is_finished()
    }
    fn author_name(&self) -> &str {
        if self.author.trim().is_empty() {
            "Autor desconocido"
        } else {
            self.author.trim()
        }
    }
}

#[derive(Clone, PartialEq)]
enum Filter {
    Todo,
    Leyendo,
    ParaLeer,
    Terminados,
    Libros,
    Pdf,
    Autores,
    Autor(String),
    Temas,
    Tema(String),
    Coleccion(String),
    Estadisticas,
}

impl Filter {
    fn title(&self) -> String {
        match self {
            Filter::Todo => "Todo".into(),
            Filter::Estadisticas => "Lectura".into(),
            Filter::Leyendo => "Leyendo".into(),
            Filter::ParaLeer => "Para leer".into(),
            Filter::Terminados => "Terminados".into(),
            Filter::Libros => "Libros".into(),
            Filter::Pdf => "PDF y cómics".into(),
            Filter::Autores => "Autores".into(),
            Filter::Temas => "Temas".into(),
            Filter::Autor(a) | Filter::Tema(a) | Filter::Coleccion(a) => a.clone(),
        }
    }
    fn matches(&self, e: &Entry) -> bool {
        match self {
            Filter::Todo | Filter::Autores | Filter::Temas | Filter::Estadisticas => true,
            Filter::Leyendo => e.is_reading(),
            Filter::ParaLeer => !e.opened && !e.is_finished(),
            Filter::Terminados => e.is_finished(),
            Filter::Libros => !e.is_pdf,
            Filter::Pdf => e.is_pdf,
            Filter::Autor(a) => e.author_name() == a,
            Filter::Tema(t) => e.subjects.iter().any(|s| s == t),
            Filter::Coleccion(c) => e.collections.iter().any(|s| s == c),
        }
    }
}

#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
struct Saved {
    settings: Settings,
    library: Vec<Entry>,
    collections: Vec<String>,
    sort: u8,
    /// segundos leídos por día (AAAA-MM-DD)
    stats: std::collections::BTreeMap<String, u32>,
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
    outline: std::sync::Arc<std::sync::Mutex<Option<Vec<(String, usize, usize)>>>>,
    crops: HashMap<usize, Rect>,
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
    /// selección de texto en curso (ancla, cabeza)
    sel: Option<(TextPos, TextPos)>,
    selecting: bool,
    popup: Option<Popup>,
    note_edit: Option<NoteEdit>,
    /// 0 índice, 1 notas, 2 buscar
    panel_tab: u8,
    search_q: String,
    search_results: Vec<SearchHit>,
    search_hit: Option<(TextPos, TextPos)>,
    search_pending: bool,
    pdf_text: Option<std::sync::Arc<std::sync::Mutex<Option<Vec<String>>>>>,
    tts: Option<std::process::Child>,
    tts_on: bool,
    tts_page: Option<usize>,
    /// desplazamiento vertical en modo continuo (PDF)
    scroll_y: f32,
    scroll_page: Option<usize>,
    fill_spread: Option<usize>,
    edge_since: Option<f64>,
}

#[derive(Clone)]
struct SearchHit {
    pos: TextPos,
    len: usize,
    page: usize,
    context: String,
}

#[derive(Clone)]
enum PopupKind {
    New(TextPos, TextPos),
    Existing(usize),
}

#[derive(Clone)]
struct Popup {
    pos: Pos2,
    kind: PopupKind,
}

struct NoteEdit {
    /// índice de la nota (None: nueva nota de página en PDF)
    idx: Option<usize>,
    page: usize,
    text: String,
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
    filter: Filter,
    new_collection: Option<String>,
    meta_requested: HashSet<String>,
    stat_tick: Option<std::time::Instant>,
    stat_acc: f32,
    meta_editor: Option<(usize, String, String, String)>,
}

impl Drop for Reader {
    fn drop(&mut self) {
        if let Some(mut c) = self.tts.take() {
            let _ = c.kill();
        }
    }
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
            filter: Filter::Todo,
            new_collection: None,
            meta_requested: HashSet::new(),
            stat_tick: None,
            stat_acc: 0.0,
            meta_editor: None,
        }
    }

    fn add_path(&mut self, p: &Path) -> Option<usize> {
        let s = p.to_string_lossy().to_string();
        let lower = s.to_lowercase();
        let is_pdf = lower.ends_with(".pdf") || lower.ends_with(".cbz");
        let book = [".epub", ".mobi", ".azw", ".azw3", ".prc"]
            .iter()
            .any(|e| lower.ends_with(e));
        if !is_pdf && !book {
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
                    outline: start_pdf_outline(if crate::pdfdoc::is_cbz(&e.path) {
                        vec![]
                    } else {
                        data.clone()
                    }),
                    crops: HashMap::new(),
                    worker: if crate::pdfdoc::is_cbz(&e.path) {
                        match crate::pdfdoc::cbz_images(&data) {
                            Some(imgs) => PdfWorker::spawn_images(imgs, String::new(), ctx.clone()),
                            None => {
                                self.flash("El CBZ no contiene imágenes".into());
                                return;
                            }
                        }
                    } else {
                        PdfWorker::spawn(data, ctx.clone())
                    },
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
            sel: None,
            selecting: false,
            popup: None,
            note_edit: None,
            panel_tab: 0,
            search_q: String::new(),
            search_results: vec![],
            search_hit: None,
            search_pending: false,
            pdf_text: None,
            tts: None,
            tts_on: false,
            tts_page: None,
            scroll_y: 0.0,
            scroll_page: None,
            fill_spread: None,
            edge_since: None,
        });
    }

    fn open_dialog(&mut self, ctx: &egui::Context) {
        if let Some(files) = rfd::FileDialog::new()
            .add_filter(
                "Libros",
                &["pdf", "epub", "mobi", "azw", "azw3", "prc", "cbz"],
            )
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
        // archivos o carpetas por línea de comandos: uno solo se abre, varios se añaden
        #[allow(unused_mut)]
        let mut initial = std::mem::take(&mut self.initial);
        #[cfg(target_os = "macos")]
        initial.extend(crate::macos::take_opened());
        let mut books = Vec::new();
        for p in &initial {
            let p = PathBuf::from(p);
            if p.is_dir() && !p.to_string_lossy().to_lowercase().ends_with(".epub") {
                covers::scan(&p, &mut books, 0);
            } else {
                books.push(p);
            }
        }
        if books.len() == 1 && initial.len() == 1 {
            if let Some(i) = self.add_path(&books[0]) {
                self.open(&ctx, i);
            }
        } else if !books.is_empty() {
            for b in &books {
                self.add_path(b);
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
                if let Some(info) = r.meta {
                    if !info.title.is_empty() && (!e.meta_done || e.meta_v < 2) {
                        e.title = info.title;
                    }
                    if !info.author.is_empty() {
                        e.author = info.author;
                    }
                    e.subjects = info.subjects;
                    e.meta_v = 2;
                }
                e.meta_done = true;
            }
            if r.meta_only {
                continue;
            }
            let tex = r.image.map(|img| {
                ctx.load_texture(format!("portada:{}", r.path), img, TextureOptions::LINEAR)
            });
            self.cover_tex.insert(r.path, tex);
        }

        if self.reader.is_some() {
            self.track_reading(&ctx);
            self.reader_ui(ui);
        } else {
            self.library_ui(ui);
            self.cover_editor_ui(&ctx);
            self.meta_editor_ui(&ctx);
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

        let bg = Color32::from_rgb(250, 249, 247);
        let side_bg = Color32::from_rgb(236, 234, 230);
        let ink = Color32::from_rgb(30, 30, 30);

        // ---- recuentos
        let lib = &self.saved.library;
        let count = |f: &Filter| lib.iter().filter(|e| f.matches(e)).count();
        let mut subjects: HashMap<String, usize> = HashMap::new();
        let mut authors: HashMap<String, usize> = HashMap::new();
        for e in lib {
            for t in &e.subjects {
                *subjects.entry(t.clone()).or_default() += 1;
            }
            *authors.entry(e.author_name().to_string()).or_default() += 1;
        }
        let mut subj_sorted: Vec<(String, usize)> = subjects.into_iter().collect();
        subj_sorted.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        let mut auth_sorted: Vec<(String, usize)> = authors.into_iter().collect();
        auth_sorted.sort_by(|a, b| sort_key(&a.0).cmp(&sort_key(&b.0)));
        let mut collections = self.saved.collections.clone();
        for e in lib {
            for c in &e.collections {
                if !collections.contains(c) {
                    collections.push(c.clone());
                }
            }
        }
        collections.sort_by_key(|c| sort_key(c));
        let n_total = lib.len();
        let counts = [
            count(&Filter::Leyendo),
            count(&Filter::ParaLeer),
            count(&Filter::Terminados),
            count(&Filter::Libros),
            count(&Filter::Pdf),
        ];
        let coll_counts: Vec<usize> = collections
            .iter()
            .map(|c| lib.iter().filter(|e| e.collections.contains(c)).count())
            .collect();

        // ---- menú lateral
        let mut new_filter: Option<Filter> = None;
        let today_min = self.today_minutes();
        let goal_min = self.saved.settings.goal_min;
        let mut delete_collection: Option<String> = None;
        let current = self.filter.clone();
        egui::Panel::left("menu")
            .resizable(false)
            .exact_size(230.0)
            .frame(
                egui::Frame::new()
                    .fill(side_bg)
                    .inner_margin(egui::Margin::symmetric(12, 14)),
            )
            .show(ui, |ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.search)
                        .hint_text("Buscar título, autor o tema")
                        .desired_width(f32::INFINITY),
                );
                ui.add_space(10.0);
                // tarjeta de lectura de hoy
                let (card, cresp) =
                    ui.allocate_exact_size(vec2(ui.available_width(), 52.0), Sense::click());
                let cp = ui.painter();
                cp.rect_filled(
                    card,
                    8.0,
                    if current == Filter::Estadisticas {
                        Color32::from_rgb(214, 211, 206)
                    } else {
                        Color32::from_rgb(246, 244, 240)
                    },
                );
                cp.text(
                    card.left_top() + vec2(10.0, 8.0),
                    Align2::LEFT_TOP,
                    "Lectura de hoy",
                    FontId::proportional(13.0),
                    Color32::from_gray(90),
                );
                cp.text(
                    card.right_top() + vec2(-10.0, 8.0),
                    Align2::RIGHT_TOP,
                    format!("{} / {} min", today_min, goal_min),
                    FontId::proportional(13.0),
                    Color32::from_rgb(30, 30, 30),
                );
                let bar = Rect::from_min_size(
                    card.left_top() + vec2(10.0, 32.0),
                    vec2(card.width() - 20.0, 8.0),
                );
                cp.rect_filled(bar, 4.0, Color32::from_gray(220));
                cp.rect_filled(
                    Rect::from_min_size(
                        bar.min,
                        vec2(
                            bar.width() * (today_min as f32 / goal_min.max(1) as f32).min(1.0),
                            8.0,
                        ),
                    ),
                    4.0,
                    Color32::from_rgb(130, 30, 30),
                );
                if cresp
                    .on_hover_text("Estadísticas y objetivo de lectura")
                    .clicked()
                {
                    new_filter = Some(Filter::Estadisticas);
                }
                ui.add_space(6.0);
                egui::ScrollArea::vertical()
                    .auto_shrink(false)
                    .show(ui, |ui| {
                        let mut row = |ui: &mut egui::Ui,
                                       f: Filter,
                                       label: &str,
                                       n: usize|
                         -> egui::Response {
                            let sel = current == f;
                            let (rect, r) = ui.allocate_exact_size(
                                vec2(ui.available_width(), 26.0),
                                Sense::click(),
                            );
                            let p = ui.painter();
                            if sel {
                                p.rect_filled(rect, 6.0, Color32::from_rgb(214, 211, 206));
                            } else if r.hovered() {
                                p.rect_filled(rect, 6.0, Color32::from_rgb(226, 223, 218));
                            }
                            let mut text = label.to_string();
                            if text.chars().count() > 22 {
                                text = text.chars().take(21).collect::<String>() + "…";
                            }
                            p.text(
                                rect.left_center() + vec2(10.0, 0.0),
                                Align2::LEFT_CENTER,
                                text,
                                FontId::proportional(14.5),
                                Color32::from_rgb(30, 30, 30),
                            );
                            p.text(
                                rect.right_center() - vec2(10.0, 0.0),
                                Align2::RIGHT_CENTER,
                                n.to_string(),
                                FontId::proportional(12.5),
                                Color32::from_gray(120),
                            );
                            let r = r.on_hover_text(label);
                            if r.clicked() {
                                new_filter = Some(f);
                            }
                            r
                        };
                        let head = |ui: &mut egui::Ui, t: &str| {
                            ui.add_space(8.0);
                            ui.label(
                                egui::RichText::new(t)
                                    .strong()
                                    .size(13.0)
                                    .color(Color32::from_gray(90)),
                            );
                        };
                        head(ui, "Biblioteca");
                        row(ui, Filter::Leyendo, "Leyendo", counts[0]);
                        row(ui, Filter::ParaLeer, "Para leer", counts[1]);
                        row(ui, Filter::Terminados, "Terminados", counts[2]);
                        row(ui, Filter::Todo, "Todo", n_total);
                        row(ui, Filter::Libros, "Libros", counts[3]);
                        row(ui, Filter::Pdf, "PDF y cómics", counts[4]);
                        head(ui, "Explorar");
                        row(ui, Filter::Autores, "Autores", auth_sorted.len());
                        row(ui, Filter::Temas, "Temas", subj_sorted.len());
                        if !subj_sorted.is_empty() {
                            head(ui, "Temas principales");
                            for (t, n) in subj_sorted.iter().take(10) {
                                row(ui, Filter::Tema(t.clone()), t, *n);
                            }
                        }
                        head(ui, "Mis colecciones");
                        for (c, n) in collections.iter().zip(&coll_counts) {
                            let r = row(ui, Filter::Coleccion(c.clone()), c, *n);
                            r.context_menu(|ui| {
                                if ui.button("Eliminar colección").clicked() {
                                    delete_collection = Some(c.clone());
                                    ui.close();
                                }
                            });
                        }
                        match &mut self.new_collection {
                            Some(name) => {
                                let r = ui.add(
                                    egui::TextEdit::singleline(name)
                                        .hint_text("Nombre y pulsa Intro"),
                                );
                                r.request_focus();
                                if r.lost_focus() {
                                    let n = name.trim().to_string();
                                    if !n.is_empty() && !self.saved.collections.contains(&n) {
                                        self.saved.collections.push(n.clone());
                                        new_filter = Some(Filter::Coleccion(n));
                                    }
                                    self.new_collection = None;
                                }
                            }
                            None => {
                                if ui.button("+ Nueva colección").clicked() {
                                    self.new_collection = Some(String::new());
                                }
                            }
                        }
                    });
            });
        if let Some(f) = new_filter {
            self.filter = f;
        }
        if let Some(c) = delete_collection {
            self.saved.collections.retain(|x| *x != c);
            for e in &mut self.saved.library {
                e.collections.retain(|x| *x != c);
            }
            if self.filter == Filter::Coleccion(c) {
                self.filter = Filter::Todo;
            }
        }

        // ---- lista filtrada
        let q = self.search.to_lowercase();
        let lib = &self.saved.library;
        let mut order: Vec<usize> = (0..lib.len())
            .filter(|&i| {
                let e = &lib[i];
                self.filter.matches(e)
                    && (q.is_empty()
                        || e.title.to_lowercase().contains(&q)
                        || e.author.to_lowercase().contains(&q)
                        || e.subjects.iter().any(|s| s.to_lowercase().contains(&q)))
            })
            .collect();
        let sort = self.saved.sort;
        order.sort_by(|&a, &b| {
            let (ea, eb) = (&lib[a], &lib[b]);
            match sort {
                1 => sort_key(&ea.title).cmp(&sort_key(&eb.title)),
                2 => sort_key(ea.author_name())
                    .cmp(&sort_key(eb.author_name()))
                    .then(sort_key(&ea.title).cmp(&sort_key(&eb.title))),
                3 => eb
                    .progress
                    .partial_cmp(&ea.progress)
                    .unwrap_or(std::cmp::Ordering::Equal),
                _ => eb
                    .last_opened
                    .cmp(&ea.last_opened)
                    .then(eb.added.cmp(&ea.added)),
            }
        });
        let n_pdf_shown = order.iter().filter(|&&i| lib[i].is_pdf).count();
        let list_mode = matches!(
            self.filter,
            Filter::Autores | Filter::Temas | Filter::Estadisticas
        );

        egui::Panel::top("cabecera")
            .frame(egui::Frame::new().fill(bg).inner_margin(egui::Margin {
                left: 28,
                right: 28,
                top: 18,
                bottom: 10,
            }))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    if matches!(self.filter, Filter::Autor(_)) && ui.button("‹ Autores").clicked()
                    {
                        self.filter = Filter::Autores;
                    }
                    if matches!(self.filter, Filter::Tema(_)) && ui.button("‹ Temas").clicked() {
                        self.filter = Filter::Temas;
                    }
                    ui.label(
                        egui::RichText::new(self.filter.title())
                            .font(FontId::new(34.0, FontFamily::Name("serif-b".into())))
                            .color(ink),
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
                        if !list_mode {
                            ui.add_space(12.0);
                            let names = ["Recientes", "Título", "Autor", "Progreso"];
                            egui::ComboBox::from_id_salt("orden")
                                .selected_text(names[sort as usize % 4])
                                .show_ui(ui, |ui| {
                                    for (k, n) in names.iter().enumerate() {
                                        ui.selectable_value(&mut self.saved.sort, k as u8, *n);
                                    }
                                });
                            ui.label(egui::RichText::new("Ordenar").color(Color32::GRAY));
                        }
                    });
                });
                let sub = match self.filter {
                    Filter::Autores => format!("{} autores", auth_sorted.len()),
                    Filter::Temas => format!("{} temas", subj_sorted.len()),
                    Filter::Estadisticas => "Tu tiempo de lectura".to_string(),
                    _ => format!(
                        "{} libros, {} archivos PDF",
                        order.len() - n_pdf_shown,
                        n_pdf_shown
                    ),
                };
                ui.label(egui::RichText::new(sub).color(Color32::GRAY));
            });

        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(bg).inner_margin(egui::Margin::symmetric(28, 8)))
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
                            egui::RichText::new("o usa «Abrir…», «Añadir carpeta…» o «Importar Apple Books»")
                                .color(Color32::GRAY),
                        );
                    });
                    return;
                }
                if self.filter == Filter::Estadisticas {
                    self.stats_ui(ui);
                    return;
                }
                // ---- vista de autores / temas
                if list_mode {
                    let is_auth = self.filter == Filter::Autores;
                    let mut items: Vec<(String, usize)> =
                        if is_auth { auth_sorted.clone() } else { subj_sorted.clone() };
                    items.sort_by(|a, b| sort_key(&a.0).cmp(&sort_key(&b.0)));
                    items.retain(|(n, _)| q.is_empty() || n.to_lowercase().contains(&q));
                    if items.is_empty() {
                        ui.add_space(40.0);
                        ui.label(
                            egui::RichText::new(if is_auth {
                                "Sin resultados."
                            } else {
                                "Los temas se leen de los propios EPUB (todavía cargando o el libro no los trae)."
                            })
                            .color(Color32::GRAY),
                        );
                    }
                    egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                        let mut letter = String::new();
                        for (name, n) in items {
                            let l: String = sort_key(&name)
                                .chars()
                                .next()
                                .map(|c| c.to_uppercase().collect())
                                .unwrap_or_default();
                            if l != letter {
                                letter = l.clone();
                                ui.add_space(10.0);
                                ui.label(
                                    egui::RichText::new(&l)
                                        .font(FontId::new(22.0, FontFamily::Name("serif-b".into())))
                                        .color(Color32::from_rgb(130, 30, 30)),
                                );
                                ui.separator();
                            }
                            let r = ui.add(
                                egui::Button::new(
                                    egui::RichText::new(format!(
                                        "{name}   ·   {n} {}",
                                        if n == 1 { "libro" } else { "libros" }
                                    ))
                                    .size(16.0)
                                    .color(ink),
                                )
                                .frame_when_inactive(false),
                            );
                            if r.clicked() {
                                self.filter = if is_auth { Filter::Autor(name.clone()) } else { Filter::Tema(name.clone()) };
                                self.search.clear();
                            }
                        }
                    });
                    return;
                }
                if order.is_empty() {
                    ui.add_space(40.0);
                    ui.label(egui::RichText::new("No hay libros aquí.").color(Color32::GRAY));
                    return;
                }
                let cell = vec2(170.0, 300.0);
                let cols = ((ui.available_width() + 24.0) / (cell.x + 24.0)).floor().max(1.0) as usize;
                let rows = order.len().div_ceil(cols);
                let colls = collections.clone();
                egui::ScrollArea::vertical().auto_shrink(false).show_rows(ui, cell.y, rows, |ui, range| {
                    for r in range {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 24.0;
                            for c in 0..cols {
                                let Some(&i) = order.get(r * cols + c) else { break };
                                let (resp, want_cover) = self.book_cell(ui, i, cell);
                                if want_cover {
                                    create_idx = Some(i);
                                } else if resp.clicked() {
                                    open_idx = Some(i);
                                }
                                let has_cover =
                                    matches!(self.cover_tex.get(&self.saved.library[i].path), Some(Some(_)));
                                resp.context_menu(|ui| {
                                    if ui.button("Abrir").clicked() {
                                        open_idx = Some(i);
                                    }
                                    let fin = self.saved.library[i].is_finished();
                                    if ui
                                        .button(if fin { "Marcar como no terminado" } else { "Marcar como terminado" })
                                        .clicked()
                                    {
                                        let e = &mut self.saved.library[i];
                                        if fin {
                                            e.finished = false;
                                            if e.progress >= 0.995 {
                                                e.progress = 0.99;
                                            }
                                        } else {
                                            e.finished = true;
                                            e.finished_at = now();
                                        }
                                        ui.close();
                                    }
                                    ui.menu_button("Añadir a colección", |ui| {
                                        for c in &colls {
                                            let mut on = self.saved.library[i].collections.contains(c);
                                            if ui.checkbox(&mut on, c).changed() {
                                                let e = &mut self.saved.library[i];
                                                if on {
                                                    e.collections.push(c.clone());
                                                } else {
                                                    e.collections.retain(|x| x != c);
                                                }
                                            }
                                        }
                                        if colls.is_empty() {
                                            ui.weak("Crea una con «+ Nueva colección»");
                                        }
                                    });
                                    if ui.button("Editar datos…").clicked() {
                                            let e = &self.saved.library[i];
                                            self.meta_editor = Some((i, e.title.clone(), e.author.clone(), e.subjects.join(", ")));
                                            ui.close();
                                        }
                                        ui.separator();
                                    #[cfg(target_os = "macos")]
                                    if ui.button("Mostrar en Finder").clicked() {
                                        let _ = std::process::Command::new("open")
                                            .arg("-R")
                                            .arg(&self.saved.library[i].path)
                                            .spawn();
                                    }
                                    let label = if self.saved.library[i].is_pdf {
                                        "Añadir portada…"
                                    } else if has_cover {
                                        "Cambiar portada…"
                                    } else {
                                        "Crear portada…"
                                    };
                                    if ui.button(label).clicked() {
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
                });
            });

        // metadatos (temas, autores) de todos los libros, en segundo plano
        for e in &self.saved.library {
            if (e.meta_v < 2)
                && !self.requested.contains(&e.path)
                && !self.meta_requested.contains(&e.path)
            {
                self.meta_requested.insert(e.path.clone());
                self.covers.request_meta(&e.path);
            }
        }

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
            self.covers.request(&e.path, !e.meta_done || e.meta_v < 2);
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
        let ext = Path::new(&e.path)
            .extension()
            .map(|x| x.to_string_lossy().to_uppercase())
            .unwrap_or_default();
        if ext != "EPUB" {
            p.text(
                pos2(rect.right(), y),
                Align2::RIGHT_TOP,
                ext,
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
            if e.is_finished() && e.finished_at == 0 {
                e.finished_at = now();
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
        let mut note_ops: Vec<NoteOp> = Vec::new();
        let mut do_search = false;

        let reader = self.reader.as_mut().unwrap();
        let entry = &self.saved.library[reader.entry];
        let bookmarks = entry.bookmarks.clone();
        let notes = entry.notes.clone();

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
                        let c = r.crop;
                        pdf.crops.insert(
                            r.page,
                            Rect::from_min_max(pos2(c[0], c[1]), pos2(c[2], c[3])),
                        );
                    }
                    Msg::Error(e) => pdf.error = Some(e),
                }
            }
        }

        let full = ui.max_rect();
        let painter = ui.painter_at(full);
        book::desk(&painter, full, &th);

        // ---- área útil y modo de página
        let area = full.shrink2(vec2(14.0, 16.0));
        let scroll_mode = settings.pdf_scroll && matches!(reader.doc, Doc::Pdf(_));
        let fill = settings.pdf_fill && !scroll_mode && matches!(reader.doc, Doc::Pdf(_));
        let crop_on = settings.pdf_crop || fill;
        let double = !scroll_mode
            && match settings.page_mode {
                1 => false,
                2 => true,
                _ => area.width() > area.height() * 1.15,
            };

        // ---- EPUB: paginación
        let is_pdf = matches!(reader.doc, Doc::Pdf(_));
        // EPUB: las páginas ocupan toda la pantalla disponible
        let epub_page = |area: Rect, double: bool| -> Vec2 {
            if double {
                vec2(area.width() / 2.0, area.height())
            } else {
                vec2(area.width(), area.height())
            }
        };
        let margins = |size: Vec2| -> (f32, f32, f32) {
            // (lateral, superior, inferior)
            (size.x * settings.margin, size.y * 0.06, size.y * 0.07)
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
                hyphenate: settings.hyphenate,
                publisher: settings.publisher,
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
        let pdf_crops: HashMap<usize, Rect> = match &reader.doc {
            Doc::Pdf(p) if crop_on => p.crops.clone(),
            _ => HashMap::new(),
        };
        let pdf_sizes: Option<Vec<(f32, f32)>> = match &reader.doc {
            Doc::Pdf(p) => Some(p.sizes.clone()),
            _ => None,
        };
        let pan_cell = std::cell::Cell::new(pan);
        let geo = |s: usize| -> Geo {
            let pan = pan_cell.get();
            let (l, r) = sp.pages(s);
            let size_of = |p: usize| -> Vec2 {
                match &pdf_sizes {
                    Some(sz) => {
                        let c = pdf_crops
                            .get(&p)
                            .copied()
                            .unwrap_or(Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)));
                        vec2(sz[p].0 * c.width(), sz[p].1 * c.height())
                    }
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
            if fill {
                // de borde a borde: ajusta al ancho de la ventana
                let w_avail = full.width();
                let cy = full.center().y + pan.y;
                let cx = full.center().x + pan.x;
                return if double {
                    let h = (w_avail / 2.0 / al.max(ar)) * zoom;
                    let top = cy - h / 2.0;
                    Geo {
                        spine: cx,
                        left: l
                            .map(|_| Rect::from_min_size(pos2(cx - al * h, top), vec2(al * h, h))),
                        right: r.map(|_| Rect::from_min_size(pos2(cx, top), vec2(ar * h, h))),
                    }
                } else {
                    let h = (w_avail / ar) * zoom;
                    let w = ar * h;
                    Geo {
                        spine: cx - w / 2.0,
                        left: None,
                        right: r.map(|_| {
                            Rect::from_min_size(pos2(cx - w / 2.0, cy - h / 2.0), vec2(w, h))
                        }),
                    }
                };
            }
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
        let mut g = geo(cur_spread);
        // ---- PDF a pantalla completa: desplazamiento vertical dentro del pliego
        let overflow = if fill {
            g.left
                .or(g.right)
                .map(|r| (r.height() - full.height()).max(0.0))
                .unwrap_or(0.0)
        } else {
            0.0
        };
        if fill {
            if let Doc::Pdf(p) = &mut reader.doc {
                if reader.fill_spread != Some(cur_spread) {
                    // pliego nuevo: arriba si avanzas, abajo si retrocedes
                    let back = reader.fill_spread.is_some_and(|s| s > cur_spread);
                    p.pan.y = if back {
                        -overflow / 2.0
                    } else {
                        overflow / 2.0
                    };
                    reader.fill_spread = Some(cur_spread);
                    reader.edge_since = None;
                }
                if p.zoom <= 1.01 {
                    p.pan.x = 0.0;
                }
                p.pan.y = p.pan.y.clamp(-overflow / 2.0, overflow / 2.0);
                pan_cell.set(p.pan);
            }
            g = geo(cur_spread);
        }
        // ---- PDF en desplazamiento continuo
        if scroll_mode {
            if let Some(t) = reader.turn.take() {
                let s = if t.target.unwrap_or(t.theta) > 1.5 {
                    t.a + 1
                } else {
                    t.a
                };
                reader.page = s.min(n_pages - 1);
            }
            let sizes = pdf_sizes.clone().unwrap_or_default();
            let col_w = (area.width() * 0.72).min(area.height() * 1.1).max(200.0) * zoom;
            let gap = 18.0;
            let mut tops = Vec::with_capacity(sizes.len() + 1);
            let mut y = 0.0;
            let dims: Vec<Vec2> = (0..sizes.len())
                .map(|p| {
                    let c = pdf_crops
                        .get(&p)
                        .copied()
                        .unwrap_or(Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)));
                    let (w, h) = sizes[p];
                    let (w, h) = (w * c.width(), h * c.height());
                    vec2(col_w, col_w * h / w.max(1.0))
                })
                .collect();
            for d in &dims {
                tops.push(y);
                y += d.y + gap;
            }
            let total = y;
            // salto externo (índice, barra, teclas)
            if reader.scroll_page != Some(reader.page) {
                reader.scroll_y = tops.get(reader.page).copied().unwrap_or(0.0) - 10.0;
            }
            let inp = ctx.input(|i| {
                (
                    i.smooth_scroll_delta,
                    i.key_pressed(Key::ArrowDown),
                    i.key_pressed(Key::ArrowUp),
                )
            });
            let (pointer_in, drag_dy) = ctx.input(|i| {
                let inside = i.pointer.hover_pos().is_some_and(|p| area.contains(p));
                let dy = if i.pointer.primary_down() && i.pointer.is_decidedly_dragging() {
                    i.pointer.delta().y
                } else {
                    0.0
                };
                (inside, dy)
            });
            if pointer_in && !reader.show_toc && !reader.show_settings {
                reader.scroll_y -= inp.0.y;
                if zoom > 1.01 {
                    if let Doc::Pdf(p) = &mut reader.doc {
                        p.pan.x += inp.0.x;
                    }
                }
            }
            if pointer_in {
                reader.scroll_y -= drag_dy;
            }
            if inp.1 {
                reader.scroll_y += 80.0;
            }
            if inp.2 {
                reader.scroll_y -= 80.0;
            }
            reader.scroll_y = reader
                .scroll_y
                .clamp(-20.0, (total - area.height()).max(0.0) + 20.0);
            let cx = area.center().x + pan.x;
            let mid = reader.scroll_y + area.height() * 0.4;
            let cur = tops.iter().rposition(|t| *t <= mid).unwrap_or(0);
            reader.page = cur;
            reader.scroll_page = Some(cur);
            let mut want = Vec::new();
            for (p, d) in dims.iter().enumerate() {
                let top = area.top() + tops[p] - reader.scroll_y;
                let r = Rect::from_min_size(pos2(cx - d.x / 2.0, top), *d);
                if r.bottom() < full.top() - 600.0 || r.top() > full.bottom() + 600.0 {
                    continue;
                }
                if let Doc::Pdf(_) = &reader.doc {
                    let (w, h) = sizes[p];
                    let ch = pdf_crops.get(&p).map(|c| c.height()).unwrap_or(1.0);
                    let s = (d.y * ppp / (h * ch)).min(5000.0 / w.max(h)).max(0.05);
                    want.push((p, s));
                }
                if r.bottom() < full.top() || r.top() > full.bottom() {
                    continue;
                }
                painter.add(
                    egui::epaint::Shadow {
                        offset: [0, 4],
                        blur: 16,
                        spread: 0,
                        color: Color32::from_black_alpha(90),
                    }
                    .as_shape(r, 2.0),
                );
                let tint = tint_for(settings.theme);
                painter.rect_filled(
                    r,
                    0.0,
                    if th.night {
                        Color32::from_gray(28)
                    } else {
                        tint
                    },
                );
                if let Doc::Pdf(pdf) = &reader.doc {
                    if let Some((t, _)) = pdf.tex.get(&p) {
                        let uv = pdf_crops
                            .get(&p)
                            .copied()
                            .unwrap_or(Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)));
                        painter.image(t.id(), r, uv, tint);
                    }
                }
            }
            if let Doc::Pdf(pdf) = &mut reader.doc {
                let ok = |v: Option<f32>, s: f32| {
                    v.is_some_and(|v| (v / s - 1.0).abs() < 0.2 || (v > s && v / s < 1.6))
                };
                let missing: Vec<(usize, f32)> = want
                    .iter()
                    .filter(|(p, s)| !ok(pdf.tex.get(p).map(|t| t.1), *s))
                    .copied()
                    .collect();
                let same = missing.len() == pdf.pending.len()
                    && missing
                        .iter()
                        .all(|(p, s)| ok(pdf.pending.get(p).copied(), *s));
                if !same {
                    pdf.pending = missing.iter().copied().collect();
                    if !missing.is_empty() {
                        pdf.worker.request(missing);
                    }
                }
                if pdf.tex.len() > 16 {
                    let keep: HashSet<usize> = want.iter().map(|w| w.0).collect();
                    pdf.tex.retain(|k, _| keep.contains(k));
                }
            }
        }

        // ---- PDF: peticiones de renderizado
        if let (Doc::Pdf(pdf), false) = (&mut reader.doc, scroll_mode) {
            let mut want: Vec<(usize, f32)> = Vec::new();
            let mut push = |p: Option<usize>, rect_h: f32, pdf: &PdfState| {
                if let Some(p) = p {
                    let (w, h) = pdf.sizes[p];
                    let ch = if crop_on {
                        pdf.crops.get(&p).map(|c| c.height()).unwrap_or(1.0)
                    } else {
                        1.0
                    };
                    let mut s = rect_h * ppp / (h * ch);
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
            // Páginas que faltan (o con resolución insuficiente). Se envía siempre
            // la lista completa: el hilo de render sustituye su cola por la última
            // petición, así ninguna página se queda "pendiente" para siempre.
            let ok = |v: Option<f32>, s: f32| {
                v.is_some_and(|v| (v / s - 1.0).abs() < 0.2 || (v > s && v / s < 1.6))
            };
            let missing: Vec<(usize, f32)> = want
                .iter()
                .filter(|(p, s)| !ok(pdf.tex.get(p).map(|t| t.1), *s))
                .copied()
                .collect();
            let same = missing.len() == pdf.pending.len()
                && missing
                    .iter()
                    .all(|(p, s)| ok(pdf.pending.get(p).copied(), *s));
            if !same {
                pdf.pending = missing.iter().copied().collect();
                if !missing.is_empty() {
                    pdf.worker.request(missing);
                }
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
            // PDF de borde a borde: ↓ / espacio bajan por la página; al final, siguiente página arriba
            let page_scroll = fill && overflow > 0.5 && reader.turn.is_none();
            let mut step = 0.0f32;
            if page_scroll {
                if input.key_pressed(Key::ArrowDown) {
                    step = 140.0;
                }
                if input.key_pressed(Key::Space) || input.key_pressed(Key::PageDown) {
                    step = full.height() * 0.85;
                }
                if input.key_pressed(Key::ArrowUp) {
                    step = -140.0;
                }
                if input.key_pressed(Key::PageUp) {
                    step = -full.height() * 0.85;
                }
            }
            if step != 0.0 {
                let lim = overflow / 2.0;
                let mut turn_dir = None;
                if let Doc::Pdf(p) = &mut reader.doc {
                    if step > 0.0 && p.pan.y <= -lim + 0.5 {
                        turn_dir = Some(true);
                    } else if step < 0.0 && p.pan.y >= lim - 0.5 {
                        turn_dir = Some(false);
                    } else {
                        p.pan.y = (p.pan.y - step).clamp(-lim, lim);
                        pan_cell.set(p.pan);
                    }
                }
                if let Some(fwd) = turn_dir {
                    start_turn(reader, fwd);
                }
            }
            let fwd_keys = input.key_pressed(Key::ArrowRight)
                || (!page_scroll
                    && (input.key_pressed(Key::Space) || input.key_pressed(Key::PageDown)));
            let back_keys = input.key_pressed(Key::ArrowLeft)
                || input.key_pressed(Key::Backspace)
                || (!page_scroll && input.key_pressed(Key::PageUp));
            if fwd_keys {
                start_turn(reader, true);
            }
            if back_keys {
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
            if input.modifiers.command && input.key_pressed(Key::F) {
                reader.show_toc = true;
                reader.panel_tab = 2;
            } else if input.key_pressed(Key::F) {
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
            if p.zoom > 1.01 {
                if resp.dragged() {
                    p.pan += resp.drag_delta();
                }
                p.pan += input.smooth_scroll_delta;
            }
        }

        if !zoomed && !scroll_mode {
            // rueda / gesto de dos dedos
            if resp.hovered() && reader.turn.is_none() {
                let mut delta = input.smooth_scroll_delta;
                // página más alta que la pantalla: la rueda la desplaza; al llegar al borde pasa página
                if fill && overflow > 0.5 {
                    // la página no se mueve sola: solo la rueda/trackpad en vertical la recorre
                    delta.x = 0.0;
                    if time - reader.last_turn < 0.6 {
                        // inercia del gesto anterior: la página nueva se queda fija arriba
                        delta.y = 0.0;
                        reader.edge_since = None;
                    }
                    if let Doc::Pdf(p) = &mut reader.doc {
                        let dy = delta.y;
                        let lim = overflow / 2.0;
                        let at_bottom = p.pan.y <= -lim + 0.5;
                        let at_top = p.pan.y >= lim - 0.5;
                        if (dy < 0.0 && at_bottom) || (dy > 0.0 && at_top) {
                            // en el borde: hay que hacer una pausa y volver a desplazar para pasar
                            let since = *reader.edge_since.get_or_insert(time);
                            delta.y = if time - since > 0.45 { dy } else { 0.0 };
                        } else {
                            if dy != 0.0 {
                                reader.edge_since = None;
                                reader.scroll_acc = Vec2::ZERO;
                            }
                            p.pan.y = (p.pan.y + dy).clamp(-lim, lim);
                            delta.y = 0.0;
                        }
                        pan_cell.set(p.pan);
                    }
                    if delta == Vec2::ZERO {
                        reader.scroll_acc = Vec2::ZERO;
                    }
                }
                reader.scroll_acc += delta;
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
            // ---- selección de texto (EPUB): arrastrar sobre el texto selecciona
            let hit_at = |reader: &Reader, pos: Pos2, slack: bool| -> Option<TextPos> {
                let Doc::Epub(ep) = &reader.doc else {
                    return None;
                };
                let l = ep.layout.as_ref()?;
                let s = sp.of(reader.page);
                let gg = geo(s);
                let (pl, pr) = sp.pages(s);
                for (p, r) in [pl.zip(gg.left), pr.zip(gg.right)].into_iter().flatten() {
                    if !r.expand(12.0).contains(pos) {
                        continue;
                    }
                    let (mx, mt, _) = margins(r.size());
                    return layout::hit(&l.pages[p], r.min + vec2(mx, mt), pos, slack);
                }
                None
            };
            let mut started_selection = false;
            if resp.drag_started() && reader.turn.is_none() {
                if let Some(ptr) = resp.interact_pointer_pos() {
                    if let Some(h) = hit_at(reader, ptr, false) {
                        reader.sel = Some((h, h));
                        reader.selecting = true;
                        reader.popup = None;
                        started_selection = true;
                    }
                }
            }
            if reader.selecting {
                if let (Some(ptr), Some((a, _))) = (pointer, reader.sel) {
                    if let Some(h) = hit_at(reader, ptr, true) {
                        if h.0 == a.0 {
                            reader.sel = Some((a, h));
                        }
                    }
                }
                if !resp.dragged() {
                    reader.selecting = false;
                    match reader.sel {
                        Some((a, b)) if a != b => {
                            let (a, b) = match &reader.doc {
                                Doc::Epub(ep) => layout::snap_words(&ep.epub, a, b),
                                _ => (a, b),
                            };
                            reader.sel = Some((a, b));
                            let p = pointer.unwrap_or(full.center());
                            reader.popup = Some(Popup {
                                pos: p,
                                kind: PopupKind::New(a.min(b), a.max(b)),
                            });
                        }
                        _ => reader.sel = None,
                    }
                }
            }
            let mut click_consumed = false;
            if resp.clicked() && reader.turn.is_none() {
                if reader.popup.is_some() || reader.sel.is_some() {
                    reader.popup = None;
                    reader.sel = None;
                    click_consumed = true;
                } else if let Some(ptr) = resp.interact_pointer_pos() {
                    if let Some(h) = hit_at(reader, ptr, false) {
                        if let Some(i) = notes.iter().position(|n| n.from() <= h && h < n.to()) {
                            reader.popup = Some(Popup {
                                pos: ptr,
                                kind: PopupKind::Existing(i),
                            });
                            click_consumed = true;
                        }
                    }
                }
            }
            // arrastrar la esquina
            if resp.drag_started() && !started_selection && !reader.selecting {
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
            if resp.clicked() && reader.turn.is_none() && !click_consumed {
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

        // ---- lectura en voz alta continua
        if reader.tts_on {
            let finished = match reader.tts.as_mut() {
                Some(c) => c.try_wait().ok().flatten().is_some(),
                None => true,
            };
            if finished && reader.turn.is_none() {
                let s = sp.of(reader.page);
                let first = sp.first(s);
                if reader.tts_page == Some(first) {
                    if s + 1 < sp.count() {
                        start_turn(reader, true);
                    } else {
                        reader.tts_on = false;
                    }
                } else {
                    let (pl, pr) = sp.pages(s);
                    let lang;
                    let text: Option<String> = match &reader.doc {
                        Doc::Epub(ep) => {
                            lang = ep.epub.language.clone();
                            ep.layout.as_ref().map(|l| {
                                [pl, pr]
                                    .into_iter()
                                    .flatten()
                                    .map(|p| layout::page_text(&l.pages[p]))
                                    .collect::<Vec<_>>()
                                    .join(" ")
                            })
                        }
                        Doc::Pdf(_) => {
                            lang = "es".into();
                            let path = self.saved.library[reader.entry].path.clone();
                            let pt = reader
                                .pdf_text
                                .get_or_insert_with(|| start_pdf_text(path, ctx.clone()))
                                .clone();
                            let g = pt.lock().ok();
                            g.as_ref().and_then(|g| g.as_ref()).map(|pages| {
                                [pl, pr]
                                    .into_iter()
                                    .flatten()
                                    .filter_map(|p| pages.get(p).cloned())
                                    .collect::<Vec<_>>()
                                    .join(" ")
                            })
                        }
                    };
                    if let Some(t) = text {
                        reader.tts = tools::speak(&t, &lang, settings.tts_rate);
                        reader.tts_page = Some(first);
                        if reader.tts.is_none() && t.trim().is_empty() {
                            // página sin texto: pasa a la siguiente
                        } else if reader.tts.is_none() {
                            reader.tts_on = false;
                            self.message = Some(("No se pudo usar la voz del sistema".into(), 0.0));
                        }
                    }
                }
            }
            ctx.request_repaint_after(std::time::Duration::from_millis(300));
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
        if fill {
            if let Doc::Pdf(p) = &reader.doc {
                pan_cell.set(p.pan);
            }
            g = geo(sp.of(reader.page));
        }
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
            None if scroll_mode => (None, None, None),
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
        let search_hit = reader.search_hit;
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
                        let uv = if crop_on {
                            pdf.crops
                                .get(&page)
                                .copied()
                                .unwrap_or(Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)))
                        } else {
                            Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0))
                        };
                        out.push(book::strip(
                            rect,
                            side,
                            t.id(),
                            uv,
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
                    for n in &notes {
                        if n.chapter != pg.chapter {
                            continue;
                        }
                        for r in layout::range_rects(pg, text.min, n.from(), n.to()) {
                            shapes.push((
                                rect,
                                Shape::rect_filled(r, 2.0, note_fill(n.color, th.night)),
                            ));
                            if !n.note.is_empty() {
                                shapes.push((
                                    rect,
                                    Shape::line_segment(
                                        [r.left_bottom(), r.right_bottom()],
                                        egui::Stroke::new(1.5, th.faint),
                                    ),
                                ));
                            }
                        }
                    }
                    if let Some((a, b)) = search_hit {
                        for r in layout::range_rects(pg, text.min, a, b) {
                            shapes.push((
                                rect,
                                Shape::rect_filled(
                                    r,
                                    2.0,
                                    Color32::from_rgba_unmultiplied(255, 140, 0, 120),
                                ),
                            ));
                        }
                    }
                    for it in &pg.items {
                        match it {
                            Item::Bullet { galley, offset } => {
                                let ts = egui::epaint::TextShape::new(
                                    text.min + *offset,
                                    galley.clone(),
                                    th.ink,
                                )
                                .with_override_text_color(th.ink);
                                shapes.push((rect, Shape::Text(ts)));
                            }
                            Item::Text {
                                galley,
                                offset,
                                clip,
                                ..
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
        // selección en curso
        if let (Some((a, b)), None, Doc::Epub(ep)) = (reader.sel, reader.turn, &reader.doc) {
            if let Some(l) = &ep.layout {
                for (p, r) in [statics_l, statics_r].into_iter().flatten() {
                    let (mx, mt, _) = margins(r.size());
                    for rr in layout::range_rects(&l.pages[p], r.min + vec2(mx, mt), a, b) {
                        painter.rect_filled(
                            rr,
                            2.0,
                            Color32::from_rgba_unmultiplied(60, 130, 230, 70),
                        );
                    }
                }
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
                                if ui.selectable_label(open_toc, "Índice y notas").clicked() {
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
                                            .button(if double { "1 página" } else { "2 páginas" })
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
                                            .selectable_label(
                                                reader.tts_on,
                                                if reader.tts_on { "Parar" } else { "Leer" },
                                            )
                                            .on_hover_text("Leer en voz alta")
                                            .clicked()
                                        {
                                            if reader.tts_on {
                                                reader.tts_on = false;
                                                if let Some(mut c) = reader.tts.take() {
                                                    let _ = c.kill();
                                                }
                                            } else {
                                                if let Some(mut c) = reader.tts.take() {
                                                    let _ = c.kill();
                                                }
                                                reader.tts_on = true;
                                                reader.tts_page = None;
                                            }
                                        }
                                        if ui
                                            .button("Buscar")
                                            .on_hover_text("Buscar en el libro (⌘F)")
                                            .clicked()
                                        {
                                            open_toc = true;
                                            reader.panel_tab = 2;
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
                                        // el título ocupa solo el hueco libre, recortado si no cabe
                                        let w = ui.available_width();
                                        if w > 60.0 {
                                            ui.add_sized(
                                                [w, 24.0],
                                                egui::Label::new(
                                                    egui::RichText::new(title.clone()).font(
                                                        FontId::new(
                                                            16.0,
                                                            FontFamily::Name("serif-b".into()),
                                                        ),
                                                    ),
                                                )
                                                .truncate(),
                                            );
                                        }
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

        // ---- menú de selección / subrayado
        if let Some(pp) = reader.popup.clone() {
            let lang = match &reader.doc {
                Doc::Epub(ep) => ep.epub.language.clone(),
                _ => "es".into(),
            };
            let (text, cur_color) = match &pp.kind {
                PopupKind::New(a, b) => match &reader.doc {
                    Doc::Epub(ep) => (layout::range_text(&ep.epub, *a, *b), None),
                    _ => (String::new(), None),
                },
                PopupKind::Existing(i) => notes
                    .get(*i)
                    .map(|n| (n.text.clone(), Some(n.color)))
                    .unwrap_or_default(),
            };
            let pos = pos2(
                (pp.pos.x - 170.0).clamp(full.left() + 8.0, full.right() - 360.0),
                (pp.pos.y + 14.0).min(full.bottom() - 120.0),
            );
            egui::Area::new(egui::Id::NULL.with("menu_sel"))
                .fixed_pos(pos)
                .order(egui::Order::Foreground)
                .show(&ctx, |ui| {
                    egui::Frame::popup(ui.style())
                        .inner_margin(egui::Margin::same(8))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                for (ci, (r, g, b)) in NOTE_COLORS.iter().enumerate() {
                                    let (rect, rr) =
                                        ui.allocate_exact_size(vec2(26.0, 26.0), Sense::click());
                                    ui.painter().circle_filled(
                                        rect.center(),
                                        10.0,
                                        Color32::from_rgb(*r, *g, *b),
                                    );
                                    if cur_color == Some(ci as u8) {
                                        ui.painter().circle_stroke(
                                            rect.center(),
                                            12.0,
                                            egui::Stroke::new(2.0, txt),
                                        );
                                    }
                                    if rr.on_hover_text("Subrayar").clicked() {
                                        note_ops.push(match &pp.kind {
                                            PopupKind::New(a, b) => {
                                                NoteOp::Add(*a, *b, ci as u8, text.clone(), false)
                                            }
                                            PopupKind::Existing(i) => NoteOp::Color(*i, ci as u8),
                                        });
                                    }
                                }
                                ui.separator();
                                match &pp.kind {
                                    PopupKind::New(a, b) => {
                                        if ui.button("Nota").clicked() {
                                            note_ops.push(NoteOp::Add(
                                                *a,
                                                *b,
                                                0,
                                                text.clone(),
                                                true,
                                            ));
                                        }
                                    }
                                    PopupKind::Existing(i) => {
                                        if ui.button("Nota").clicked() {
                                            note_ops.push(NoteOp::Edit(*i));
                                        }
                                        if ui.button("Borrar").clicked() {
                                            note_ops.push(NoteOp::Delete(*i));
                                        }
                                    }
                                }
                            });
                            ui.horizontal(|ui| {
                                if ui.button("Copiar").clicked() {
                                    ctx.copy_text(text.clone());
                                    note_ops.push(NoteOp::Close);
                                }
                                let first_word: String =
                                    text.split_whitespace().next().unwrap_or("").to_string();
                                if ui.button("Diccionario").clicked() {
                                    tools::dictionary(&first_word);
                                    note_ops.push(NoteOp::Close);
                                }
                                if ui
                                    .button("RAE")
                                    .on_hover_text("Diccionario de la lengua española")
                                    .clicked()
                                {
                                    tools::rae(&first_word);
                                    note_ops.push(NoteOp::Close);
                                }
                                if ui.button("Traducir").clicked() {
                                    tools::translate(&text);
                                    note_ops.push(NoteOp::Close);
                                }
                                if ui
                                    .button("Leer")
                                    .on_hover_text("Leer en voz alta")
                                    .clicked()
                                {
                                    if let Some(mut c) = reader.tts.take() {
                                        let _ = c.kill();
                                    }
                                    reader.tts_on = false;
                                    reader.tts = tools::speak(&text, &lang, settings.tts_rate);
                                    note_ops.push(NoteOp::Close);
                                }
                            });
                        });
                });
        }

        // ---- editor de notas
        let mut close_editor = false;
        if let Some(ed) = reader.note_edit.as_mut() {
            egui::Window::new("Nota")
                .collapsible(false)
                .resizable(false)
                .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
                .show(&ctx, |ui| {
                    if let Some(i) = ed.idx {
                        if let Some(n) = notes.get(i) {
                            if !n.text.is_empty() {
                                let mut q: String = n.text.chars().take(300).collect();
                                if n.text.chars().count() > 300 {
                                    q.push('…');
                                }
                                ui.label(egui::RichText::new(format!("«{q}»")).italics());
                                ui.add_space(6.0);
                            }
                        }
                    } else {
                        ui.label(format!("Nota de la página {}", ed.page + 1));
                    }
                    ui.add(
                        egui::TextEdit::multiline(&mut ed.text)
                            .desired_width(420.0)
                            .desired_rows(6)
                            .hint_text("Escribe tu nota…"),
                    );
                    ui.horizontal(|ui| {
                        if ui.button("Guardar").clicked() {
                            note_ops.push(NoteOp::SetText(ed.idx, ed.page, ed.text.clone()));
                            close_editor = true;
                        }
                        if ui.button("Cancelar").clicked() {
                            close_editor = true;
                        }
                    });
                });
        }
        if close_editor {
            reader.note_edit = None;
        }

        // ---- panel lateral: índice, notas y búsqueda
        if open_toc {
            egui::Area::new(egui::Id::NULL.with("indice"))
                .fixed_pos(full.left_top() + vec2(0.0, 48.0))
                .order(egui::Order::Foreground)
                .show(&ctx, |ui| {
                    let panel_fill = if th.night { Color32::from_rgb(24, 24, 26) } else { Color32::from_rgb(248, 247, 244) };
                    egui::Frame::new().fill(panel_fill).inner_margin(egui::Margin::same(14)).show(ui, |ui| {
                        ui.set_width(320.0);
                        ui.set_height(full.height() - 110.0);
                        ui.visuals_mut().override_text_color = Some(txt);
                        ui.horizontal(|ui| {
                            for (i, n) in ["Índice", "Notas", "Buscar"].iter().enumerate() {
                                if ui.selectable_label(reader.panel_tab == i as u8, *n).clicked() {
                                    reader.panel_tab = i as u8;
                                }
                            }
                        });
                        ui.separator();
                        match reader.panel_tab {
                            // ---------- Índice
                            0 => {
                                egui::ScrollArea::vertical().show(ui, |ui| {
                                    match &reader.doc {
                                        Doc::Epub(ep) => {
                                            let cur = sp.first(sp.of(reader.page));
                                            for t in &ep.epub.toc {
                                                let p = ep.layout.as_ref().map(|l| l.page_of(t.chapter, t.block)).unwrap_or(0);
                                                ui.horizontal(|ui| {
                                                    ui.add_space(t.level as f32 * 14.0);
                                                    let sel = cur >= p
                                                        && ep.layout.as_ref().is_some_and(|l| l.pages[cur].chapter == t.chapter);
                                                    if ui.selectable_label(sel, &t.title).clicked() {
                                                        jump_anchor = Some((t.chapter, t.block));
                                                    }
                                                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                                        ui.weak(format!("{}", p + 1));
                                                    });
                                                });
                                            }
                                            if ep.epub.toc.is_empty() {
                                                ui.weak("Este libro no incluye índice.");
                                            }
                                        }
                                        Doc::Pdf(pdf) => {
                                            let outline = pdf.outline.lock().ok().and_then(|o| o.clone());
                                            match outline {
                                                None => {
                                                    ui.weak("Leyendo el índice del PDF…");
                                                }
                                                Some(v) if v.is_empty() => {
                                                    ui.weak("Este PDF no incluye índice.");
                                                }
                                                Some(v) => {
                                                    for (t, lvl, p) in v {
                                                        ui.horizontal(|ui| {
                                                            ui.add_space(lvl as f32 * 14.0);
                                                            if ui.selectable_label(reader.page == p, &t).clicked() {
                                                                jump = Some(p);
                                                            }
                                                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                                                ui.weak(format!("{}", p + 1));
                                                            });
                                                        });
                                                    }
                                                }
                                            }
                                        }
                                    }
                                    ui.add_space(14.0);
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
                                                let ch = ep.epub.chapter_title(b.0).unwrap_or("").to_string();
                                                (format!("Pág. {} — {}", p + 1, ch), p)
                                            }
                                        };
                                        if ui.selectable_label(false, label).clicked() {
                                            jump = Some(target);
                                        }
                                    }
                                });
                            }
                            // ---------- Notas
                            1 => {
                                ui.horizontal(|ui| {
                                    ui.label(format!("{} notas y subrayados", notes.len()));
                                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                        if !notes.is_empty() && ui.button("Exportar…").clicked() {
                                            note_ops.push(NoteOp::Export);
                                        }
                                        if matches!(reader.doc, Doc::Pdf(_)) && ui.button("+ Nota").clicked() {
                                            reader.note_edit = Some(NoteEdit { idx: None, page: reader.page, text: String::new() });
                                        }
                                    });
                                });
                                ui.add_space(6.0);
                                if notes.is_empty() {
                                    ui.weak(if matches!(reader.doc, Doc::Pdf(_)) {
                                        "Añade notas a cualquier página con «+ Nota»."
                                    } else {
                                        "Arrastra sobre el texto para seleccionarlo y elige un color o «Nota»."
                                    });
                                }
                                egui::ScrollArea::vertical().show(ui, |ui| {
                                    for (i, n) in notes.iter().enumerate() {
                                        let page = match &reader.doc {
                                            Doc::Epub(ep) => ep.layout.as_ref().map(|l| l.page_of_char(n.chapter, n.block, n.start)).unwrap_or(0),
                                            Doc::Pdf(_) => n.chapter,
                                        };
                                        let (r, g, b) = NOTE_COLORS[n.color as usize % 4];
                                        egui::Frame::new()
                                            .inner_margin(egui::Margin::same(6))
                                            .stroke(egui::Stroke::new(1.0, Color32::from_gray(if th.night { 60 } else { 210 })))
                                            .corner_radius(6.0)
                                            .show(ui, |ui| {
                                                ui.set_width(ui.available_width());
                                                ui.horizontal(|ui| {
                                                    let (rect, _) = ui.allocate_exact_size(vec2(4.0, 16.0), Sense::hover());
                                                    ui.painter().rect_filled(rect, 1.0, Color32::from_rgb(r, g, b));
                                                    ui.weak(format!("Pág. {}", page + 1));
                                                });
                                                if !n.text.is_empty() {
                                                    let mut q: String = n.text.chars().take(220).collect();
                                                    if n.text.chars().count() > 220 {
                                                        q.push('…');
                                                    }
                                                    ui.label(egui::RichText::new(q).italics());
                                                }
                                                if !n.note.is_empty() {
                                                    ui.label(egui::RichText::new(&n.note).strong());
                                                }
                                                ui.horizontal(|ui| {
                                                    if ui.small_button("Ir").clicked() {
                                                        jump = Some(page);
                                                    }
                                                    if ui.small_button("Editar nota").clicked() {
                                                        note_ops.push(NoteOp::Edit(i));
                                                    }
                                                    if ui.small_button("Borrar").clicked() {
                                                        note_ops.push(NoteOp::Delete(i));
                                                    }
                                                });
                                            });
                                        ui.add_space(4.0);
                                    }
                                });
                            }
                            // ---------- Buscar
                            _ => {
                                let r = ui.add(
                                    egui::TextEdit::singleline(&mut reader.search_q)
                                        .hint_text("Buscar en este libro")
                                        .desired_width(f32::INFINITY),
                                );
                                if reader.panel_tab == 2 && reader.search_results.is_empty() && reader.search_q.is_empty() {
                                    r.request_focus();
                                }
                                if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) || ui.button("Buscar").clicked() {
                                    do_search = true;
                                }
                                if reader.search_pending {
                                    ui.horizontal(|ui| {
                                        ui.spinner();
                                        ui.weak("Leyendo el texto del PDF…");
                                    });
                                    ctx.request_repaint_after(std::time::Duration::from_millis(250));
                                } else if !reader.search_q.is_empty() {
                                    ui.weak(format!("{} resultados", reader.search_results.len()));
                                }
                                egui::ScrollArea::vertical().show(ui, |ui| {
                                    for hit in reader.search_results.clone() {
                                        let label = egui::RichText::new(format!("Pág. {}  ·  {}", hit.page + 1, hit.context));
                                        if ui.add(egui::Button::new(label).wrap().frame_when_inactive(false)).clicked() {
                                            jump = Some(hit.page);
                                            if matches!(reader.doc, Doc::Epub(_)) {
                                                let end = (hit.pos.0, hit.pos.1, hit.pos.2 + hit.len);
                                                reader.search_hit = Some((hit.pos, end));
                                                note_ops.push(NoteOp::Refresh);
                                            }
                                        }
                                    }
                                });
                            }
                        }
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
                                ui.horizontal(|ui| {
                                    ui.checkbox(&mut s.hyphenate, "Partir palabras (guiones)");
                                    ui.checkbox(&mut s.publisher, "Estilo del editor")
                                        .on_hover_text("Respeta alineaciones, sangrías y tamaños definidos por el propio libro");
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
                                    egui::Slider::new(&mut s.margin, 0.02..=0.18).text("Márgenes"),
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
                            if is_pdf {
                                ui.add_space(8.0);
                                ui.label(egui::RichText::new("PDF").strong());
                                ui.checkbox(&mut s.pdf_fill, "Pantalla completa de borde a borde (ajustar al ancho)");
                                ui.checkbox(&mut s.pdf_crop, "Recortar márgenes blancos");
                                ui.checkbox(&mut s.pdf_scroll, "Desplazamiento continuo (sin pasar página)");
                            }
                            ui.add_space(8.0);
                            ui.label(egui::RichText::new("Lectura en voz alta").strong());
                            ui.add(egui::Slider::new(&mut s.tts_rate, 120..=320).text("Velocidad"));
                        });
                });
        }

        reader.show_settings = open_settings;
        reader.show_toc = open_toc;

        // ---- búsqueda
        if do_search
            || (reader.search_pending
                && reader
                    .pdf_text
                    .as_ref()
                    .is_some_and(|t| t.lock().is_ok_and(|g| g.is_some())))
        {
            let q = reader.search_q.trim().to_string();
            reader.search_results.clear();
            reader.search_hit = None;
            if !q.is_empty() {
                match &reader.doc {
                    Doc::Epub(ep) => {
                        reader.search_results = search_epub(ep, &q);
                    }
                    Doc::Pdf(_) => {
                        let path = self.saved.library[reader.entry].path.clone();
                        let pt = reader
                            .pdf_text
                            .get_or_insert_with(|| start_pdf_text(path, ctx.clone()))
                            .clone();
                        let guard = pt.lock().ok();
                        match guard.as_ref().and_then(|g| g.as_ref()) {
                            Some(pages) => {
                                reader.search_results = search_pages(pages, &q);
                                reader.search_pending = false;
                            }
                            None => reader.search_pending = true,
                        }
                    }
                }
            }
            if let Doc::Epub(ep) = &mut reader.doc {
                ep.cache.clear();
            }
        }

        // ---- operaciones sobre notas
        if !note_ops.is_empty() {
            let entry = &mut self.saved.library[reader.entry];
            for op in note_ops.drain(..) {
                match op {
                    NoteOp::Add(a, b, color, text, edit) => {
                        let (a, b) = (a.min(b), a.max(b));
                        entry.notes.push(Note {
                            chapter: a.0,
                            block: a.1,
                            start: a.2,
                            end_block: b.1,
                            end: b.2,
                            color,
                            text,
                            note: String::new(),
                            created: now(),
                        });
                        entry.notes.sort_by_key(|n| (n.chapter, n.block, n.start));
                        let idx = entry
                            .notes
                            .iter()
                            .position(|n| n.from() == a && n.to() == b);
                        reader.sel = None;
                        reader.popup = None;
                        if edit {
                            reader.note_edit = Some(NoteEdit {
                                idx,
                                page: reader.page,
                                text: String::new(),
                            });
                        }
                    }
                    NoteOp::Color(i, c) => {
                        if let Some(n) = entry.notes.get_mut(i) {
                            n.color = c;
                        }
                        reader.popup = None;
                    }
                    NoteOp::Edit(i) => {
                        let text = entry
                            .notes
                            .get(i)
                            .map(|n| n.note.clone())
                            .unwrap_or_default();
                        reader.note_edit = Some(NoteEdit {
                            idx: Some(i),
                            page: reader.page,
                            text,
                        });
                        reader.popup = None;
                    }
                    NoteOp::Delete(i) => {
                        if i < entry.notes.len() {
                            entry.notes.remove(i);
                        }
                        reader.popup = None;
                    }
                    NoteOp::SetText(idx, page, text) => match idx {
                        Some(i) => {
                            if let Some(n) = entry.notes.get_mut(i) {
                                n.note = text;
                            }
                        }
                        None => {
                            if !text.trim().is_empty() {
                                entry.notes.push(Note {
                                    chapter: page,
                                    note: text,
                                    created: now(),
                                    ..Default::default()
                                });
                                entry.notes.sort_by_key(|n| (n.chapter, n.block, n.start));
                            }
                        }
                    },
                    NoteOp::Close => {
                        reader.popup = None;
                        reader.sel = None;
                    }
                    NoteOp::Refresh => {}
                    NoteOp::Export => {
                        let md = export_notes(entry, &reader.doc);
                        let name = format!("{} - notas.md", entry.title.replace(['/', ':'], "-"));
                        if let Some(f) = rfd::FileDialog::new()
                            .set_file_name(&name)
                            .add_filter("Markdown", &["md"])
                            .save_file()
                        {
                            match std::fs::write(&f, md) {
                                Ok(()) => self.message = Some(("Notas exportadas".into(), 0.0)),
                                Err(e) => {
                                    self.message = Some((format!("No se pudo guardar: {e}"), 0.0))
                                }
                            }
                        }
                    }
                }
            }
            if let Doc::Epub(ep) = &mut reader.doc {
                ep.cache.clear();
            }
        }

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
    fn meta_editor_ui(&mut self, ctx: &egui::Context) {
        let Some((idx, title, author, subjects)) = self.meta_editor.as_mut() else {
            return;
        };
        let mut save = false;
        let mut close = false;
        egui::Window::new("Datos del libro")
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ctx, |ui| {
                ui.set_width(380.0);
                ui.label("Título");
                ui.add(egui::TextEdit::singleline(title).desired_width(f32::INFINITY));
                ui.label("Autor");
                ui.add(egui::TextEdit::singleline(author).desired_width(f32::INFINITY));
                ui.label("Temas (separados por comas)");
                ui.add(egui::TextEdit::singleline(subjects).desired_width(f32::INFINITY));
                ui.weak("Los cambios se guardan en la biblioteca, no en el archivo.");
                ui.horizontal(|ui| {
                    if ui.button("Guardar").clicked() {
                        save = true;
                    }
                    if ui.button("Cancelar").clicked() {
                        close = true;
                    }
                });
            });
        if save {
            if let Some(e) = self.saved.library.get_mut(*idx) {
                e.title = title.trim().to_string();
                e.author = author.trim().to_string();
                e.subjects = subjects
                    .split(',')
                    .map(|t| t.trim().to_string())
                    .filter(|t| !t.is_empty())
                    .collect();
                e.meta_v = 2;
                e.meta_done = true;
            }
            close = true;
        }
        if close {
            self.meta_editor = None;
        }
    }

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

/// Clave de orden: sin acentos ni artículos, en minúsculas.
fn sort_key(s: &str) -> String {
    let t: String = s
        .to_lowercase()
        .chars()
        .map(|c| match c {
            'á' | 'à' | 'ä' | 'â' => 'a',
            'é' | 'è' | 'ë' | 'ê' => 'e',
            'í' | 'ì' | 'ï' | 'î' => 'i',
            'ó' | 'ò' | 'ö' | 'ô' => 'o',
            'ú' | 'ù' | 'ü' | 'û' => 'u',
            '¿' | '¡' | '"' | '“' | '«' => ' ',
            c => c,
        })
        .collect();
    let t = t.trim_start();
    for art in ["el ", "la ", "los ", "las ", "the ", "un ", "una "] {
        if let Some(r) = t.strip_prefix(art) {
            return r.trim_start().to_string();
        }
    }
    t.to_string()
}

enum NoteOp {
    Add(TextPos, TextPos, u8, String, bool),
    Color(usize, u8),
    Edit(usize),
    Delete(usize),
    SetText(Option<usize>, usize, String),
    Close,
    Refresh,
    Export,
}

/// Minúsculas sin acentos, carácter a carácter (conserva los índices).
fn fold(c: char) -> char {
    match c {
        'á' | 'à' | 'ä' | 'â' | 'Á' | 'À' | 'Ä' | 'Â' => 'a',
        'é' | 'è' | 'ë' | 'ê' | 'É' | 'È' | 'Ë' | 'Ê' => 'e',
        'í' | 'ì' | 'ï' | 'î' | 'Í' | 'Ì' | 'Ï' | 'Î' => 'i',
        'ó' | 'ò' | 'ö' | 'ô' | 'Ó' | 'Ò' | 'Ö' | 'Ô' => 'o',
        'ú' | 'ù' | 'ü' | 'û' | 'Ú' | 'Ù' | 'Ü' | 'Û' => 'u',
        'ñ' | 'Ñ' => 'n',
        'ç' | 'Ç' => 'c',
        '\u{a0}' | '\n' => ' ',
        c => c.to_lowercase().next().unwrap_or(c),
    }
}

fn find_all(hay: &[char], needle: &[char]) -> Vec<usize> {
    let mut out = Vec::new();
    if needle.is_empty() || hay.len() < needle.len() {
        return out;
    }
    let mut i = 0;
    while i + needle.len() <= hay.len() {
        if hay[i..i + needle.len()] == *needle {
            out.push(i);
            i += needle.len();
        } else {
            i += 1;
        }
    }
    out
}

fn context(chars: &[char], at: usize, len: usize) -> String {
    let a = at.saturating_sub(40);
    let b = (at + len + 60).min(chars.len());
    let mut s: String = chars[a..b].iter().collect::<String>().replace('\n', " ");
    if a > 0 {
        s = format!("…{s}");
    }
    if b < chars.len() {
        s.push('…');
    }
    s
}

fn search_epub(ep: &EpubState, q: &str) -> Vec<SearchHit> {
    let needle: Vec<char> = q.chars().map(fold).collect();
    let mut out = Vec::new();
    for (ci, ch) in ep.epub.chapters.iter().enumerate() {
        for (bi, b) in ch.blocks.iter().enumerate() {
            let orig: Vec<char> = layout::block_text(b).chars().collect();
            let folded: Vec<char> = orig.iter().map(|c| fold(*c)).collect();
            for at in find_all(&folded, &needle) {
                let page = ep
                    .layout
                    .as_ref()
                    .map(|l| l.page_of_char(ci, bi, at))
                    .unwrap_or(0);
                out.push(SearchHit {
                    pos: (ci, bi, at),
                    len: needle.len(),
                    page,
                    context: context(&orig, at, needle.len()),
                });
                if out.len() >= 1000 {
                    return out;
                }
            }
        }
    }
    out
}

fn search_pages(pages: &[String], q: &str) -> Vec<SearchHit> {
    let needle: Vec<char> = q.chars().map(fold).collect();
    let mut out = Vec::new();
    for (pi, t) in pages.iter().enumerate() {
        let orig: Vec<char> = t.chars().collect();
        let folded: Vec<char> = orig.iter().map(|c| fold(*c)).collect();
        for at in find_all(&folded, &needle) {
            out.push(SearchHit {
                pos: (pi, 0, at),
                len: needle.len(),
                page: pi,
                context: context(&orig, at, needle.len()),
            });
            if out.len() >= 1000 {
                return out;
            }
        }
    }
    out
}

/// Extrae el texto de un PDF página a página en segundo plano.
fn start_pdf_text(
    path: String,
    ctx: egui::Context,
) -> std::sync::Arc<std::sync::Mutex<Option<Vec<String>>>> {
    let slot = std::sync::Arc::new(std::sync::Mutex::new(None));
    let s2 = slot.clone();
    std::thread::spawn(move || {
        let pages = std::fs::read(&path)
            .ok()
            .and_then(|d| {
                std::panic::catch_unwind(|| pdf_extract::extract_text_from_mem_by_pages(&d).ok())
                    .ok()
                    .flatten()
            })
            .unwrap_or_default();
        if let Ok(mut g) = s2.lock() {
            *g = Some(pages);
        }
        ctx.request_repaint();
    });
    slot
}

/// Índice (marcadores) del PDF en segundo plano.
fn start_pdf_outline(
    data: Vec<u8>,
) -> std::sync::Arc<std::sync::Mutex<Option<Vec<(String, usize, usize)>>>> {
    let slot = std::sync::Arc::new(std::sync::Mutex::new(None));
    let s2 = slot.clone();
    std::thread::spawn(move || {
        let v = std::panic::catch_unwind(|| {
            lopdf::Document::load_mem(&data)
                .ok()
                .and_then(|d| d.get_toc().ok())
                .map(|t| {
                    t.toc
                        .into_iter()
                        .map(|e| {
                            (
                                e.title.trim().to_string(),
                                e.level.saturating_sub(1),
                                e.page.saturating_sub(1),
                            )
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        })
        .unwrap_or_default();
        if let Ok(mut g) = s2.lock() {
            *g = Some(v);
        }
    });
    slot
}

fn export_notes(entry: &Entry, doc: &Doc) -> String {
    let mut s = format!("# {}\n", entry.title);
    if !entry.author.is_empty() {
        s.push_str(&format!("\n*{}*\n", entry.author));
    }
    let mut last_ch = usize::MAX;
    for n in &entry.notes {
        let (page, chapter_title) = match doc {
            Doc::Epub(ep) => (
                ep.layout
                    .as_ref()
                    .map(|l| l.page_of_char(n.chapter, n.block, n.start))
                    .unwrap_or(0),
                ep.epub.chapter_title(n.chapter).map(|s| s.to_string()),
            ),
            Doc::Pdf(_) => (n.chapter, None),
        };
        if let Some(t) = chapter_title {
            if n.chapter != last_ch {
                s.push_str(&format!("\n## {t}\n"));
                last_ch = n.chapter;
            }
        }
        s.push('\n');
        if !n.text.is_empty() {
            for l in n.text.lines() {
                s.push_str(&format!("> {l}\n"));
            }
            s.push('\n');
        }
        if !n.note.is_empty() {
            s.push_str(&format!("{}\n\n", n.note));
        }
        s.push_str(&format!("— pág. {}\n", page + 1));
    }
    s
}

/// Día local en formato AAAA-MM-DD.
fn today_key(offset_days: i64) -> String {
    (chrono::Local::now() - chrono::Duration::days(offset_days))
        .format("%Y-%m-%d")
        .to_string()
}

impl App {
    /// Suma tiempo de lectura (máx. 3 min entre interacciones).
    fn track_reading(&mut self, ctx: &egui::Context) {
        let now_i = std::time::Instant::now();
        let focused = ctx.input(|i| i.focused);
        let active = ctx.input(|i| !i.events.is_empty() || i.pointer.is_moving());
        if let Some(prev) = self.stat_tick {
            let gap = now_i.duration_since(prev).as_secs_f32().min(180.0);
            if focused && (active || self.reader.as_ref().is_some_and(|r| r.tts_on)) {
                self.stat_acc += gap;
            }
        }
        self.stat_tick = Some(now_i);
        if self.stat_acc >= 1.0 {
            let secs = self.stat_acc.floor();
            self.stat_acc -= secs;
            *self.saved.stats.entry(today_key(0)).or_default() += secs as u32;
        }
    }

    fn today_minutes(&self) -> u32 {
        self.saved.stats.get(&today_key(0)).copied().unwrap_or(0) / 60
    }

    fn streak(&self) -> u32 {
        let mut n = 0;
        let mut d = if self.saved.stats.get(&today_key(0)).copied().unwrap_or(0) >= 60 {
            0
        } else {
            1
        };
        while self.saved.stats.get(&today_key(d)).copied().unwrap_or(0) >= 60 {
            n += 1;
            d += 1;
        }
        n
    }

    fn stats_ui(&mut self, ui: &mut egui::Ui) {
        let ink = Color32::from_rgb(30, 30, 30);
        let accent = Color32::from_rgb(130, 30, 30);
        let goal = self.saved.settings.goal_min.max(1);
        let today = self.today_minutes();
        let big = |ui: &mut egui::Ui, v: String, label: &str| {
            ui.vertical(|ui| {
                ui.label(
                    egui::RichText::new(v)
                        .font(FontId::new(40.0, FontFamily::Name("serif-b".into())))
                        .color(ink),
                );
                ui.label(egui::RichText::new(label).color(Color32::GRAY));
            });
        };
        let year = chrono::Local::now().format("%Y").to_string();
        let year_secs: u32 = self
            .saved
            .stats
            .iter()
            .filter(|(k, _)| k.starts_with(&year))
            .map(|(_, v)| *v)
            .sum();
        let year_start = chrono::NaiveDate::parse_from_str(&format!("{year}-01-01"), "%Y-%m-%d")
            .ok()
            .and_then(|d| d.and_hms_opt(0, 0, 0))
            .map(|d| d.and_utc().timestamp() as u64)
            .unwrap_or(0);
        let finished: Vec<(String, String)> = self
            .saved
            .library
            .iter()
            .filter(|e| e.finished_at >= year_start && e.is_finished())
            .map(|e| (e.title.clone(), e.author_name().to_string()))
            .collect();
        let streak = self.streak();
        egui::ScrollArea::vertical()
            .auto_shrink(false)
            .show(ui, |ui| {
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 60.0;
                    big(ui, format!("{today} / {goal} min"), "Lectura de hoy");
                    big(ui, format!("{streak}"), "Días seguidos leyendo");
                    big(
                        ui,
                        format!("{:.1} h", year_secs as f32 / 3600.0),
                        &format!("Leído en {year}"),
                    );
                    big(
                        ui,
                        format!("{}", finished.len()),
                        &format!("Libros terminados en {year}"),
                    );
                });
                ui.add_space(16.0);
                // anillo de progreso del objetivo
                let (r, _) = ui.allocate_exact_size(
                    vec2(ui.available_width().min(700.0), 14.0),
                    Sense::hover(),
                );
                ui.painter().rect_filled(r, 7.0, Color32::from_gray(225));
                let f = (today as f32 / goal as f32).min(1.0);
                ui.painter().rect_filled(
                    Rect::from_min_size(r.min, vec2(r.width() * f, r.height())),
                    7.0,
                    accent,
                );
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.label("Objetivo diario:");
                    ui.add(
                        egui::Slider::new(&mut self.saved.settings.goal_min, 5..=180)
                            .suffix(" min"),
                    );
                });
                ui.add_space(24.0);
                ui.label(egui::RichText::new("Últimos 30 días").strong().color(ink));
                ui.add_space(6.0);
                let w = ui.available_width().min(900.0);
                let (rect, _) = ui.allocate_exact_size(vec2(w, 170.0), Sense::hover());
                let p = ui.painter();
                let days: Vec<(String, u32)> = (0..30)
                    .rev()
                    .map(|d| {
                        (
                            today_key(d),
                            self.saved.stats.get(&today_key(d)).copied().unwrap_or(0) / 60,
                        )
                    })
                    .collect();
                let maxv = days.iter().map(|d| d.1).max().unwrap_or(0).max(goal).max(1) as f32;
                let bw = w / 30.0;
                let base = rect.bottom() - 20.0;
                let gy = base - (goal as f32 / maxv) * 130.0;
                p.line_segment(
                    [pos2(rect.left(), gy), pos2(rect.right(), gy)],
                    egui::Stroke::new(1.0, Color32::from_rgb(200, 150, 150)),
                );
                p.text(
                    pos2(rect.right(), gy - 2.0),
                    Align2::RIGHT_BOTTOM,
                    "objetivo",
                    FontId::proportional(11.0),
                    Color32::from_rgb(180, 120, 120),
                );
                for (i, (k, v)) in days.iter().enumerate() {
                    let hgt = (*v as f32 / maxv) * 130.0;
                    let x = rect.left() + i as f32 * bw;
                    let col = if *v >= goal {
                        accent
                    } else {
                        Color32::from_rgb(190, 160, 150)
                    };
                    p.rect_filled(
                        Rect::from_min_max(pos2(x + 3.0, base - hgt), pos2(x + bw - 3.0, base)),
                        3.0,
                        col,
                    );
                    if i % 5 == 4 || i == 29 {
                        p.text(
                            pos2(x + bw / 2.0, base + 4.0),
                            Align2::CENTER_TOP,
                            &k[8..],
                            FontId::proportional(11.0),
                            Color32::GRAY,
                        );
                    }
                }
                ui.add_space(24.0);
                ui.label(
                    egui::RichText::new(format!("Terminados en {year}"))
                        .strong()
                        .color(ink),
                );
                if finished.is_empty() {
                    ui.weak("Todavía ninguno. ¡A por el primero!");
                }
                for (t, a) in &finished {
                    ui.label(format!("• {t} — {a}"));
                }
            });
    }
}

fn l_page(ep: &EpubState, b: (usize, usize)) -> Option<usize> {
    ep.layout.as_ref().map(|l| l.page_of(b.0, b.1))
}

#[allow(dead_code)]
fn _unused(_: Pos2) {}
