//! Diseño de portadas y su incorporación al propio archivo (EPUB o PDF).

use ab_glyph::{Font, FontRef, PxScale, ScaleFont, point};
use image::{DynamicImage, Rgb, RgbImage};
use std::io::{Read, Write};
use std::path::Path;

pub struct Style {
    pub name: &'static str,
    pub bg: [u8; 3],
    pub fg: [u8; 3],
    pub accent: [u8; 3],
}

pub const STYLES: [Style; 6] = [
    Style {
        name: "Clásica",
        bg: [243, 236, 220],
        fg: [40, 30, 25],
        accent: [130, 30, 30],
    },
    Style {
        name: "Granate",
        bg: [110, 28, 32],
        fg: [245, 232, 210],
        accent: [212, 175, 95],
    },
    Style {
        name: "Azul noche",
        bg: [24, 36, 64],
        fg: [235, 228, 210],
        accent: [212, 175, 95],
    },
    Style {
        name: "Verde inglés",
        bg: [28, 64, 48],
        fg: [238, 230, 210],
        accent: [212, 175, 95],
    },
    Style {
        name: "Negro",
        bg: [22, 22, 24],
        fg: [240, 236, 228],
        accent: [200, 60, 50],
    },
    Style {
        name: "Mostaza",
        bg: [214, 160, 48],
        fg: [30, 26, 22],
        accent: [30, 26, 22],
    },
];

static BOLD: &[u8] = include_bytes!("../assets/fonts/CrimsonText-Bold.ttf");
static ITALIC: &[u8] = include_bytes!("../assets/fonts/CrimsonText-Italic.ttf");

fn blend(img: &mut RgbImage, x: i32, y: i32, c: [u8; 3], a: f32) {
    if x < 0 || y < 0 || x >= img.width() as i32 || y >= img.height() as i32 {
        return;
    }
    let p = img.get_pixel_mut(x as u32, y as u32);
    for i in 0..3 {
        p.0[i] = (p.0[i] as f32 * (1.0 - a) + c[i] as f32 * a) as u8;
    }
}

fn rect(img: &mut RgbImage, x0: i32, y0: i32, x1: i32, y1: i32, c: [u8; 3]) {
    for y in y0..y1 {
        for x in x0..x1 {
            blend(img, x, y, c, 1.0);
        }
    }
}

fn frame(img: &mut RgbImage, inset: i32, t: i32, c: [u8; 3]) {
    let (w, h) = (img.width() as i32, img.height() as i32);
    rect(img, inset, inset, w - inset, inset + t, c);
    rect(img, inset, h - inset - t, w - inset, h - inset, c);
    rect(img, inset, inset, inset + t, h - inset, c);
    rect(img, w - inset - t, inset, w - inset, h - inset, c);
}

fn line_width(font: &FontRef, scale: PxScale, s: &str) -> f32 {
    let f = font.as_scaled(scale);
    let mut w = 0.0;
    let mut prev = None;
    for c in s.chars() {
        let id = f.glyph_id(c);
        if let Some(p) = prev {
            w += f.kern(p, id);
        }
        w += f.h_advance(id);
        prev = Some(id);
    }
    w
}

