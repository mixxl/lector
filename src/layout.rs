//! Paginación de EPUB: convierte los bloques en páginas como en un libro impreso.
//! Corte de líneas propio con partición silábica (guiones) y justificación por palabras.

use crate::epub::{Block, Epub, ParaKind, Span};
use eframe::egui::{
    self, Align, Color32, FontFamily, FontId, Rect, Vec2, pos2,
    text::{ByteIndex, LayoutJob, LayoutSection, TextFormat, TextWrapping},
    vec2,
};
use hyphenation::{Hyphenator, Language, Load, Standard};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Params {
    pub width: f32,
    pub height: f32,
    pub font_size: f32,
    pub line_spacing: f32,
    pub sans: bool,
    pub justify: bool,
    pub hyphenate: bool,
    pub publisher: bool,
    pub ppp: f32,
}

pub enum Item {
    /// Una línea de texto. `offset` relativo al área de texto; `clip` (y0, y1) en coordenadas de página.
    Text {
        galley: Arc<egui::Galley>,
        offset: Vec2,
        clip: (f32, f32),
        chapter: usize,
        block: usize,
        /// primer carácter de la línea dentro del párrafo
        char0: usize,
        /// caracteres del párrafo en esta línea (sin el guion añadido)
        nchars: usize,
        /// la línea termina con guion de partición
        hyph: bool,
    },
    Image {
        path: String,
        rect: Rect,
    },
    Ornament {
        y: f32,
    },
    /// Viñeta de lista (no forma parte del texto)
    Bullet {
        galley: Arc<egui::Galley>,
        offset: Vec2,
    },
}

pub struct Page {
    pub items: Vec<Item>,
    pub chapter: usize,
    pub block: usize,
    /// Primera página de capítulo o página de imagen: sin cabecera
    pub plain: bool,
}

pub struct Layout {
    pub pages: Vec<Page>,
    pub params: Params,
}

impl Layout {
    pub fn page_of(&self, chapter: usize, block: usize) -> usize {
        let mut best = 0;
        for (i, p) in self.pages.iter().enumerate() {
            if (p.chapter, p.block) <= (chapter, block) {
                best = i;
            } else {
                break;
            }
        }
        best
    }

    /// Página que contiene un carácter concreto de un párrafo.
    pub fn page_of_char(&self, chapter: usize, block: usize, ch: usize) -> usize {
        let start = self.page_of(chapter, block);
        for (i, p) in self.pages.iter().enumerate().skip(start) {
            if p.chapter > chapter {
                break;
            }
            for it in &p.items {
                if let Item::Text {
                    chapter: c,
                    block: b,
                    char0,
                    nchars,
                    ..
                } = it
                {
                    if *c == chapter && *b == block && ch >= *char0 && ch < char0 + (*nchars).max(1)
                    {
                        return i;
                    }
                }
            }
        }
        start
    }
}

/// Texto plano de un párrafo (el mismo que se maqueta).
pub fn block_text(b: &Block) -> String {
    match b {
        Block::Para { spans, .. } => spans.iter().map(|s| s.text.as_str()).collect(),
        _ => String::new(),
    }
}

fn family(sans: bool, bold: bool, italic: bool) -> FontFamily {
    if sans {
        return FontFamily::Proportional;
    }
    FontFamily::Name(
        match (bold, italic) {
            (false, false) => "serif",
            (false, true) => "serif-i",
            (true, false) => "serif-b",
            (true, true) => "serif-bi",
        }
        .into(),
    )
}

fn image_size(epub: &mut Epub, path: &str) -> Option<(f32, f32)> {
    let bytes = epub.image_bytes(path)?;
    let r = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .ok()?;
    let (w, h) = r.into_dimensions().ok()?;
    Some((w as f32, h as f32))
}

/// Diccionarios de partición, cargados una sola vez por idioma.
fn hyphenator(lang: &str) -> Option<Arc<Standard>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Option<Arc<Standard>>>>> = OnceLock::new();
    let l = lang.to_lowercase();
    let language = match l.get(..2).unwrap_or("") {
        "es" | "" => Language::Spanish,
        "en" => Language::EnglishUS,
        "fr" => Language::French,
        "de" => Language::German1996,
        "it" => Language::Italian,
        "pt" => Language::Portuguese,
        "ca" => Language::Catalan,
        _ => return None,
    };
    let key = format!("{language:?}");
    let mut c = CACHE
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .ok()?;
    c.entry(key)
        .or_insert_with(|| Standard::from_embedded(language).ok().map(Arc::new))
        .clone()
}

