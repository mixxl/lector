//! Lectura de EPUB (2 y 3), tanto empaquetados (.epub zip) como en carpeta
//! (formato en que Apple Books guarda los libros).

use std::collections::HashMap;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};

pub enum Archive {
    Zip(zip::ZipArchive<Cursor<Vec<u8>>>),
    Dir(PathBuf),
    /// recursos en memoria (imágenes de MOBI/AZW3)
    Mem(HashMap<String, Vec<u8>>),
}

impl Archive {
    pub fn open(path: &Path) -> Result<Self, String> {
        if path.is_dir() {
            Ok(Archive::Dir(path.to_path_buf()))
        } else {
            let data = std::fs::read(path).map_err(|e| e.to_string())?;
            let z = zip::ZipArchive::new(Cursor::new(data)).map_err(|e| e.to_string())?;
            Ok(Archive::Zip(z))
        }
    }

    pub fn read(&mut self, name: &str) -> Option<Vec<u8>> {
        let name = name.trim_start_matches('/');
        match self {
            Archive::Mem(m) => m.get(name).cloned(),
            Archive::Dir(base) => std::fs::read(base.join(name)).ok(),
            Archive::Zip(z) => {
                let mut out = Vec::new();
                if let Ok(mut f) = z.by_name(name) {
                    f.read_to_end(&mut out).ok()?;
                    return Some(out);
                }
                // Búsqueda sin distinguir mayúsculas
                let lower = name.to_lowercase();
                let real = z
                    .file_names()
                    .filter_map(|n| n.ok())
                    .find(|n| n.to_lowercase() == lower)?
                    .to_string();
                let mut f = z.by_name(&real).ok()?;
                f.read_to_end(&mut out).ok()?;
                Some(out)
            }
        }
    }

    pub fn find_ext(&mut self, ext: &str) -> Option<String> {
        match self {
            Archive::Mem(_) => None,
            Archive::Zip(z) => z
                .file_names()
                .filter_map(|n| n.ok())
                .find(|n| n.to_lowercase().ends_with(ext))
                .map(|n| n.to_string()),
            Archive::Dir(base) => {
                fn walk(d: &Path, base: &Path, ext: &str, depth: usize) -> Option<String> {
                    if depth > 3 {
                        return None;
                    }
                    for e in std::fs::read_dir(d).ok()?.flatten() {
                        let p = e.path();
                        if p.is_dir() {
                            if let Some(r) = walk(&p, base, ext, depth + 1) {
                                return Some(r);
                            }
                        } else if p.to_string_lossy().to_lowercase().ends_with(ext) {
                            return Some(
                                p.strip_prefix(base)
                                    .ok()?
                                    .to_string_lossy()
                                    .replace('\\', "/"),
                            );
                        }
                    }
                    None
                }
                let b = base.clone();
                walk(&b, &b, ext, 0)
            }
        }
    }

    pub fn read_string(&mut self, name: &str) -> Option<String> {
        let b = self.read(name)?;
        Some(String::from_utf8_lossy(&b).into_owned())
    }
}

#[derive(Clone, Debug)]
pub struct Span {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
    pub small: bool,
    pub scale: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ParaKind {
    Body,
    Heading(u8),
    Quote,
    ListItem,
    Center,
    Pre,
}

impl Default for Span {
    fn default() -> Self {
        Span {
            text: String::new(),
            bold: false,
            italic: false,
            small: false,
            scale: 1.0,
        }
    }
}

#[derive(Clone, Debug)]
pub enum Block {
    Para {
        spans: Vec<Span>,
        kind: ParaKind,
        /// alineación del CSS: 0 sin indicar, 1 izq., 2 centro, 3 der., 4 justif.
        align: u8,
        /// sangría del CSS (em)
        indent: Option<f32>,
        /// tamaño relativo del CSS
        scale: f32,
    },
    Image {
        path: String,
    },
    Rule,
}

pub struct Chapter {
    pub blocks: Vec<Block>,
    pub anchors: HashMap<String, usize>,
}

#[derive(Clone, Debug)]
pub struct TocEntry {
    pub title: String,
    pub chapter: usize,
    pub block: usize,
    pub level: usize,
}

pub struct Epub {
    pub title: String,
    pub author: String,
    pub language: String,
    pub chapters: Vec<Chapter>,
    pub toc: Vec<TocEntry>,
    #[allow(dead_code)]
    pub cover_path: Option<String>,
    archive: Archive,
    image_cache: HashMap<String, Option<Vec<u8>>>,
}

impl Epub {
    pub fn image_bytes(&mut self, path: &str) -> Option<Vec<u8>> {
        if let Some(v) = self.image_cache.get(path) {
            return v.clone();
        }
        let v = self.archive.read(path);
        self.image_cache.insert(path.to_string(), v.clone());
        v
    }