fn wrap(font: &FontRef, scale: PxScale, text: &str, maxw: f32) -> Vec<String> {
    let mut lines = Vec::new();
    let mut cur = String::new();
    for word in text.split_whitespace() {
        let cand = if cur.is_empty() {
            word.to_string()
        } else {
            format!("{cur} {word}")
        };
        if line_width(font, scale, &cand) > maxw && !cur.is_empty() {
            lines.push(std::mem::replace(&mut cur, word.to_string()));
        } else {
            cur = cand;
        }
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    lines
}

fn draw_line(
    img: &mut RgbImage,
    font: &FontRef,
    scale: PxScale,
    s: &str,
    cx: f32,
    baseline: f32,
    c: [u8; 3],
) {
    let f = font.as_scaled(scale);
    let mut x = cx - line_width(font, scale, s) / 2.0;
    let mut prev = None;
    for ch in s.chars() {
        let id = f.glyph_id(ch);
        if let Some(p) = prev {
            x += f.kern(p, id);
        }
        let g = id.with_scale_and_position(scale, point(x, baseline));
        if let Some(o) = font.outline_glyph(g) {
            let b = o.px_bounds();
            o.draw(|gx, gy, cov| {
                blend(
                    img,
                    b.min.x as i32 + gx as i32,
                    b.min.y as i32 + gy as i32,
                    c,
                    cov.min(1.0),
                )
            });
        }
        x += f.h_advance(id);
        prev = Some(id);
    }
}

/// Dibuja un bloque de texto centrado; devuelve la altura usada.
fn draw_block(
    img: &mut RgbImage,
    font: &FontRef,
    text: &str,
    size: f32,
    top: f32,
    maxw: f32,
    c: [u8; 3],
) -> f32 {
    let scale = PxScale::from(size);
    let lines = wrap(font, scale, text, maxw);
    let lh = size * 1.12;
    let cx = img.width() as f32 / 2.0;
    for (i, l) in lines.iter().enumerate() {
        draw_line(img, font, scale, l, cx, top + size * 0.8 + i as f32 * lh, c);
    }
    lines.len() as f32 * lh
}

/// Portada tipográfica generada.
pub fn design(title: &str, author: &str, style: usize, w: u32, h: u32) -> RgbImage {
    let st = &STYLES[style % STYLES.len()];
    let mut img = RgbImage::new(w, h);
    // fondo con degradado suave y grano
    let mut seed: u32 = 12345;
    for (_, y, p) in img.enumerate_pixels_mut() {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        let n = (seed % 9) as f32 - 4.0;
        let k = 1.0 - (y as f32 / h as f32) * 0.10;
        *p = Rgb(st.bg.map(|c| ((c as f32 * k) + n).clamp(0.0, 255.0) as u8));
    }
    let bold = FontRef::try_from_slice(BOLD).expect("fuente");
    let ital = FontRef::try_from_slice(ITALIC).expect("fuente");
    let wf = w as f32;
    let u = wf / 100.0;
    frame(
        &mut img,
        (u * 4.5) as i32,
        (u * 0.9).max(2.0) as i32,
        st.accent,
    );
    frame(
        &mut img,
        (u * 6.3) as i32,
        (u * 0.25).max(1.0) as i32,
        st.accent,
    );

    // título: reduce el tamaño hasta que quepa en 4 líneas
    let maxw = wf * 0.74;
    let mut size = wf * 0.12;
    while size > wf * 0.05 && wrap(&bold, PxScale::from(size), title, maxw).len() > 4 {
        size *= 0.9;
    }
    let top = h as f32 * 0.24;
    let used = draw_block(&mut img, &bold, title, size, top, maxw, st.fg);

    // ornamento
    let oy = (top + used + u * 6.0) as i32;
    let cx = (wf / 2.0) as i32;
    rect(
        &mut img,
        cx - (u * 14.0) as i32,
        oy,
        cx - (u * 2.5) as i32,
        oy + (u * 0.3).max(1.0) as i32,
        st.accent,
    );
    rect(
        &mut img,
        cx + (u * 2.5) as i32,
        oy,
        cx + (u * 14.0) as i32,
        oy + (u * 0.3).max(1.0) as i32,
        st.accent,
    );
    let r = (u * 1.3) as i32;
    for dy in -r..=r {
        let half = r - dy.abs();
        rect(
            &mut img,
            cx - half,
            oy + dy,
            cx + half + 1,
            oy + dy + 1,
            st.accent,
        );
    }

    if !author.trim().is_empty() {
        let s = wf * 0.055;
        draw_block(&mut img, &ital, author, s, h as f32 * 0.78, maxw, st.fg);
    }
    img
}

/// Ajusta una imagen propia al tamaño indicado (recorte centrado).
pub fn fit_image(img: &DynamicImage, w: u32, h: u32) -> RgbImage {
    img.resize_to_fill(w, h, image::imageops::FilterType::Lanczos3)
        .to_rgb8()
}

pub fn jpeg(img: &RgbImage) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 90);
    enc.encode_image(img).map_err(|e| e.to_string())?;
    Ok(out)
}