/// Estilo de un tramo de texto.
fn fmt_for(p: &Params, s: &Span, scale: f32, bold_all: bool) -> TextFormat {
    let rel = if s.small {
        0.82
    } else {
        s.scale.clamp(1.0, 2.0)
    };
    let size = p.font_size * scale * rel;
    TextFormat {
        font_id: FontId::new(size, family(p.sans, s.bold || bold_all, s.italic)),
        line_height: Some(p.font_size * scale * p.line_spacing),
        color: Color32::BLACK,
        italics: p.sans && s.italic,
        ..Default::default()
    }
}

struct Line {
    galley: Arc<egui::Galley>,
    x: f32,
    char0: usize,
    nchars: usize,
    hyph: bool,
}

/// Construye un LayoutJob con el tramo de caracteres [a, b) del párrafo.
fn job_for(
    chars: &[char],
    styles: &[usize],
    formats: &[TextFormat],
    a: usize,
    b: usize,
    hyphen: bool,
) -> LayoutJob {
    let mut job = LayoutJob {
        wrap: TextWrapping {
            max_width: f32::INFINITY,
            ..Default::default()
        },
        break_on_newline: false,
        ..Default::default()
    };
    let mut i = a;
    while i < b {
        let st = styles[i];
        let mut j = i;
        while j < b && styles[j] == st {
            j += 1;
        }
        let text: String = chars[i..j].iter().collect();
        let start = job.text.len();
        job.text.push_str(&text);
        job.sections.push(LayoutSection {
            leading_space: 0.0,
            byte_range: ByteIndex(start)..ByteIndex(job.text.len()),
            format: formats[st].clone(),
        });
        i = j;
    }
    if hyphen || a == b {
        let st = if b > a {
            styles[b - 1]
        } else {
            styles.get(a).copied().unwrap_or(0)
        };
        let start = job.text.len();
        job.text.push_str(if hyphen { "-" } else { "" });
        job.sections.push(LayoutSection {
            leading_space: 0.0,
            byte_range: ByteIndex(start)..ByteIndex(job.text.len()),
            format: formats[st].clone(),
        });
    }
    job
}

