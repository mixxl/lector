//! Lectura de EPUB (2 y 3), tanto empaquetados (.epub zip) como en carpeta
//! (formato en que Apple Books guarda los libros).

use std::collections::HashMap;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};

pub enum Archive {
    Zip(zip::ZipArchive<Cursor<Vec<u8>>>),
    Dir(PathBuf),
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

#[derive(Clone, Debug, Default)]
pub struct Span {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
    pub small: bool,
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

#[derive(Clone, Debug)]
pub enum Block {
    Para { spans: Vec<Span>, kind: ParaKind },
    Image { path: String },
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
pub fn quick_info(path: &Path) -> Option<(String, String, Option<Vec<u8>>)> {
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
    Some((pkg.title, pkg.author, cover))
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
    let mut cover_id = None;
    let mut manifest: HashMap<String, (String, String, String)> = HashMap::new();
    let mut spine_ids = Vec::new();
    let mut toc_id = None;
    for n in doc.descendants().filter(|n| n.is_element()) {
        match n.tag_name().name() {
            "title" if title.is_empty() => title = n.text().unwrap_or("").trim().to_string(),
            "creator" if author.is_empty() => author = n.text().unwrap_or("").trim().to_string(),
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

pub fn open(path: &Path) -> Result<Epub, String> {
    let mut ar = Archive::open(path)?;
    if has_drm(&mut ar) {
        return Err("el libro está protegido con DRM".into());
    }
    let opf_path = find_opf(&mut ar).ok_or("No se encuentra el paquete OPF")?;
    let opf = ar.read_string(&opf_path).ok_or("OPF ilegible")?;
    let pkg = parse_opf(&opf, &opf_path).ok_or("OPF inválido")?;

    let mut chapters = Vec::new();
    let mut chapter_index: HashMap<String, usize> = HashMap::new();
    for href in &pkg.spine {
        let Some(html) = ar.read_string(href) else {
            continue;
        };
        let ch = html_to_blocks(&html, &dir_of(href));
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
    Ok(Epub {
        title,
        author: pkg.author,
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
// HTML -> bloques

struct Builder {
    blocks: Vec<Block>,
    anchors: HashMap<String, usize>,
    pending_anchors: Vec<String>,
    spans: Vec<Span>,
    kind: ParaKind,
}

impl Builder {
    fn flush(&mut self) {
        // normaliza espacios
        let pre = self.kind == ParaKind::Pre;
        let mut spans: Vec<Span> = Vec::new();
        let mut last_space = true;
        for s in self.spans.drain(..) {
            let mut t = String::with_capacity(s.text.len());
            for c in s.text.chars() {
                if c == '\u{2028}' {
                    // salto explícito (<br>)
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
                } else {
                    t.push(c);
                    last_space = false;
                }
            }
            if !t.is_empty() {
                spans.push(Span { text: t, ..s });
            }
        }
        // recorta final
        while let Some(last) = spans.last_mut() {
            let trimmed = last.text.trim_end().to_string();
            if trimmed.is_empty() {
                spans.pop();
            } else {
                last.text = trimmed;
                break;
            }
        }
        if let Some(first) = spans.first_mut() {
            first.text = first.text.trim_start().to_string();
        }
        if spans.iter().any(|s| !s.text.trim().is_empty()) {
            self.push(Block::Para {
                spans,
                kind: self.kind,
            });
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

fn html_to_blocks(html: &str, base: &str) -> Chapter {
    let toks = tokenize(html);
    let mut bld = Builder {
        blocks: vec![],
        anchors: HashMap::new(),
        pending_anchors: vec![],
        spans: vec![],
        kind: ParaKind::Body,
    };
    let mut bold = 0i32;
    let mut italic = 0i32;
    let mut small = 0i32;
    let mut skip = 0i32;
    let mut in_body = !html.contains("<body");
    let mut quote = 0i32;
    let mut center_stack: Vec<(String, bool)> = vec![];
    let mut heading: Option<u8> = None;
    let mut pre = 0i32;

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
        )
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
                    continue;
                }
                if matches!(name.as_str(), "head" | "script" | "style" | "title") && !self_closing {
                    skip += 1;
                    continue;
                }
                if let Some(id) = attr(&attrs, "id") {
                    bld.pending_anchors.push(id);
                }
                if is_block(&name) {
                    bld.flush();
                    let cls = format!(
                        "{} {}",
                        attr(&attrs, "class").unwrap_or_default(),
                        attr(&attrs, "style").unwrap_or_default()
                    )
                    .to_lowercase();
                    let centered = name == "center"
                        || cls.contains("center")
                        || cls.contains("centr")
                        || attr(&attrs, "align").is_some_and(|a| a == "center");
                    if !self_closing {
                        center_stack.push((name.clone(), centered));
                    }
                    match name.as_str() {
                        "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                            heading = Some(name[1..].parse().unwrap_or(2))
                        }
                        "blockquote" => quote += 1,
                        "pre" => pre += 1,
                        _ => {}
                    }
                }
                match name.as_str() {
                    "b" | "strong" | "th" => bold += 1,
                    "i" | "em" | "cite" | "var" | "dfn" => italic += 1,
                    "small" | "sup" | "sub" => small += 1,
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
                        if let Some(src) = src {
                            if !src.starts_with("data:") {
                                bld.flush();
                                bld.push(Block::Image {
                                    path: join_path(base, &percent_decode(&src)),
                                });
                            }
                        }
                    }
                    "td" => bld.spans.push(Span {
                        text: "  ".into(),
                        ..Default::default()
                    }),
                    _ => {}
                }
                if self_closing {
                    match name.as_str() {
                        "b" | "strong" => bold -= 1,
                        "i" | "em" => italic -= 1,
                        _ => {}
                    }
                }
            }
            Tok::Close(name) => {
                if name == "body" {
                    in_body = false;
                    continue;
                }
                if matches!(name.as_str(), "head" | "script" | "style" | "title") {
                    skip = (skip - 1).max(0);
                    continue;
                }
                match name.as_str() {
                    "b" | "strong" | "th" => bold = (bold - 1).max(0),
                    "i" | "em" | "cite" | "var" | "dfn" => italic = (italic - 1).max(0),
                    "small" | "sup" | "sub" => small = (small - 1).max(0),
                    _ => {}
                }
                if is_block(&name) {
                    bld.flush();
                    if let Some(pos) = center_stack.iter().rposition(|(n, _)| *n == name) {
                        center_stack.truncate(pos);
                    }
                    match name.as_str() {
                        "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => heading = None,
                        "blockquote" => quote = (quote - 1).max(0),
                        "pre" => pre = (pre - 1).max(0),
                        _ => {}
                    }
                }
            }
            Tok::Text(text) => {
                if skip > 0 || !in_body {
                    continue;
                }
                bld.kind = if let Some(h) = heading {
                    ParaKind::Heading(h)
                } else if pre > 0 {
                    ParaKind::Pre
                } else if center_stack.iter().any(|(_, c)| *c) {
                    ParaKind::Center
                } else if quote > 0 {
                    ParaKind::Quote
                } else if center_stack.iter().any(|(n, _)| n == "li") {
                    ParaKind::ListItem
                } else {
                    ParaKind::Body
                };
                bld.spans.push(Span {
                    text,
                    bold: bold > 0,
                    italic: italic > 0,
                    small: small > 0,
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
        );
        assert_eq!(c.blocks.len(), 3);
        assert_eq!(c.anchors["c1"], 0);
    }
}

pub fn find_opf_path(ar: &mut Archive) -> Option<String> {
    find_opf(ar)
}