fn backup(path: &Path) {
    if let Some(d) = dirs::data_dir().map(|d| d.join("Lector").join("copias")) {
        let _ = std::fs::create_dir_all(&d);
        if let Some(name) = path.file_name() {
            let dst = d.join(name);
            if !dst.exists() {
                if path.is_dir() {
                    // solo se copia el OPF en libros-carpeta (Apple Books)
                } else {
                    let _ = std::fs::copy(path, dst);
                }
            }
        }
    }
}

const COVER_ID: &str = "lector-portada";

fn patch_opf(opf: &str, epub3: bool) -> String {
    // quita marcas de portada anteriores
    let mut s = opf.replace("cover-image", "");
    while let Some(i) = s.find("<meta name=\"cover\"") {
        let end = s[i..].find("/>").map(|e| i + e + 2).unwrap_or(i + 18);
        s.replace_range(i..end, "");
    }
    let props = if epub3 {
        " properties=\"cover-image\""
    } else {
        ""
    };
    let items = format!(
        "<item id=\"{COVER_ID}-img\" href=\"{COVER_ID}.jpg\" media-type=\"image/jpeg\"{props}/>\n<item id=\"{COVER_ID}-pag\" href=\"{COVER_ID}.xhtml\" media-type=\"application/xhtml+xml\"/>\n"
    );
    if let Some(i) = s.find("</manifest>").or_else(|| s.find("</opf:manifest>")) {
        s.insert_str(i, &items);
    }
    if let Some(i) = s.find("</metadata>").or_else(|| s.find("</opf:metadata>")) {
        s.insert_str(
            i,
            &format!("<meta name=\"cover\" content=\"{COVER_ID}-img\"/>\n"),
        );
    }
    if let Some(i) = s.find("<spine").or_else(|| s.find("<opf:spine")) {
        if let Some(e) = s[i..].find('>') {
            let tag = if s[i..].starts_with("<opf:") {
                "opf:itemref"
            } else {
                "itemref"
            };
            s.insert_str(i + e + 1, &format!("\n<{tag} idref=\"{COVER_ID}-pag\"/>"));
        }
    }
    s
}

const COVER_XHTML: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<!DOCTYPE html>
<html xmlns="http://www.w3.org/1999/xhtml"><head><title>Portada</title>
<style>body{margin:0;padding:0;text-align:center}img{max-width:100%;max-height:100vh}</style>
</head><body><div><img src="lector-portada.jpg" alt="Portada"/></div></body></html>"#;