/// Corte de líneas de un párrafo con guiones y justificación.
#[allow(clippy::too_many_arguments)]
fn layout_para(
    ctx: &egui::Context,
    p: &Params,
    spans: &[Span],
    scale: f32,
    bold_all: bool,
    width: f32,
    first_indent: f32,
    align: Align,
    justify: bool,
    hy: Option<&Standard>,
    hcache: &mut HashMap<String, Vec<usize>>,
) -> Vec<Line> {
    let mut chars: Vec<char> = Vec::new();
    let mut styles: Vec<usize> = Vec::new();
    let mut formats: Vec<TextFormat> = Vec::new();
    for s in spans {
        formats.push(fmt_for(p, s, scale, bold_all));
        for c in s.text.chars() {
            chars.push(c);
            styles.push(formats.len() - 1);
        }
    }
    if formats.is_empty() {
        return vec![];
    }
    let mut lines = Vec::new();
    let mut seg_start = 0;
    let mut first_line = true;
    loop {
        let seg_end = chars[seg_start..]
            .iter()
            .position(|c| *c == '\n')
            .map(|i| seg_start + i)
            .unwrap_or(chars.len());
        let n = seg_end - seg_start;
        // medida de cada carácter en una sola fila sin cortes
        let full = ctx.fonts_mut(|f| {
            f.layout_job(job_for(
                &chars, &styles, &formats, seg_start, seg_end, false,
            ))
        });
        let glyphs: Vec<&egui::epaint::text::Glyph> =
            full.rows.iter().flat_map(|r| r.row.glyphs.iter()).collect();
        let xs: Vec<f32> = if glyphs.len() == n && full.rows.len() <= 1 {
            let mut v: Vec<f32> = glyphs.iter().map(|g| g.pos.x).collect();
            v.push(
                glyphs
                    .last()
                    .map(|g| g.pos.x + g.advance_width)
                    .unwrap_or(0.0),
            );
            v
        } else {
            let mut acc = 0.0;
            let mut v = vec![0.0];
            for i in seg_start..seg_end {
                acc += formats[styles[i]].font_id.size * 0.5;
                v.push(acc);
            }
            v
        };
        let hyphen_w = formats[styles.get(seg_start).copied().unwrap_or(0)]
            .font_id
            .size
            * 0.33;
        let ch = |i: usize| chars[seg_start + i];
        let wid = |a: usize, b: usize| xs[b] - xs[a];

        if n == 0 {
            let g = ctx.fonts_mut(|f| {
                f.layout_job(job_for(
                    &chars, &styles, &formats, seg_start, seg_start, false,
                ))
            });
            lines.push(Line {
                galley: g,
                x: 0.0,
                char0: seg_start,
                nchars: 0,
                hyph: false,
            });
        }
        let mut start = 0usize;
        while start < n {
            while start < n && ch(start) == ' ' {
                start += 1;
            }
            if start >= n {
                break;
            }
            let indent = if first_line { first_indent } else { 0.0 };
            let avail = (width - indent).max(20.0);
            let mut last_fit: Option<usize> = None;
            let mut k = start;
            let mut overflow_word: Option<(usize, usize)> = None;
            while k < n {
                let mut e = k;
                while e < n
                    && ch(e) != ' '
                    && !(e > k && matches!(ch(e - 1), '-' | '—' | '–' | '/'))
                {
                    e += 1;
                }
                if e == k {
                    e = k + 1;
                }
                if wid(start, e) <= avail {
                    last_fit = Some(e);
                    k = e;
                    while k < n && ch(k) == ' ' {
                        k += 1;
                    }
                } else {
                    overflow_word = Some((k, e));
                    break;
                }
            }
            let mut end = last_fit.unwrap_or(start);
            let mut hyph = false;
            if last_fit.is_some_and(|e| e >= n) {
                end = n;
            } else if let Some((ws, we)) = overflow_word {
                let fill = last_fit.map(|e| wid(start, e) / avail).unwrap_or(0.0);
                if let Some(h) = hy.filter(|_| p.hyphenate && fill < 0.93) {
                    let mut a = ws;
                    while a < we && !ch(a).is_alphabetic() {
                        a += 1;
                    }
                    let mut b = we;
                    while b > a && !ch(b - 1).is_alphabetic() {
                        b -= 1;
                    }
                    let all_alpha = (a..b).all(|i| ch(i).is_alphabetic());
                    if b > a + 4 && all_alpha {
                        let word: String = (a..b).map(ch).collect();
                        let breaks = hcache
                            .entry(word.clone())
                            .or_insert_with(|| {
                                let hw = h.hyphenate(&word);
                                hw.breaks
                                    .iter()
                                    .map(|bi| word[..*bi].chars().count())
                                    .collect()
                            })
                            .clone();
                        if let Some(&bp) = breaks
                            .iter()
                            .rev()
                            .find(|&&bp| wid(start, a + bp) + hyphen_w <= avail)
                        {
                            end = a + bp;
                            hyph = true;
                        }
                    }
                }
                if last_fit.is_none() && !hyph {
                    end = start + 1;
                    while end < n && wid(start, end + 1) <= avail {
                        end += 1;
                    }
                }
            }
            let mut vis_end = end;
            while vis_end > start && ch(vis_end - 1) == ' ' {
                vis_end -= 1;
            }
            let last_of_seg = end >= n;
            let job = job_for(
                &chars,
                &styles,
                &formats,
                seg_start + start,
                seg_start + vis_end,
                hyph,
            );
            let mut g = ctx.fonts_mut(|f| f.layout_job(job));
            let justified = justify && !last_of_seg;
            if justified {
                g = justify_line(g, avail);
            }
            let natural = if justified {
                avail
            } else {
                natural_width(&g).min(avail)
            };
            let x = indent
                + match align {
                    Align::Center => (avail - natural) / 2.0,
                    Align::Max => avail - natural,
                    _ => 0.0,
                };
            lines.push(Line {
                galley: g,
                x,
                char0: seg_start + start,
                nchars: vis_end - start,
                hyph,
            });
            first_line = false;
            start = end;
        }
        first_line = false;
        if seg_end >= chars.len() {
            break;
        }
        seg_start = seg_end + 1;
    }
    lines
}