    /// Título del capítulo según el índice (para la cabecera de página).
    pub fn chapter_title(&self, chapter: usize) -> Option<&str> {
        self.toc
            .iter()
            .filter(|t| t.chapter <= chapter)
            .last()
            .map(|t| t.title.as_str())
    }
}

/// Lectura rápida de metadatos y portada (para la biblioteca).
pub struct Info {
    pub title: String,
    pub author: String,
    pub subjects: Vec<String>,
}

/// Normaliza un tema (dc:subject): descarta códigos BISAC/Thema y basura.
fn clean_subject(s: &str) -> Option<String> {
    let t = s
        .trim()
        .trim_matches(|c: char| c == '.' || c == '-' || c == '"');
    if t.chars().count() < 3 || t.chars().count() > 40 {
        return None;
    }
    let digits = t.chars().filter(|c| c.is_ascii_digit()).count();
    if digits * 2 > t.len() || (t.len() <= 12 && digits >= 3) {
        return None;
    }
    let mut c = t.chars();
    let first = c.next()?;
    let rest: String = c.collect();
    let rest = if rest.chars().all(|c| !c.is_lowercase()) {
        rest.to_lowercase()
    } else {
        rest
    };
    Some(first.to_uppercase().collect::<String>() + &rest)
}

pub fn quick_info(path: &Path) -> Option<(Info, Option<Vec<u8>>)> {
    if is_mobi(path) {
        let m = mobi::Mobi::from_path(path).ok()?;
        let imgs = mobi_images(&m);
        let mut keys: Vec<(usize, &String)> = imgs
            .keys()
            .filter_map(|k| {
                k.strip_prefix("recindex:")
                    .and_then(|n| n.parse().ok())
                    .map(|n| (n, k))
            })
            .collect();
        keys.sort();
        let cover = keys
            .iter()
            .map(|(_, k)| imgs[*k].clone())
            .find(|b| image::guess_format(b).is_ok());
        let title = m.title();
        return Some((
            Info {
                title,
                author: m.author().unwrap_or_default(),
                subjects: vec![],
            },
            cover,
        ));
    }
    let mut ar = Archive::open(path).ok()?;
    let opf_path = find_opf(&mut ar)?;
    let opf = ar.read_string(&opf_path)?;
    let pkg = parse_opf(&opf, &opf_path)?;
    let decodes = |b: &Vec<u8>| image::guess_format(b).is_ok();
    let mut cover = pkg.cover.as_ref().and_then(|c| ar.read(c)).filter(decodes);
    if cover.is_none() {
        for page in &pkg.cover_pages {
            let Some(html) = ar.read_string(page) else {
                continue;
            };
            let base = dir_of(page);
            let img = tokenize(&html).into_iter().find_map(|t| match t {
                Tok::Open { name, attrs, .. } if name == "img" || name == "image" => {
                    attr(&attrs, "src").or_else(|| attr(&attrs, "href"))
                }
                _ => None,
            });
            if let Some(src) = img {
                if let Some(b) = ar
                    .read(&join_path(&base, &percent_decode(&src)))
                    .filter(decodes)
                {
                    cover = Some(b);
                    break;
                }
            }
        }
    }
    Some((
        Info {
            title: pkg.title,
            author: pkg.author,
            subjects: pkg.subjects,
        },
        cover,
    ))
}

fn find_opf(ar: &mut Archive) -> Option<String> {
    let container = ar.read_string("META-INF/container.xml");
    if let Some(container) = container {
        let fixed = sanitize_xml(&container);
        if let Some(doc) = parse_xml(&fixed) {
            if let Some(p) = doc
                .descendants()
                .find(|n| n.tag_name().name() == "rootfile")
                .and_then(|rf| rf.attribute("full-path"))
            {
                return Some(percent_decode(p));
            }
        }
    }
    // sin container.xml válido: busca cualquier .opf
    ar.find_ext(".opf")
}

fn parse_xml(s: &str) -> Option<roxmltree::Document<'_>> {
    let opts = roxmltree::ParsingOptions {
        allow_dtd: true,
        ..Default::default()
    };
    roxmltree::Document::parse_with_options(s, opts).ok()
}