/// Añade (o sustituye) la portada dentro del EPUB.
pub fn attach_epub(path: &Path, jpg: &[u8]) -> Result<(), String> {
    backup(path);
    let dir = |opf: &str| match opf.rfind('/') {
        Some(i) => opf[..i + 1].to_string(),
        None => String::new(),
    };
    if path.is_dir() {
        let mut ar = crate::epub::Archive::open(path)?;
        let opf_path = crate::epub::find_opf_path(&mut ar).ok_or("OPF no encontrado")?;
        let opf = ar.read_string(&opf_path).ok_or("OPF ilegible")?;
        let base = path.join(dir(&opf_path));
        std::fs::write(base.join(format!("{COVER_ID}.jpg")), jpg).map_err(|e| e.to_string())?;
        if !opf.contains(COVER_ID) {
            std::fs::write(base.join(format!("{COVER_ID}.xhtml")), COVER_XHTML)
                .map_err(|e| e.to_string())?;
            let epub3 = opf.contains("version=\"3");
            std::fs::write(path.join(&opf_path), patch_opf(&opf, epub3))
                .map_err(|e| e.to_string())?;
        }
        return Ok(());
    }

    let data = std::fs::read(path).map_err(|e| e.to_string())?;
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(data)).map_err(|e| e.to_string())?;
    let mut ar = crate::epub::Archive::open(path)?;
    let opf_path = crate::epub::find_opf_path(&mut ar).ok_or("OPF no encontrado")?;
    let opf = ar.read_string(&opf_path).ok_or("OPF ilegible")?;
    let already = opf.contains(COVER_ID);
    let base = dir(&opf_path);
    let new_opf = if already {
        opf.clone()
    } else {
        patch_opf(&opf, opf.contains("version=\"3"))
    };

    let mut out = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let stored =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    let deflated = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    out.start_file("mimetype", stored)
        .map_err(|e| e.to_string())?;
    out.write_all(b"application/epub+zip")
        .map_err(|e| e.to_string())?;
    let img_name = format!("{base}{COVER_ID}.jpg");
    let page_name = format!("{base}{COVER_ID}.xhtml");
    for i in 0..z.len() {
        let mut f = z.by_index(i).map_err(|e| e.to_string())?;
        let name = f.name().map_err(|e| e.to_string())?.to_string();
        if name == "mimetype" || name.ends_with('/') || name == img_name || name == page_name {
            continue;
        }
        let mut buf = Vec::new();
        f.read_to_end(&mut buf).map_err(|e| e.to_string())?;
        if name == opf_path {
            buf = new_opf.as_bytes().to_vec();
        }
        out.start_file(name, deflated).map_err(|e| e.to_string())?;
        out.write_all(&buf).map_err(|e| e.to_string())?;
    }
    out.start_file(img_name, stored)
        .map_err(|e| e.to_string())?;
    out.write_all(jpg).map_err(|e| e.to_string())?;
    out.start_file(page_name, deflated)
        .map_err(|e| e.to_string())?;
    out.write_all(COVER_XHTML.as_bytes())
        .map_err(|e| e.to_string())?;
    let bytes = out.finish().map_err(|e| e.to_string())?.into_inner();
    let tmp = path.with_extension("epub.tmp");
    std::fs::write(&tmp, bytes).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())?;
    Ok(())
}

/// Inserta la portada como primera página del PDF.
pub fn attach_pdf(
    path: &Path,
    jpg: &[u8],
    img_w: u32,
    img_h: u32,
    page_w: f32,
    page_h: f32,
) -> Result<(), String> {
    use lopdf::{Object, Stream, dictionary};
    backup(path);
    let mut doc =
        lopdf::Document::load(path).map_err(|e| format!("No se puede editar este PDF: {e}"))?;
    if doc.is_encrypted() {
        return Err("PDF protegido: no se puede modificar".into());
    }
    let pages_id = doc
        .catalog()
        .map_err(|e| e.to_string())?
        .get(b"Pages")
        .and_then(|p| p.as_reference())
        .map_err(|e| e.to_string())?;
    let img_id = doc.add_object(Stream::new(
        dictionary! {
            "Type" => "XObject",
            "Subtype" => "Image",
            "Width" => img_w as i64,
            "Height" => img_h as i64,
            "ColorSpace" => "DeviceRGB",
            "BitsPerComponent" => 8,
            "Filter" => "DCTDecode",
        },
        jpg.to_vec(),
    ));
    let content = format!("q {page_w} 0 0 {page_h} 0 0 cm /PortadaLector Do Q");
    let content_id = doc.add_object(Stream::new(dictionary! {}, content.into_bytes()));
    let page_id = doc.add_object(dictionary! {
        "Type" => "Page",
        "Parent" => pages_id,
        "MediaBox" => vec![0.into(), 0.into(), Object::Real(page_w), Object::Real(page_h)],
        "Contents" => content_id,
        "Resources" => dictionary! { "XObject" => dictionary! { "PortadaLector" => img_id } },
    });
    let pages = doc
        .get_object_mut(pages_id)
        .and_then(|o| o.as_dict_mut())
        .map_err(|e| e.to_string())?;
    let count = pages.get(b"Count").and_then(|c| c.as_i64()).unwrap_or(0);
    pages.set("Count", count + 1);
    pages
        .get_mut(b"Kids")
        .and_then(|k| k.as_array_mut())
        .map_err(|e| e.to_string())?
        .insert(0, page_id.into());
    let tmp = path.with_extension("pdf.tmp");
    doc.save(&tmp).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())?;
    Ok(())
}