fn natural_width(g: &egui::Galley) -> f32 {
    g.rows
        .first()
        .and_then(|r| {
            r.row
                .glyphs
                .iter()
                .rev()
                .find(|g| !g.chr.is_whitespace())
                .map(|gl| gl.pos.x + gl.advance_width)
        })
        .unwrap_or(0.0)
}

/// Justificación tipográfica: reparte el espacio sobrante solo entre las palabras.
fn justify_line(galley: Arc<egui::Galley>, maxw: f32) -> Arc<egui::Galley> {
    let mut g: egui::Galley = (*galley).clone();
    for placed in g.rows.iter_mut() {
        let row = &placed.row;
        let glyphs = &row.glyphs;
        let Some(first) = glyphs.iter().position(|g| !g.chr.is_whitespace()) else {
            continue;
        };
        let Some(last) = glyphs.iter().rposition(|g| !g.chr.is_whitespace()) else {
            continue;
        };
        let spaces = glyphs[first..=last].iter().filter(|g| g.chr == ' ').count();
        if spaces == 0 {
            continue;
        }
        let natural = glyphs[last].pos.x + glyphs[last].advance_width;
        let extra = maxw - natural;
        if extra <= 0.0 || extra > maxw * 0.4 {
            continue;
        }
        let per = extra / spaces as f32;
        let row = Arc::make_mut(&mut placed.row);
        let nverts = row.visuals.mesh.vertices.len() as u32;
        let starts: Vec<u32> = row.glyphs.iter().map(|g| g.first_vertex).collect();
        let mut shift = 0.0;
        for i in 0..row.glyphs.len() {
            if i > first && i <= last && row.glyphs[i - 1].chr == ' ' {
                shift += per;
            }
            if shift == 0.0 {
                continue;
            }
            row.glyphs[i].pos.x += shift;
            let a = starts[i];
            let b = starts.get(i + 1).copied().unwrap_or(nverts).min(nverts);
            if b <= a {
                continue;
            }
            for v in &mut row.visuals.mesh.vertices[a as usize..b as usize] {
                v.pos.x += shift;
            }
        }
        row.visuals.mesh_bounds.max.x = row.visuals.mesh_bounds.max.x.max(maxw);
        row.size.x = row.size.x.max(maxw);
    }
    g.rect.max.x = g.rect.max.x.max(maxw);
    g.mesh_bounds.max.x = g.mesh_bounds.max.x.max(maxw);
    Arc::new(g)
}

