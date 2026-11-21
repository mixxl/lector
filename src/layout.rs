//! Paginación de EPUB: convierte los bloques en páginas como en un libro impreso.

use crate::epub::{Block, Epub, ParaKind};
use eframe::egui::{
    self, Align, Color32, FontFamily, FontId, Rect, Vec2, pos2,
    text::{LayoutJob, LayoutSection, TextFormat, TextWrapping},
    vec2,
};
use std::sync::Arc;

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Params {
    pub width: f32,
    pub height: f32,
    pub font_size: f32,
    pub line_spacing: f32,
    pub sans: bool,
    pub justify: bool,
    pub ppp: f32,
}

pub enum Item {
    /// Galería de texto; `offset` relativo al área de texto y `clip` (y0, y1) en coordenadas de página.
    Text {
        galley: Arc<egui::Galley>,
        offset: Vec2,
        clip: (f32, f32),
    },
    Image {
        path: String,
        rect: Rect,
    },
    Ornament {
        y: f32,
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

#[allow(unused_assignments)]
pub fn paginate(epub: &mut Epub, ctx: &egui::Context, p: Params) -> Layout {
    let w = p.width;
    let h = p.height;
    let fs = p.font_size;
    let line = fs * p.line_spacing;
    let mut pages: Vec<Page> = Vec::new();

    let mut cur = Page {
        items: vec![],
        chapter: 0,
        block: 0,
        plain: true,
    };
    let mut y = 0.0f32;

    let n_chapters = epub.chapters.len();
    for ci in 0..n_chapters {
        // Cada capítulo empieza en página nueva
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
            y = h * 0.16;
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
                        // Imagen a página completa (portadas, láminas)
                        let s = (w / iw).min(h / ih);
                        let (dw, dh) = (iw * s, ih * s);
                        let rect =
                            Rect::from_min_size(pos2((w - dw) / 2.0, (h - dh) / 2.0), vec2(dw, dh));
                        if !cur.items.is_empty() {
                            new_page!();
                        }
                        cur.items.push(Item::Image {
                            path: path.clone(),
                            rect,
                        });
                        cur.plain = true;
                        new_page!();
                        continue;
                    }
                    // Tamaño natural (px CSS ~ puntos) limitado al área
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
                Block::Para { spans, kind } => {
                    let kind = *kind;
                    let text_len: usize = spans.iter().map(|s| s.text.trim().len()).sum();
                    if text_len == 0
                        || spans
                            .iter()
                            .all(|s| s.text.trim_matches(['\u{a0}', ' ', '\n']).is_empty())
                    {
                        y += line;
                        continue;
                    }
                    let (scale, centered, bold_all) = match kind {
                        ParaKind::Heading(1) => (1.55, true, false),
                        ParaKind::Heading(2) => (1.35, true, false),
                        ParaKind::Heading(3) => (1.18, true, true),
                        ParaKind::Heading(_) => (1.05, true, true),
                        ParaKind::Center => (1.0, true, false),
                        _ => (1.0, false, false),
                    };
                    let is_heading = matches!(kind, ParaKind::Heading(_));
                    let left = match kind {
                        ParaKind::Quote => fs * 1.6,
                        ParaKind::ListItem => fs * 1.3,
                        _ => 0.0,
                    };
                    let width = w
                        - left
                        - if kind == ParaKind::Quote {
                            fs * 1.2
                        } else {
                            0.0
                        };
                    let indent = kind == ParaKind::Body
                        && prev_was_para
                        && !matches!(
                            prev_kind,
                            Some(ParaKind::Heading(_)) | Some(ParaKind::Center)
                        );

                    let mut job = LayoutJob::default();
                    job.wrap = TextWrapping {
                        max_width: width,
                        ..Default::default()
                    };
                    job.justify = false;
                    let do_justify = p.justify && !centered && kind != ParaKind::Pre;
                    job.halign = if centered { Align::Center } else { Align::Min };
                    job.break_on_newline = true;
                    if kind == ParaKind::ListItem {
                        let f = TextFormat {
                            font_id: FontId::new(fs, family(p.sans, false, false)),
                            line_height: Some(line),
                            color: Color32::BLACK,
                            ..Default::default()
                        };
                        job.append("•\u{2002}", 0.0, f);
                    }
                    let mut first = true;
                    for s in spans {
                        let size = fs * scale * if s.small { 0.8 } else { 1.0 };
                        let f = TextFormat {
                            font_id: FontId::new(
                                size,
                                family(p.sans, s.bold || bold_all, s.italic),
                            ),
                            line_height: Some(size * p.line_spacing),
                            color: Color32::BLACK,
                            italics: p.sans && s.italic,
                            ..Default::default()
                        };
                        let lead = if first && indent { fs * 1.5 } else { 0.0 };
                        first = false;
                        job.sections.push(LayoutSection {
                            leading_space: lead,
                            byte_range: egui::text::ByteIndex(job.text.len())
                                ..egui::text::ByteIndex(job.text.len() + s.text.len()),
                            format: f,
                        });
                        job.text.push_str(&s.text);
                    }
                    let mut galley = ctx.fonts_mut(|f| f.layout_job(job));
                    if do_justify {
                        galley = justify(galley, width);
                    }
                    let x = if centered { left + width / 2.0 } else { left };

                    // espacio antes
                    let before = if is_heading {
                        line * 1.2
                    } else if matches!(kind, ParaKind::Quote | ParaKind::Center | ParaKind::Pre)
                        || prev_kind.is_some_and(|k| k != kind)
                    {
                        line * 0.5
                    } else {
                        0.0
                    };
                    if y > 0.0 {
                        y += before;
                    }
                    if y >= h - line * 0.5 && !cur.items.is_empty() {
                        new_page!();
                    }

                    let rows = &galley.rows;
                    let total_h = rows.last().map(|r| r.max_y()).unwrap_or(0.0);
                    // Mantener título con lo siguiente
                    if is_heading && y > 0.0 && y + total_h + line * 3.0 > h {
                        new_page!();
                    }
                    let mut start = 0usize;
                    while start < rows.len() {
                        let top = rows[start].min_y();
                        let mut end = start;
                        while end < rows.len() && rows[end].max_y() - top <= h - y + 0.5 {
                            end += 1;
                        }
                        // Evita líneas huérfanas: no dejar 1 línea sola al pie
                        if start == 0 && end == 1 && rows.len() >= 3 && y > 0.0 {
                            end = 0;
                        }
                        // Evita viudas: no dejar 1 línea sola en la página siguiente
                        if end < rows.len() && end > 1 && rows.len() - end == 1 && end - start >= 3
                        {
                            end -= 1;
                        }
                        if end == start {
                            if y == 0.0 {
                                end = start + 1; // fila más alta que la página
                            } else {
                                new_page!();
                                continue;
                            }
                        }
                        let bottom = rows[end - 1].max_y();
                        cur.items.push(Item::Text {
                            galley: galley.clone(),
                            offset: vec2(x, y - top),
                            clip: (y, y + bottom - top),
                        });
                        y += bottom - top;
                        start = end;
                        if start < rows.len() {
                            new_page!();
                        }
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

/// Justificación tipográfica: reparte el espacio sobrante solo entre las
/// palabras (no entre letras), desplazando los vértices de cada glifo.
fn justify(galley: Arc<egui::Galley>, maxw: f32) -> Arc<egui::Galley> {
    let mut g: egui::Galley = (*galley).clone();
    let n = g.rows.len();
    for (ri, placed) in g.rows.iter_mut().enumerate() {
        if ri + 1 == n || placed.ends_with_newline {
            continue;
        }
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
        if extra <= 0.0 || extra > maxw * 0.35 {
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
    Arc::new(g)
}