/// Arregla XML "casi válido": entidades HTML (&nbsp;…), BOM, basura inicial.
fn sanitize_xml(s: &str) -> String {
    let s = s.trim_start_matches('\u{feff}');
    let s = match s.find('<') {
        Some(i) => &s[i..],
        None => s,
    };
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(p) = rest.find('&') {
        out.push_str(&rest[..p]);
        rest = &rest[p..];
        match rest[..rest.len().min(12)].find(';') {
            Some(e) => {
                let ent = &rest[1..e];
                if matches!(ent, "amp" | "lt" | "gt" | "quot" | "apos") || ent.starts_with('#') {
                    out.push_str(&rest[..=e]);
                } else if let Some(c) = named_entity(ent) {
                    out.push_str(&format!("&#{};", c as u32));
                } else {
                    out.push_str("&amp;");
                    out.push_str(&rest[1..=e]);
                }
                rest = &rest[e + 1..];
            }
            None => {
                out.push_str("&amp;");
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

struct Package {
    title: String,
    author: String,
    spine: Vec<String>,
    subjects: Vec<String>,
    language: String,
    cover: Option<String>,
    cover_pages: Vec<String>,
    nav: Option<String>,
    ncx: Option<String>,
}

fn parse_opf(opf: &str, opf_path: &str) -> Option<Package> {
    if let Some(p) = parse_opf_doc(opf, opf_path) {
        return Some(p);
    }
    parse_opf_doc(&sanitize_xml(opf), opf_path)
}

fn parse_opf_doc(opf: &str, opf_path: &str) -> Option<Package> {
    let doc = parse_xml(opf)?;
    let mut guide_cover: Option<String> = None;
    let base = dir_of(opf_path);
    let mut title = String::new();
    let mut author = String::new();
    let mut subjects: Vec<String> = Vec::new();
    let mut language = String::new();
    let mut cover_id = None;
    let mut manifest: HashMap<String, (String, String, String)> = HashMap::new();
    let mut spine_ids = Vec::new();
    let mut toc_id = None;
    for n in doc.descendants().filter(|n| n.is_element()) {
        match n.tag_name().name() {
            "title" if title.is_empty() => title = n.text().unwrap_or("").trim().to_string(),
            "language" if language.is_empty() => {
                language = n.text().unwrap_or("").trim().to_string()
            }
            "creator" if author.is_empty() => author = n.text().unwrap_or("").trim().to_string(),
            "subject" => {
                for part in n.text().unwrap_or("").split([',', ';', '/', '|']) {
                    if let Some(t) = clean_subject(part) {
                        if !subjects.contains(&t) {
                            subjects.push(t);
                        }
                    }
                }
            }
            "meta" if n.attribute("name") == Some("cover") => {
                cover_id = n.attribute("content").map(|s| s.to_string())
            }
            "item" => {
                if let (Some(id), Some(href)) = (n.attribute("id"), n.attribute("href")) {
                    manifest.insert(
                        id.to_string(),
                        (
                            join_path(&base, &percent_decode(href)),
                            n.attribute("media-type").unwrap_or("").to_string(),
                            n.attribute("properties").unwrap_or("").to_string(),
                        ),
                    );
                }
            }
            "spine" => toc_id = n.attribute("toc").map(|s| s.to_string()),
            "reference" => {
                let t = n.attribute("type").unwrap_or("").to_lowercase();
                if guide_cover.is_none()
                    && (t == "cover" || t == "title-page" && guide_cover.is_none())
                {
                    if let Some(h) = n.attribute("href") {
                        guide_cover = Some(join_path(
                            &base,
                            &percent_decode(h.split('#').next().unwrap_or(h)),
                        ));
                    }
                }
            }
            "itemref" => {
                if let Some(id) = n.attribute("idref") {
                    spine_ids.push(id.to_string())
                }
            }
            _ => {}
        }
    }
    let spine: Vec<String> = spine_ids
        .iter()
        .filter_map(|id| manifest.get(id).map(|m| m.0.clone()))
        .collect();
    let mut cover = manifest
        .values()
        .find(|m| m.2.split_whitespace().any(|p| p == "cover-image"))
        .map(|m| m.0.clone());
    if cover.is_none() {
        cover = cover_id
            .and_then(|id| manifest.get(&id))
            .map(|m| m.0.clone());
    }
    if cover.is_none() {
        cover = manifest
            .iter()
            .find(|(id, m)| {
                (id.to_lowercase().contains("cover")
                    || m.0.to_lowercase().contains("cover")
                    || m.0.to_lowercase().contains("portada"))
                    && m.1.starts_with("image/")
            })
            .map(|(_, m)| m.0.clone());
    }
    // páginas HTML donde buscar la imagen de portada si no hay otra pista
    let mut cover_pages: Vec<String> = Vec::new();
    if let Some(g) = guide_cover {
        cover_pages.push(g);
    }
    cover_pages.extend(
        manifest
            .values()
            .filter(|m| m.1.contains("html") && m.0.to_lowercase().contains("cover"))
            .map(|m| m.0.clone()),
    );
    let nav = manifest
        .values()
        .find(|m| m.2.split_whitespace().any(|p| p == "nav"))
        .map(|m| m.0.clone());
    let ncx = toc_id
        .and_then(|id| manifest.get(&id).map(|m| m.0.clone()))
        .or_else(|| {
            manifest
                .values()
                .find(|m| m.1 == "application/x-dtbncx+xml")
                .map(|m| m.0.clone())
        });
    cover_pages.extend(spine.iter().take(2).cloned());
    Some(Package {
        title,
        author,
        subjects,
        language,
        spine,
        cover,
        cover_pages,
        nav,
        ncx,
    })
}

fn has_drm(ar: &mut Archive) -> bool {
    if ar.read("META-INF/sinf.xml").is_some() || ar.read("META-INF/rights.xml").is_some() {
        return true;
    }
    if let Some(enc) = ar.read_string("META-INF/encryption.xml") {
        // La ofuscación de fuentes (IDPF/Adobe) no es DRM
        let total = enc.matches("EncryptionMethod").count();
        let fonts = enc.matches("http://www.idpf.org/2008/embedding").count()
            + enc.matches("http://ns.adobe.com/pdf/enc#RC").count();
        return total > fonts;
    }
    false
}

fn is_mobi(path: &Path) -> bool {
    path.extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .is_some_and(|e| matches!(e.as_str(), "mobi" | "azw" | "azw3" | "prc"))
}

fn mobi_lang(m: &mobi::Mobi) -> String {
    let l = format!("{:?}", m.language()).to_lowercase();
    match l.as_str() {
        x if x.starts_with("spanish") => "es",
        x if x.starts_with("english") => "en",
        x if x.starts_with("french") => "fr",
        x if x.starts_with("german") => "de",
        x if x.starts_with("italian") => "it",
        x if x.starts_with("portuguese") => "pt",
        x if x.starts_with("catalan") => "ca",
        _ => "es",
    }
    .to_string()
}

fn mobi_images(m: &mobi::Mobi) -> HashMap<String, Vec<u8>> {
    let first = m.metadata.mobi.first_image_index as usize;
    let recs = m.raw_records();
    let mut out = HashMap::new();
    for (i, r) in recs.range(first..).iter().enumerate() {
        out.insert(format!("recindex:{}", i + 1), r.content.to_vec());
    }
    out
}

/// Libros de Kindle sin DRM (MOBI, AZW, AZW3).
pub fn open_mobi(path: &Path) -> Result<Epub, String> {
    let m = mobi::Mobi::from_path(path).map_err(|e| format!("MOBI no válido: {e:?}"))?;
    if !matches!(m.encryption(), mobi::headers::Encryption::No) {
        return Err("el libro está protegido con DRM".into());
    }
    let raw = std::fs::read(path).map_err(|e| e.to_string())?;
    let html = mobi_text(&raw).unwrap_or_else(|| m.content_as_string_lossy());
    let css = Css {
        rules: HashMap::new(),
        own_align_only: true,
    };
    let mut chapters = Vec::new();
    let parts: Vec<&str> = html.split("<mbp:pagebreak").collect();
    for part in parts {
        let part = match part.find('>') {
            Some(i) if part.starts_with(' ') || part.starts_with('/') => &part[i + 1..],
            _ => part,
        };
        let ch = html_to_blocks(&format!("<body>{part}</body>"), "", &css);
        if !ch.blocks.is_empty() {
            chapters.push(ch);
        }
    }
    if chapters.is_empty() {
        return Err("No se pudo leer el contenido (¿AZW3 sin texto compatible?)".into());
    }
    let toc = toc_from_headings(&chapters);
    let title = m.title();
    let title = if title.trim().is_empty() {
        path.file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default()
    } else {
        title
    };
    Ok(Epub {
        title,
        author: m.author().unwrap_or_default(),
        language: mobi_lang(&m),
        chapters,
        toc,
        cover_path: Some("recindex:1".into()),
        archive: Archive::Mem(mobi_images(&m)),
        image_cache: HashMap::new(),
    })
}

pub fn open(path: &Path) -> Result<Epub, String> {
    if is_mobi(path) {
        return open_mobi(path);
    }
    let mut ar = Archive::open(path)?;
    if has_drm(&mut ar) {
        return Err("el libro está protegido con DRM".into());
    }
    let opf_path = find_opf(&mut ar).ok_or("No se encuentra el paquete OPF")?;
    let opf = ar.read_string(&opf_path).ok_or("OPF ilegible")?;
    let pkg = parse_opf(&opf, &opf_path).ok_or("OPF inválido")?;

    let mut chapters = Vec::new();
    let mut chapter_index: HashMap<String, usize> = HashMap::new();
    let mut css_cache: HashMap<String, String> = HashMap::new();
    for href in &pkg.spine {
        let Some(html) = ar.read_string(href) else {
            continue;
        };
        let base = dir_of(href);
        let (links, inline) = stylesheet_refs(&html, &base);
        let mut css = Css::default();
        for l in links {
            let c = css_cache
                .entry(l.clone())
                .or_insert_with(|| ar.read_string(&l).unwrap_or_default())
                .clone();
            css.parse(&c);
        }
        css.parse(&inline);
        let ch = html_to_blocks(&html, &base, &css);
        chapter_index.insert(href.clone(), chapters.len());
        chapters.push(ch);
    }
    if chapters.is_empty() {
        return Err("El libro no tiene contenido legible".into());
    }

    let mut raw_toc: Vec<(String, String, usize)> = Vec::new();
    if let Some(nav) = &pkg.nav {
        if let Some(s) = ar.read_string(nav) {
            raw_toc = parse_nav(&s, &dir_of(nav));
        }
    }
    if raw_toc.is_empty() {
        if let Some(ncx) = &pkg.ncx {
            if let Some(s) = ar.read_string(ncx) {
                raw_toc = parse_ncx(&s, &dir_of(ncx));
            }
        }
    }
    let mut toc = Vec::new();
    for (title, target, level) in raw_toc {
        let (file, frag) = match target.split_once('#') {
            Some((f, a)) => (f.to_string(), Some(a.to_string())),
            None => (target.clone(), None),
        };
        if let Some(&ci) = chapter_index.get(&file) {
            let block = frag
                .and_then(|f| chapters[ci].anchors.get(&f).copied())
                .unwrap_or(0);
            if !title.trim().is_empty() {
                toc.push(TocEntry {
                    title: title.trim().to_string(),
                    chapter: ci,
                    block,
                    level,
                });
            }
        }
    }

    let title = if pkg.title.is_empty() {
        path.file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default()
    } else {
        pkg.title
    };
    if toc.is_empty() {
        toc = toc_from_headings(&chapters);
    }
    Ok(Epub {
        title,
        author: pkg.author,
        language: pkg.language,
        chapters,
        toc,
        cover_path: pkg.cover,
        archive: ar,
        image_cache: HashMap::new(),
    })
}

fn parse_ncx(s: &str, base: &str) -> Vec<(String, String, usize)> {
    let Some(doc) = parse_xml(s) else {
        return vec![];
    };
    let mut out = Vec::new();
    for np in doc
        .descendants()
        .filter(|n| n.tag_name().name() == "navPoint")
    {
        let level = np
            .ancestors()
            .filter(|a| a.tag_name().name() == "navPoint")
            .count()
            - 1;
        let label = np
            .children()
            .find(|c| c.tag_name().name() == "navLabel")
            .map(|l| {
                l.descendants()
                    .filter(|d| d.is_text())
                    .map(|d| d.text().unwrap_or(""))
                    .collect::<String>()
            })
            .unwrap_or_default();
        let src = np
            .children()
            .find(|c| c.tag_name().name() == "content")
            .and_then(|c| c.attribute("src"))
            .unwrap_or("");
        out.push((
            collapse_ws(&label),
            join_path(base, &percent_decode(src)),
            level,
        ));
    }
    out
}

fn parse_nav(s: &str, base: &str) -> Vec<(String, String, usize)> {
    let toks = tokenize(s);
    let mut out = Vec::new();
    let mut in_toc = false;
    let mut nav_depth = 0;
    let mut ol_depth = 0usize;
    let mut cur: Option<(String, String)> = None;
    for t in toks {
        match t {
            Tok::Open {
                name,
                attrs,
                self_closing,
            } => match name.as_str() {
                "nav" => {
                    if in_toc {
                        nav_depth += 1;
                    } else if attrs
                        .iter()
                        .any(|(k, v)| k.ends_with("type") && v.contains("toc"))
                    {
                        in_toc = true;
                        nav_depth = 1;
                    }
                }
                "ol" | "ul" if in_toc => ol_depth += 1,
                "a" if in_toc && !self_closing => {
                    let href = attr(&attrs, "href").unwrap_or_default();
                    cur = Some((String::new(), join_path(base, &percent_decode(&href))));
                }
                _ => {}
            },
            Tok::Close(name) => match name.as_str() {
                "nav" if in_toc => {
                    nav_depth -= 1;
                    if nav_depth == 0 {
                        in_toc = false;
                    }
                }
                "ol" | "ul" if in_toc => ol_depth = ol_depth.saturating_sub(1),
                "a" => {
                    if let Some((t, h)) = cur.take() {
                        out.push((collapse_ws(&t), h, ol_depth.saturating_sub(1)));
                    }
                }
                _ => {}
            },
            Tok::Text(t) => {
                if let Some((s, _)) = cur.as_mut() {
                    s.push_str(&t);
                }
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Tokenizador HTML tolerante

enum Tok {
    Open {
        name: String,
        attrs: Vec<(String, String)>,
        self_closing: bool,
    },
    Close(String),
    Text(String),
}

fn attr(attrs: &[(String, String)], key: &str) -> Option<String> {
    attrs
        .iter()
        .find(|(k, _)| k == key || k.rsplit(':').next() == Some(key))
        .map(|(_, v)| v.clone())
}

fn tokenize(s: &str) -> Vec<Tok> {
    let b = s.as_bytes();
    let mut i = 0;
    let mut out = Vec::new();
    let mut text_start = 0;
    while i < b.len() {
        if b[i] != b'<' {
            i += 1;
            continue;
        }
        if i > text_start {
            out.push(Tok::Text(decode_entities(&s[text_start..i])));
        }
        if s[i..].starts_with("<!--") {
            i = s[i..].find("-->").map(|p| i + p + 3).unwrap_or(b.len());
        } else if s[i..].starts_with("<![CDATA[") {
            let end = s[i..].find("]]>").map(|p| i + p).unwrap_or(b.len());
            out.push(Tok::Text(s[i + 9..end].to_string()));
            i = (end + 3).min(b.len());
        } else if s[i..].starts_with("<!") || s[i..].starts_with("<?") {
            i = s[i..].find('>').map(|p| i + p + 1).unwrap_or(b.len());
        } else {
            // etiqueta
            let mut j = i + 1;
            let closing = j < b.len() && b[j] == b'/';
            if closing {
                j += 1;
            }
            let ns = j;
            while j < b.len() && !b[j].is_ascii_whitespace() && b[j] != b'>' && b[j] != b'/' {
                j += 1;
            }
            let raw = s[ns..j].to_lowercase();
            let name = raw.rsplit(':').next().unwrap_or("").to_string();
            let mut attrs = Vec::new();
            // atributos
            loop {
                while j < b.len() && b[j].is_ascii_whitespace() {
                    j += 1;
                }
                if j >= b.len() || b[j] == b'>' || b[j] == b'/' {
                    break;
                }
                let ks = j;
                while j < b.len()
                    && !b[j].is_ascii_whitespace()
                    && b[j] != b'='
                    && b[j] != b'>'
                    && b[j] != b'/'
                {
                    j += 1;
                }
                let key = s[ks..j].to_lowercase();
                while j < b.len() && b[j].is_ascii_whitespace() {
                    j += 1;
                }
                let mut val = String::new();
                if j < b.len() && b[j] == b'=' {
                    j += 1;
                    while j < b.len() && b[j].is_ascii_whitespace() {
                        j += 1;
                    }
                    if j < b.len() && (b[j] == b'"' || b[j] == b'\'') {
                        let q = b[j];
                        let vs = j + 1;
                        j = vs;
                        while j < b.len() && b[j] != q {
                            j += 1;
                        }
                        val = decode_entities(&s[vs..j.min(b.len())]);
                        j += 1;
                    } else {
                        let vs = j;
                        while j < b.len() && !b[j].is_ascii_whitespace() && b[j] != b'>' {
                            j += 1;
                        }
                        val = s[vs..j].to_string();
                    }
                }
                if key.is_empty() {
                    j += 1;
                } else {
                    attrs.push((key, val));
                }
            }
            let mut self_closing = false;
            while j < b.len() && b[j] != b'>' {
                if b[j] == b'/' {
                    self_closing = true;
                }
                j += 1;
            }
            i = (j + 1).min(b.len());
            if closing {
                out.push(Tok::Close(name));
            } else {
                let raw_text = matches!(name.as_str(), "script" | "style");
                out.push(Tok::Open {
                    name: name.clone(),
                    attrs,
                    self_closing,
                });
                if raw_text && !self_closing {
                    let end_tag = format!("</{name}");
                    let lower = s[i..].to_lowercase();
                    let end = lower.find(&end_tag).map(|p| i + p).unwrap_or(b.len());
                    i = end;
                }
            }
        }
        text_start = i;
    }
    if text_start < b.len() {
        out.push(Tok::Text(decode_entities(&s[text_start..])));
    }
    out
}

fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(p) = rest.find('&') {
        out.push_str(&rest[..p]);
        rest = &rest[p..];
        if let Some(end) = rest[..rest.len().min(12)].find(';') {
            let ent = &rest[1..end];
            let ch: Option<char> =
                if let Some(n) = ent.strip_prefix("#x").or(ent.strip_prefix("#X")) {
                    u32::from_str_radix(n, 16).ok().and_then(char::from_u32)
                } else if let Some(n) = ent.strip_prefix('#') {
                    n.parse().ok().and_then(char::from_u32)
                } else {
                    named_entity(ent)
                };
            if let Some(c) = ch {
                out.push(c);
                rest = &rest[end + 1..];
                continue;
            }
        }
        out.push('&');
        rest = &rest[1..];
    }
    out.push_str(rest);
    out
}

fn named_entity(e: &str) -> Option<char> {
    Some(match e {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        "nbsp" => '\u{a0}',
        "shy" => '\u{ad}',
        "mdash" => '—',
        "ndash" => '–',
        "hellip" => '…',
        "laquo" => '«',
        "raquo" => '»',
        "ldquo" => '“',
        "rdquo" => '”',
        "lsquo" => '‘',
        "rsquo" => '’',
        "bdquo" => '„',
        "iexcl" => '¡',
        "iquest" => '¿',
        "copy" => '©',
        "reg" => '®',
        "deg" => '°',
        "middot" => '·',
        "bull" => '•',
        "sect" => '§',
        "para" => '¶',
        "ordf" => 'ª',
        "ordm" => 'º',
        "times" => '×',
        "euro" => '€',
        "aacute" => 'á',
        "eacute" => 'é',
        "iacute" => 'í',
        "oacute" => 'ó',
        "uacute" => 'ú',
        "Aacute" => 'Á',
        "Eacute" => 'É',
        "Iacute" => 'Í',
        "Oacute" => 'Ó',
        "Uacute" => 'Ú',
        "ntilde" => 'ñ',
        "Ntilde" => 'Ñ',
        "uuml" => 'ü',
        "Uuml" => 'Ü',
        "ccedil" => 'ç',
        "Ccedil" => 'Ç',
        "agrave" => 'à',
        "egrave" => 'è',
        "ograve" => 'ò',
        "auml" => 'ä',
        "ouml" => 'ö',
        "szlig" => 'ß',
        "thinsp" | "ensp" | "emsp" => ' ',
        "zwj" | "zwnj" => '\u{200b}',
        _ => return None,
    })
}

fn collapse_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() + 0 && i + 2 <= b.len() - 1 {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn dir_of(p: &str) -> String {
    match p.rfind('/') {
        Some(i) => p[..i + 1].to_string(),
        None => String::new(),
    }
}

fn join_path(base: &str, rel: &str) -> String {
    if rel.is_empty() {
        return rel.to_string();
    }
    let (rel, frag) = match rel.split_once('#') {
        Some((a, b)) => (a, Some(b)),
        None => (rel, None),
    };
    let mut parts: Vec<&str> = if rel.starts_with('/') {
        vec![]
    } else {
        base.split('/').filter(|s| !s.is_empty()).collect()
    };
    for seg in rel.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            s => parts.push(s),
        }
    }
    let mut r = parts.join("/");
    if rel.is_empty() {
        r = base.trim_end_matches('/').to_string();
    }
    match frag {
        Some(f) => format!("{r}#{f}"),
        None => r,
    }
}

// ---------------------------------------------------------------------------
// CSS básico ("estilo del editor")

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Props {
    /// 1 izquierda, 2 centro, 3 derecha, 4 justificado
    pub align: Option<u8>,
    pub italic: Option<bool>,
    pub bold: Option<bool>,
    pub scale: Option<f32>,
    pub indent: Option<f32>,
}

impl Props {
    fn merge(&mut self, o: &Props) {
        if o.align.is_some() {
            self.align = o.align;
        }
        if o.italic.is_some() {
            self.italic = o.italic;
        }
        if o.bold.is_some() {
            self.bold = o.bold;
        }
        if o.scale.is_some() {
            self.scale = o.scale;
        }
        if o.indent.is_some() {
            self.indent = o.indent;
        }
    }
}

#[derive(Default, Clone)]
pub struct Css {
    rules: HashMap<String, Props>,
    /// HTML descuidado (MOBI): la alineación solo vale para el propio párrafo
    pub own_align_only: bool,
}

fn css_len_em(v: &str) -> Option<f32> {
    let v = v.trim();
    if v == "0" {
        return Some(0.0);
    }
    let num = |s: &str| s.trim().parse::<f32>().ok();
    if let Some(n) = v.strip_suffix("em").or(v.strip_suffix("rem")) {
        num(n)
    } else if let Some(n) = v.strip_suffix('%') {
        num(n).map(|x| x / 100.0)
    } else if let Some(n) = v.strip_suffix("px") {
        num(n).map(|x| x / 16.0)
    } else if let Some(n) = v.strip_suffix("pt") {
        num(n).map(|x| x / 12.0)
    } else {
        num(v)
    }
}

pub fn parse_props(decls: &str) -> Props {
    let mut p = Props::default();
    for d in decls.split(';') {
        let Some((k, v)) = d.split_once(':') else {
            continue;
        };
        let k = k.trim().to_lowercase();
        let v = v
            .trim()
            .trim_end_matches("!important")
            .trim()
            .to_lowercase();
        match k.as_str() {
            "text-align" => {
                p.align = match v.as_str() {
                    "left" | "start" => Some(1),
                    "center" => Some(2),
                    "right" | "end" => Some(3),
                    "justify" => Some(4),
                    _ => None,
                }
            }
            "font-style" => p.italic = Some(v.contains("italic") || v.contains("oblique")),
            "font-weight" => {
                p.bold =
                    Some(v == "bold" || v == "bolder" || v.parse::<u32>().is_ok_and(|w| w >= 600))
            }
            "font-size" => {
                p.scale = match v.as_str() {
                    "xx-small" => Some(0.6),
                    "x-small" => Some(0.75),
                    "small" | "smaller" => Some(0.85),
                    "medium" => Some(1.0),
                    "large" | "larger" => Some(1.15),
                    "x-large" => Some(1.4),
                    "xx-large" => Some(1.7),
                    _ => css_len_em(&v).filter(|x| *x > 0.3 && *x < 4.0),
                }
            }
            "text-indent" => p.indent = css_len_em(&v).map(|x| x.clamp(-3.0, 6.0)),
            "font" => {
                if v.contains("italic") {
                    p.italic = Some(true);
                }
                if v.contains("bold") {
                    p.bold = Some(true);
                }
            }
            _ => {}
        }
    }
    p
}

impl Css {
    pub fn parse(&mut self, src: &str) {
        // sin comentarios ni @media
        let mut s = String::with_capacity(src.len());
        let mut rest = src;
        while let Some(i) = rest.find("/*") {
            s.push_str(&rest[..i]);
            rest = match rest[i..].find("*/") {
                Some(j) => &rest[i + j + 2..],
                None => "",
            };
        }
        s.push_str(rest);
        for rule in s.split('}') {
            let Some((sel, body)) = rule.split_once('{') else {
                continue;
            };
            let sel = sel.trim();
            if sel.starts_with('@') || sel.contains('{') {
                continue;
            }
            let props = parse_props(body);
            if props == Props::default() {
                continue;
            }
            for one in sel.split(',') {
                // solo el último selector simple (p.x, .x, h1…)
                let last = one
                    .rsplit([' ', '>', '+', '~'])
                    .next()
                    .unwrap_or("")
                    .split(':')
                    .next()
                    .unwrap_or("")
                    .trim()
                    .to_lowercase();
                if last.is_empty() || last.contains('[') || last.contains('#') || last == "*" {
                    continue;
                }
                self.rules.entry(last).or_default().merge(&props);
            }
        }
    }

    fn lookup(&self, tag: &str, classes: &[&str]) -> Props {
        let mut p = Props::default();
        if let Some(r) = self.rules.get(tag) {
            p.merge(r);
        }
        for c in classes {
            if let Some(r) = self.rules.get(&format!(".{c}")) {
                p.merge(r);
            }
            if let Some(r) = self.rules.get(&format!("{tag}.{c}")) {
                p.merge(r);
            }
        }
        p
    }
}

/// Hojas de estilo enlazadas y <style> de un documento.
pub fn stylesheet_refs(html: &str, base: &str) -> (Vec<String>, String) {
    let mut links = Vec::new();
    let mut inline = String::new();
    let mut in_style = false;
    for t in tokenize(html) {
        match t {
            Tok::Open { name, attrs, .. } if name == "link" => {
                let rel = attr(&attrs, "rel").unwrap_or_default().to_lowercase();
                if rel.contains("stylesheet") {
                    if let Some(h) = attr(&attrs, "href") {
                        links.push(join_path(base, &percent_decode(&h)));
                    }
                }
            }
            Tok::Open { name, .. } if name == "style" => in_style = true,
            Tok::Close(name) if name == "style" => in_style = false,
            Tok::Text(t) if in_style => inline.push_str(&t),
            _ => {}
        }
    }
    (links, inline)
}

// ---------------------------------------------------------------------------
// HTML -> bloques

struct Builder {
    blocks: Vec<Block>,
    anchors: HashMap<String, usize>,
    pending_anchors: Vec<String>,
    spans: Vec<Span>,
    kind: ParaKind,
    align: u8,
    indent: Option<f32>,
    scale: f32,
}

impl Builder {
    fn flush(&mut self) {
        let pre = self.kind == ParaKind::Pre;
        let mut spans: Vec<Span> = Vec::new();
        let mut last_space = true;
        for s in self.spans.drain(..) {
            let mut t = String::with_capacity(s.text.len());
            for c in s.text.chars() {
                if c == '\u{2028}' {
                    while t.ends_with(' ') {
                        t.pop();
                    }
                    t.push('\n');
                    last_space = true;
                } else if !pre && c.is_whitespace() && c != '\u{a0}' {
                    if !last_space {
                        t.push(' ');
                        last_space = true;
                    }
                } else if c == '\u{ad}' || c == '\u{200b}' {
                    // guiones blandos: los decide el maquetador
                } else {
                    t.push(c);
                    last_space = false;
                }
            }
            if !t.is_empty() {
                spans.push(Span { text: t, ..s });
            }
        }
        while let Some(last) = spans.last_mut() {
            let trimmed = last.text.trim_end().to_string();
            if trimmed.is_empty() {
                spans.pop();
            } else {
                last.text = trimmed;
                break;
            }
        }
        while let Some(first) = spans.first_mut() {
            let t = first.text.trim_start().to_string();
            if t.is_empty() {
                spans.remove(0);
            } else {
                first.text = t;
                break;
            }
        }
        if spans.iter().any(|s| !s.text.trim().is_empty()) {
            let b = Block::Para {
                spans,
                kind: self.kind,
                align: self.align,
                indent: self.indent,
                scale: self.scale,
            };
            self.push(b);
        }
    }
    fn push(&mut self, b: Block) {
        let idx = self.blocks.len();
        for a in self.pending_anchors.drain(..) {
            self.anchors.insert(a, idx);
        }
        self.blocks.push(b);
    }
}

struct Frame {
    name: String,
    props: Props,
    block: bool,
}

const VOID: [&str; 12] = [
    "br", "img", "hr", "meta", "link", "input", "col", "area", "base", "source", "wbr", "image",
];

pub fn html_to_blocks(html: &str, base: &str, css: &Css) -> Chapter {
    let toks = tokenize(html);
    let mut bld = Builder {
        blocks: vec![],
        anchors: HashMap::new(),
        pending_anchors: vec![],
        spans: vec![],
        kind: ParaKind::Body,
        align: 0,
        indent: None,
        scale: 1.0,
    };
    let mut stack: Vec<Frame> = Vec::new();
    let mut skip = 0i32;
    let mut in_body = !html.contains("<body");

    let is_block = |n: &str| {
        matches!(
            n,
            "p" | "div"
                | "h1"
                | "h2"
                | "h3"
                | "h4"
                | "h5"
                | "h6"
                | "blockquote"
                | "li"
                | "section"
                | "article"
                | "figure"
                | "figcaption"
                | "pre"
                | "tr"
                | "dt"
                | "dd"
                | "table"
                | "ul"
                | "ol"
                | "header"
                | "footer"
                | "aside"
                | "center"
                | "dl"
                | "address"
                | "body"
        )
    };
    let tag_props = |n: &str| -> Props {
        let mut p = Props::default();
        match n {
            "b" | "strong" | "th" => p.bold = Some(true),
            "i" | "em" | "cite" | "var" | "dfn" => p.italic = Some(true),
            "small" | "sup" | "sub" => p.scale = Some(0.8),
            "big" => p.scale = Some(1.2),
            "center" => p.align = Some(2),
            _ => {}
        }
        p
    };

    // Estado actual calculado a partir de la pila
    let own_only = css.own_align_only;
    let current = |stack: &Vec<Frame>| -> (bool, bool, f32, u8, Option<f32>, ParaKind) {
        let mut italic = false;
        let mut bold = false;
        let mut scale = 1.0f32;
        let mut align = 0u8;
        let mut indent = None;
        let mut kind = ParaKind::Body;
        let mut in_li = false;
        let mut quote = false;
        for f in stack {
            if let Some(i) = f.props.italic {
                italic = i;
            }
            if let Some(b) = f.props.bold {
                bold = b;
            }
            if let Some(s) = f.props.scale {
                if !f.block || f.name.starts_with('h') {
                    scale *= s;
                }
            }
            if f.block {
                if let Some(a) = f.props.align {
                    align = a;
                } else if own_only {
                    align = 0;
                }
                if let Some(i) = f.props.indent {
                    indent = Some(i);
                }
            }
            match f.name.as_str() {
                "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                    kind = ParaKind::Heading(f.name[1..].parse().unwrap_or(2))
                }
                "pre" => kind = ParaKind::Pre,
                "blockquote" => quote = true,
                "li" => in_li = true,
                _ => {}
            }
        }
        if matches!(kind, ParaKind::Body) {
            if quote {
                kind = ParaKind::Quote;
            } else if in_li {
                kind = ParaKind::ListItem;
            }
        }
        if matches!(kind, ParaKind::Body | ParaKind::Quote) && align == 2 {
            kind = ParaKind::Center;
        }
        (italic, bold, scale, align, indent, kind)
    };

    for t in toks {
        match t {
            Tok::Open {
                name,
                attrs,
                self_closing,
            } => {
                if name == "body" {
                    in_body = true;
                }
                if matches!(name.as_str(), "head" | "script" | "style" | "title") && !self_closing {
                    skip += 1;
                    continue;
                }
                if let Some(id) = attr(&attrs, "id") {
                    bld.pending_anchors.push(id);
                }
                let classes_s = attr(&attrs, "class").unwrap_or_default().to_lowercase();
                let classes: Vec<&str> = classes_s.split_whitespace().collect();
                let mut props = tag_props(&name);
                props.merge(&css.lookup(&name, &classes));
                if let Some(st) = attr(&attrs, "style") {
                    props.merge(&parse_props(&st));
                }
                if attr(&attrs, "align").is_some_and(|a| a.eq_ignore_ascii_case("center")) {
                    props.align = Some(2);
                }
                // pistas por nombre de clase cuando no hay CSS
                if props.align.is_none()
                    && classes
                        .iter()
                        .any(|c| c.contains("center") || c.contains("centr"))
                {
                    props.align = Some(2);
                }
                let block = is_block(&name);
                if block {
                    bld.flush();
                }
                match name.as_str() {
                    "br" => bld.spans.push(Span {
                        text: "\u{2028}".into(),
                        ..Default::default()
                    }),
                    "hr" => {
                        bld.flush();
                        bld.push(Block::Rule);
                    }
                    "img" | "image" => {
                        let src = attr(&attrs, "src").or_else(|| attr(&attrs, "href"));
                        let src = src.or_else(|| {
                            attr(&attrs, "recindex")
                                .map(|r| format!("recindex:{}", r.trim_start_matches('0')))
                        });
                        if let Some(src) = src {
                            if !src.starts_with("data:") {
                                bld.flush();
                                let path = if src.starts_with("recindex:") {
                                    src
                                } else if let Some(k) = src.strip_prefix("kindle:embed:") {
                                    // índice en base 32 (0-9, A-V)
                                    let code: String = k
                                        .chars()
                                        .take_while(|c| c.is_ascii_alphanumeric())
                                        .collect();
                                    let n = u32::from_str_radix(&code.to_ascii_uppercase(), 32)
                                        .unwrap_or(0);
                                    format!("recindex:{n}")
                                } else {
                                    join_path(base, &percent_decode(&src))
                                };
                                bld.push(Block::Image { path });
                            }
                        }
                    }
                    "td" => bld.spans.push(Span {
                        text: "  ".into(),
                        ..Default::default()
                    }),
                    _ => {}
                }
                if !self_closing && !VOID.contains(&name.as_str()) {
                    stack.push(Frame { name, props, block });
                }
            }
            Tok::Close(name) => {
                if matches!(name.as_str(), "head" | "script" | "style" | "title") {
                    skip = (skip - 1).max(0);
                    continue;
                }
                if name == "body" {
                    bld.flush();
                    in_body = false;
                }
                if is_block(&name) {
                    bld.flush();
                }
                if let Some(pos) = stack.iter().rposition(|f| f.name == name) {
                    stack.truncate(pos);
                }
            }
            Tok::Text(text) => {
                if skip > 0 || !in_body {
                    continue;
                }
                let (italic, bold, scale, align, indent, kind) = current(&stack);
                if bld.spans.is_empty() {
                    bld.kind = kind;
                    bld.align = align;
                    bld.indent = indent;
                    let bscale = stack
                        .iter()
                        .filter(|f| f.block && !f.name.starts_with('h'))
                        .filter_map(|f| f.props.scale)
                        .product::<f32>();
                    bld.scale = bscale;
                }
                bld.spans.push(Span {
                    text,
                    bold,
                    italic,
                    small: scale < 0.9,
                    scale,
                });
            }
        }
    }
    bld.flush();
    if !bld.pending_anchors.is_empty() {
        let n = bld.blocks.len().saturating_sub(1);
        for a in bld.pending_anchors.drain(..) {
            bld.anchors.insert(a, n);
        }
    }
    Chapter {
        blocks: bld.blocks,
        anchors: bld.anchors,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn paths() {
        assert_eq!(join_path("OEBPS/text/", "../img/a.jpg"), "OEBPS/img/a.jpg");
        assert_eq!(join_path("", "a.html#x"), "a.html#x");
        assert_eq!(
            decode_entities("a&nbsp;b &amp; &#233; &#xe9;"),
            "a\u{a0}b & é é"
        );
    }
    #[test]
    fn blocks() {
        let c = html_to_blocks(
            "<html><head><title>x</title></head><body><h1 id='c1'>Cap</h1><p>Hola <i>mundo</i>\n cruel</p><img src='../i.png'/></body></html>",
            "t/",
            &Css::default(),
        );
        assert_eq!(c.blocks.len(), 3);
        assert_eq!(c.anchors["c1"], 0);
    }
    #[test]
    fn css() {
        let mut css = Css::default();
        css.parse("p.c { text-align: center } .it{font-style:italic} p{text-indent:0}");
        let c = html_to_blocks(
            "<body><p class='c'>A</p><p>B <span class='it'>x</span></p></body>",
            "",
            &css,
        );
        match &c.blocks[0] {
            Block::Para { kind, .. } => assert_eq!(*kind, ParaKind::Center),
            _ => panic!(),
        }
        match &c.blocks[1] {
            Block::Para { spans, indent, .. } => {
                assert_eq!(*indent, Some(0.0));
                assert!(spans[1].italic);
            }
            _ => panic!(),
        }
    }
}

/// Índice generado a partir de los títulos (libros sin índice, MOBI).
pub fn toc_from_headings(chapters: &[Chapter]) -> Vec<TocEntry> {
    let mut toc = Vec::new();
    for (ci, ch) in chapters.iter().enumerate() {
        for (bi, b) in ch.blocks.iter().enumerate() {
            if let Block::Para {
                spans,
                kind: ParaKind::Heading(l),
                ..
            } = b
            {
                if *l <= 3 {
                    let t: String = spans
                        .iter()
                        .map(|s| s.text.as_str())
                        .collect::<String>()
                        .replace('\n', " ");
                    let t = t.trim();
                    if !t.is_empty() && t.chars().count() < 120 {
                        toc.push(TocEntry {
                            title: t.to_string(),
                            chapter: ci,
                            block: bi,
                            level: (*l as usize).saturating_sub(1),
                        });
                    }
                }
            }
        }
    }
    toc
}

pub fn find_opf_path(ar: &mut Archive) -> Option<String> {
    find_opf(ar)
}

// ---------------------------------------------------------------------------
// Texto de MOBI (PalmDoc) con eliminación correcta de las entradas finales

fn be16(b: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_be_bytes([*b.get(o)?, *b.get(o + 1)?]))
}
fn be32(b: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_be_bytes([
        *b.get(o)?,
        *b.get(o + 1)?,
        *b.get(o + 2)?,
        *b.get(o + 3)?,
    ]))
}

fn trailing_size(data: &[u8]) -> usize {
    let mut res = 0usize;
    let start = data.len().saturating_sub(4);
    for &v in &data[start..] {
        if v & 0x80 != 0 {
            res = 0;
        }
        res = (res << 7) | (v & 0x7f) as usize;
    }
    res
}

fn palmdoc_decompress(src: &[u8]) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::with_capacity(src.len() * 2);
    let mut i = 0;
    while i < src.len() {
        let c = src[i];
        i += 1;
        match c {
            0x01..=0x08 => {
                let n = c as usize;
                out.extend_from_slice(&src[i..(i + n).min(src.len())]);
                i += n;
            }
            0x00 | 0x09..=0x7f => out.push(c),
            0x80..=0xbf => {
                if i >= src.len() {
                    break;
                }
                let pair = ((c as usize) << 8) | src[i] as usize;
                i += 1;
                let dist = (pair & 0x3fff) >> 3;
                let len = (pair & 0x07) + 3;
                if dist == 0 || dist > out.len() {
                    continue;
                }
                let start = out.len() - dist;
                for k in 0..len {
                    let b = out[start + k];
                    out.push(b);
                }
            }
            0xc0..=0xff => {
                out.push(b' ');
                out.push(c ^ 0x80);
            }
        }
    }
    out
}

fn cp1252(b: &[u8]) -> String {
    const HI: [char; 32] = [
        '€', '\u{81}', '‚', 'ƒ', '„', '…', '†', '‡', 'ˆ', '‰', 'Š', '‹', 'Œ', '\u{8d}', 'Ž',
        '\u{8f}', '\u{90}', '‘', '’', '“', '”', '•', '–', '—', '˜', '™', 'š', '›', 'œ', '\u{9d}',
        'ž', 'Ÿ',
    ];
    b.iter()
        .map(|&c| {
            if (0x80..0xa0).contains(&c) {
                HI[(c - 0x80) as usize]
            } else {
                c as char
            }
        })
        .collect()
}

/// Texto HTML de un MOBI comprimido con PalmDoc (o sin compresión).
pub fn mobi_text(data: &[u8]) -> Option<String> {
    let n = be16(data, 76)? as usize;
    let off = |i: usize| be32(data, 78 + i * 8).map(|v| v as usize);
    let rec = |i: usize| -> Option<&[u8]> {
        let a = off(i)?;
        let b = if i + 1 < n { off(i + 1)? } else { data.len() };
        data.get(a..b)
    };
    let r0 = rec(0)?;
    let compression = be16(r0, 0)?;
    let count = be16(r0, 8)? as usize;
    if be16(r0, 12)? != 0 {
        return None;
    }
    let mut encoding = 1252;
    let mut flags = 0u16;
    if r0.get(16..20) == Some(b"MOBI") {
        let hlen = be32(r0, 20)? as usize;
        encoding = be32(r0, 28)?;
        if hlen >= 0xe4 {
            flags = be16(r0, 16 + 0xe2).unwrap_or(0);
        }
    }
    if compression != 1 && compression != 2 {
        return None; // HUFF/CDIC: no soportado aquí
    }
    let mut out = Vec::new();
    for i in 1..=count.min(n.saturating_sub(1)) {
        let mut d = rec(i)?;
        let mut f = flags >> 1;
        while f != 0 {
            if f & 1 != 0 {
                let s = trailing_size(d);
                d = &d[..d.len().saturating_sub(s)];
            }
            f >>= 1;
        }
        if flags & 1 != 0 && !d.is_empty() {
            let m = (d[d.len() - 1] & 3) as usize + 1;
            d = &d[..d.len().saturating_sub(m)];
        }
        if compression == 2 {
            out.extend(palmdoc_decompress(d));
        } else {
            out.extend_from_slice(d);
        }
    }
    Some(if encoding == 65001 {
        String::from_utf8_lossy(&out).into_owned()
    } else {
        cp1252(&out)
    })
}