#[allow(unused_assignments)]
pub fn paginate(epub: &mut Epub, ctx: &egui::Context, p: Params) -> Layout {
    let w = p.width;
    let h = p.height;
    let fs = p.font_size;
    let line = fs * p.line_spacing;
    let mut pages: Vec<Page> = Vec::new();
    let hy = if p.hyphenate {
        hyphenator(&epub.language)
    } else {
        None
    };
    let mut hcache: HashMap<String, Vec<usize>> = HashMap::new();

    let mut cur = Page {
        items: vec![],
        chapter: 0,
        block: 0,
        plain: true,
    };
    let mut y = 0.0f32;

    let n_chapters = epub.chapters.len();
    for ci in 0..n_chapters {
        if !cur.items.is_empty() {
            pages.push(std::mem::replace(
                &mut cur,
                Page {
                    items: vec![],
                    chapter: ci,
                    block: 0,
                    plain: true,
                },
            ));
        }
        cur.chapter = ci;
        cur.block = 0;
        cur.plain = true;
        y = 0.0;

        let blocks = epub.chapters[ci].blocks.clone();
        let only_image = blocks.len() == 1 && matches!(blocks[0], Block::Image { .. });
        if matches!(
            blocks.first(),
            Some(Block::Para {
                kind: ParaKind::Heading(_),
                ..
            })
        ) {
            y = h * 0.12;
        }
        let mut prev_kind: Option<ParaKind> = None;
        let mut prev_was_para = false;

        for (bi, block) in blocks.iter().enumerate() {
            macro_rules! new_page {
                () => {{
                    if !cur.items.is_empty() {
                        pages.push(std::mem::replace(
                            &mut cur,
                            Page {
                                items: vec![],
                                chapter: ci,
                                block: bi,
                                plain: false,
                            },
                        ));
                    }
                    y = 0.0;
                }};
            }
            if cur.items.is_empty() && y == 0.0 {
                cur.block = bi;
            }
            match block {
                Block::Rule => {
                    if y + line * 2.0 > h {
                        new_page!();
                    }
                    cur.items.push(Item::Ornament { y: y + line * 0.5 });
                    y += line * 2.0;
                    prev_was_para = false;
                    prev_kind = None;
                }
                Block::Image { path } => {
                    let Some((iw, ih)) = image_size(epub, path) else {
                        continue;
                    };
                    if only_image || (bi == 0 && ci == 0) {
                        let s = (w / iw).min(h / ih);
                        let (dw, dh) = (iw * s, ih * s);
                        let rect =
                            Rect::from_min_size(pos2((w - dw) / 2.0, (h - dh) / 2.0), vec2(dw, dh));
                        if !cur.items.is_empty() {
                            new_page!();
                        }
                        cur.block = bi;
                        cur.items.push(Item::Image {
                            path: path.clone(),
                            rect,
                        });
                        cur.plain = true;
                        new_page!();
                        continue;
                    }
                    let mut s = (w / iw).min(h * 0.9 / ih).min(1.5);
                    if iw * s < 24.0 {
                        s = 1.0;
                    }
                    let (dw, dh) = (iw * s, ih * s);
                    let gap = if y > 0.0 { line * 0.5 } else { 0.0 };
                    if y + gap + dh > h {
                        new_page!();
                    }
                    let y0 = if y > 0.0 { y + gap } else { 0.0 };
                    cur.items.push(Item::Image {
                        path: path.clone(),
                        rect: Rect::from_min_size(pos2((w - dw) / 2.0, y0), vec2(dw, dh)),
                    });
                    y = y0 + dh + line * 0.5;
                    prev_was_para = false;
                    prev_kind = None;
                }
                Block::Para {
                    spans,
                    kind,
                    align: css_align,
                    indent: css_indent,
                    scale: css_scale,
                } => {
                    let kind = *kind;
                    if spans
                        .iter()
                        .all(|s| s.text.trim_matches(['\u{a0}', ' ', '\n']).is_empty())
                    {
                        y += line;
                        continue;
                    }
                    let (hscale, bold_all) = match kind {
                        ParaKind::Heading(1) => (1.55, false),
                        ParaKind::Heading(2) => (1.35, false),
                        ParaKind::Heading(3) => (1.18, true),
                        ParaKind::Heading(_) => (1.05, true),
                        _ => (1.0, false),
                    };
                    let scale = if p.publisher && !matches!(kind, ParaKind::Heading(_)) {
                        css_scale.clamp(0.7, 1.6)
                    } else {
                        hscale
                    };
                    let is_heading = matches!(kind, ParaKind::Heading(_));
                    let align = match (*css_align, kind) {
                        (2, _) | (_, ParaKind::Center) | (_, ParaKind::Heading(_)) => Align::Center,
                        (3, _) => Align::Max,
                        _ => Align::Min,
                    };
                    let justify = p.justify
                        && align == Align::Min
                        && kind != ParaKind::Pre
                        && !(p.publisher && *css_align == 1);
                    let left = match kind {
                        ParaKind::Quote => fs * 1.6,
                        ParaKind::ListItem => fs * 1.4,
                        _ => 0.0,
                    };
                    let width = w
                        - left
                        - if kind == ParaKind::Quote {
                            fs * 1.2
                        } else {
                            0.0
                        };
                    let auto_indent = kind == ParaKind::Body
                        && prev_was_para
                        && !matches!(
                            prev_kind,
                            Some(ParaKind::Heading(_)) | Some(ParaKind::Center)
                        );
                    let indent = if align != Align::Min {
                        0.0
                    } else if p.publisher {
                        css_indent.map(|e| e * fs).unwrap_or(0.0).max(0.0)
                    } else {
                        match css_indent {
                            Some(e) if *e <= 0.0 => 0.0,
                            _ if auto_indent => fs * 1.5,
                            _ => 0.0,
                        }
                    };
                    let lines = layout_para(
                        ctx,
                        &p,
                        spans,
                        scale,
                        bold_all,
                        width,
                        indent,
                        align,
                        justify,
                        hy.as_deref(),
                        &mut hcache,
                    );
                    if lines.is_empty() {
                        continue;
                    }
                    let mut before = if is_heading {
                        line * 1.2
                    } else if matches!(kind, ParaKind::Quote | ParaKind::Center | ParaKind::Pre)
                        || prev_kind.is_some_and(|k| k != kind)
                    {
                        line * 0.5
                    } else {
                        0.0
                    };
                    if p.publisher && kind == ParaKind::Body && prev_was_para && indent == 0.0 {
                        before = before.max(line * 0.5);
                    }
                    if y > 0.0 {
                        y += before;
                    }
                    if y >= h - line * 0.5 && !cur.items.is_empty() {
                        new_page!();
                    }
                    let hts: Vec<f32> = lines.iter().map(|l| l.galley.size().y.max(1.0)).collect();
                    let total_h: f32 = hts.iter().sum();
                    if is_heading && y > 0.0 && y + total_h + line * 3.0 > h {
                        new_page!();
                    }
                    let n = lines.len();
                    for (li, l) in lines.into_iter().enumerate() {
                        let lh = hts[li];
                        let remaining = n - li;
                        let orphan = li == 0 && n >= 3 && y > 0.0 && y + lh + hts[1] > h + 0.5;
                        let widow = remaining == 2
                            && n >= 3
                            && y > 0.0
                            && y + lh <= h + 0.5
                            && y + lh + hts[li + 1] > h + 0.5;
                        if y + lh > h + 0.5 || orphan || widow {
                            new_page!();
                        }
                        if kind == ParaKind::ListItem && li == 0 {
                            let f = TextFormat {
                                font_id: FontId::new(fs, family(p.sans, false, false)),
                                line_height: Some(line),
                                color: Color32::BLACK,
                                ..Default::default()
                            };
                            let mut job = LayoutJob::default();
                            job.append("•", 0.0, f);
                            let g = ctx.fonts_mut(|f| f.layout_job(job));
                            cur.items.push(Item::Bullet {
                                galley: g,
                                offset: vec2(left - fs * 0.9, y),
                            });
                        }
                        cur.items.push(Item::Text {
                            galley: l.galley,
                            offset: vec2(left + l.x, y),
                            clip: (y, y + lh),
                            chapter: ci,
                            block: bi,
                            char0: l.char0,
                            nchars: l.nchars,
                            hyph: l.hyph,
                        });
                        y += lh;
                    }
                    if is_heading {
                        y += line * 0.6;
                    }
                    prev_was_para = true;
                    prev_kind = Some(kind);
                }
            }
        }
    }
    if !cur.items.is_empty() || pages.is_empty() {
        pages.push(cur);
    }
    Layout { pages, params: p }
}

/// Posición de texto: (capítulo, párrafo, carácter).
pub type TextPos = (usize, usize, usize);

/// Carácter del texto bajo el puntero en una página.
pub fn hit(page: &Page, origin: egui::Pos2, pos: egui::Pos2, slack: bool) -> Option<TextPos> {
    let mut best: Option<(f32, TextPos)> = None;
    for it in &page.items {
        if let Item::Text {
            galley,
            offset,
            clip,
            chapter,
            block,
            char0,
            nchars,
            ..
        } = it
        {
            let y0 = origin.y + clip.0;
            let y1 = origin.y + clip.1;
            let x0 = origin.x + offset.x;
            let x1 = x0 + galley.size().x;
            let dy = if pos.y < y0 {
                y0 - pos.y
            } else if pos.y > y1 {
                pos.y - y1
            } else {
                0.0
            };
            let dx = if pos.x < x0 {
                x0 - pos.x
            } else if pos.x > x1 {
                pos.x - x1
            } else {
                0.0
            };
            if !slack && (dy > 0.0 || dx > 4.0) {
                continue;
            }
            let d = dy * 10.0 + dx;
            let local = pos.x - x0;
            let glyphs = galley.rows.first().map(|r| &r.row.glyphs);
            let mut idx = 0usize;
            if let Some(gs) = glyphs {
                for (i, g) in gs.iter().enumerate().take(*nchars) {
                    if g.pos.x + g.advance_width / 2.0 < local {
                        idx = i + 1;
                    }
                }
            }
            let tp = (*chapter, *block, char0 + idx.min(*nchars));
            if best.is_none_or(|(bd, _)| d < bd) {
                best = Some((d, tp));
            }
        }
    }
    best.map(|b| b.1)
}

/// Rectángulos que cubren el tramo [from, to) en una página.
pub fn range_rects(page: &Page, origin: egui::Pos2, from: TextPos, to: TextPos) -> Vec<Rect> {
    let (from, to) = if from <= to { (from, to) } else { (to, from) };
    let mut out = Vec::new();
    for it in &page.items {
        if let Item::Text {
            galley,
            offset,
            clip,
            chapter,
            block,
            char0,
            nchars,
            ..
        } = it
        {
            let a = (*chapter, *block, *char0);
            let b = (*chapter, *block, char0 + nchars);
            if b <= from || a >= to {
                continue;
            }
            let s = if from > a { from.2 - char0 } else { 0 };
            let e = if to < b { to.2 - char0 } else { *nchars };
            if e <= s {
                continue;
            }
            let Some(row) = galley.rows.first() else {
                continue;
            };
            let gs = &row.row.glyphs;
            if gs.is_empty() {
                continue;
            }
            let xa = gs.get(s).map(|g| g.pos.x).unwrap_or(0.0);
            let xb = gs
                .get(e - 1)
                .map(|g| g.pos.x + g.advance_width)
                .unwrap_or(xa);
            out.push(Rect::from_min_max(
                egui::pos2(origin.x + offset.x + xa, origin.y + clip.0 + 1.0),
                egui::pos2(origin.x + offset.x + xb, origin.y + clip.1 - 1.0),
            ));
        }
    }
    out
}

/// Texto de un tramo (dentro de un mismo capítulo).
pub fn range_text(epub: &Epub, from: TextPos, to: TextPos) -> String {
    let (from, to) = if from <= to { (from, to) } else { (to, from) };
    let Some(ch) = epub.chapters.get(from.0) else {
        return String::new();
    };
    let mut out = Vec::new();
    for b in from.1..=to.1.min(ch.blocks.len().saturating_sub(1)) {
        let t: Vec<char> = block_text(&ch.blocks[b]).chars().collect();
        let s = if b == from.1 { from.2.min(t.len()) } else { 0 };
        let e = if b == to.1 {
            to.2.min(t.len())
        } else {
            t.len()
        };
        if e > s {
            out.push(t[s..e].iter().collect::<String>());
        }
    }
    out.join("\n")
}

/// Texto de una página (para leer en voz alta).
pub fn page_text(page: &Page) -> String {
    let mut s = String::new();
    let mut last_block = None;
    for it in &page.items {
        if let Item::Text {
            galley,
            block,
            hyph,
            nchars,
            ..
        } = it
        {
            if last_block.is_some() && last_block != Some(*block) {
                s.push_str("\n");
            } else if !s.is_empty() && !s.ends_with('\u{1}') {
                s.push(' ');
            }
            s = s.trim_end_matches('\u{1}').to_string();
            let t: String = galley.job.text.chars().take(*nchars).collect();
            s.push_str(&t);
            if *hyph {
                s.push('\u{1}');
            }
            last_block = Some(*block);
        }
    }
    s.replace('\u{1}', "")
}

/// Ajusta una selección a palabras completas.
pub fn snap_words(epub: &Epub, a: TextPos, b: TextPos) -> (TextPos, TextPos) {
    let (mut a, mut b) = if a <= b { (a, b) } else { (b, a) };
    let get = |p: TextPos| -> Vec<char> {
        epub.chapters
            .get(p.0)
            .and_then(|c| c.blocks.get(p.1))
            .map(|bl| block_text(bl).chars().collect())
            .unwrap_or_default()
    };
    let ta = get(a);
    while a.2 > 0 && a.2 <= ta.len() && ta[a.2 - 1].is_alphanumeric() {
        a.2 -= 1;
    }
    let tb = get(b);
    while b.2 < tb.len() && tb[b.2].is_alphanumeric() {
        b.2 += 1;
    }
    (a, b)
}
