//! Normalización de autores y temas, y búsqueda automática de temas en
//! Open Library para los libros que no los traen.

/// "Huxley, Aldous" -> "Aldous Huxley"; espacios y mayúsculas coherentes.
pub fn normalize_author(s: &str) -> String {
    let t: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    let t = t
        .trim()
        .trim_end_matches(['.', ';', ','])
        .trim()
        .to_string();
    // varios autores: se queda con el primero
    let t = t.split([';', '&']).next().unwrap_or("").trim().to_string();
    let t = match t.split_once(',') {
        Some((last, first))
            if !first.trim().is_empty()
                && !last.trim().is_empty()
                && !first.chars().any(|c| c.is_ascii_digit())
                && first.split_whitespace().count() <= 3
                && last.split_whitespace().count() <= 3 =>
        {
            format!("{} {}", first.trim(), last.trim())
        }
        _ => t,
    };
    // TODO EN MAYÚSCULAS -> Tipo Título
    if t.chars()
        .filter(|c| c.is_alphabetic())
        .all(|c| c.is_uppercase())
        && t.chars().any(|c| c.is_alphabetic())
    {
        t.split(' ')
            .map(|w| {
                let mut c = w.chars();
                match c.next() {
                    Some(f) => f.to_uppercase().collect::<String>() + &c.as_str().to_lowercase(),
                    None => String::new(),
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    } else {
        t
    }
}

/// Clave para comparar autores (sin acentos ni mayúsculas).
pub fn author_key(s: &str) -> String {
    normalize_author(s)
        .to_lowercase()
        .chars()
        .map(|c| match c {
            'á' | 'à' | 'ä' => 'a',
            'é' | 'è' | 'ë' => 'e',
            'í' | 'ì' | 'ï' => 'i',
            'ó' | 'ò' | 'ö' => 'o',
            'ú' | 'ù' | 'ü' => 'u',
            '.' => ' ',
            c => c,
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Sinónimos (español e inglés) -> tema canónico en español.
const MAP: &[(&[&str], &str)] = &[
    (
        &[
            "ciencia ficcion",
            "ciencia-ficcion",
            "science fiction",
            "sci-fi",
            "scifi",
            "cf",
        ],
        "Ciencia ficción",
    ),
    (
        &[
            "fantasia",
            "fantasy",
            "fantastica",
            "fantastico",
            "fantasía épica",
            "fantasia epica",
        ],
        "Fantasía",
    ),
    (
        &[
            "distopia",
            "dystopia",
            "dystopias",
            "distopica",
            "dystopian",
        ],
        "Distopía",
    ),
    (&["terror", "horror", "horror tales", "miedo"], "Terror"),
    (
        &[
            "policiaca",
            "policiaco",
            "novela negra",
            "negra",
            "crimen",
            "crime",
            "detective and mystery stories",
            "detective",
            "noir",
            "mystery",
            "misterio",
            "intriga",
        ],
        "Novela negra y misterio",
    ),
    (&["thriller", "suspense", "suspenso"], "Thriller"),
    (
        &["romantica", "romance", "romantico", "love stories", "amor"],
        "Romántica",
    ),
    (
        &[
            "historica",
            "novela historica",
            "historical fiction",
            "historico",
        ],
        "Novela histórica",
    ),
    (
        &["aventuras", "aventura", "adventure", "adventure stories"],
        "Aventuras",
    ),
    (
        &["belico", "belica", "guerra", "war", "war stories"],
        "Bélica",
    ),
    (
        &["humor", "humour", "humor and satire", "satira", "satire"],
        "Humor",
    ),
    (&["poesia", "poetry", "poemas", "poems"], "Poesía"),
    (&["teatro", "drama", "plays"], "Teatro"),
    (
        &["clasicos", "clasico", "classics", "classic literature"],
        "Clásicos",
    ),
    (
        &[
            "infantil",
            "children",
            "juvenile fiction",
            "children's fiction",
            "cuentos infantiles",
        ],
        "Infantil",
    ),
    (
        &["juvenil", "young adult", "young adult fiction"],
        "Juvenil",
    ),
    (
        &[
            "biografia",
            "biography",
            "autobiografia",
            "autobiography",
            "memorias",
            "memoirs",
        ],
        "Biografías y memorias",
    ),
    (&["historia", "history"], "Historia"),
    (&["filosofia", "philosophy"], "Filosofía"),
    (&["psicologia", "psychology"], "Psicología"),
    (
        &[
            "autoayuda",
            "self-help",
            "self help",
            "desarrollo personal",
            "crecimiento personal",
        ],
        "Autoayuda",
    ),
    (
        &["economia", "economics", "finanzas", "finance"],
        "Economía",
    ),
    (
        &["empresa", "business", "management", "negocios", "marketing"],
        "Empresa",
    ),
    (&["politica", "politics", "political science"], "Política"),
    (
        &["sociologia", "sociology", "sociedad", "social science"],
        "Sociología",
    ),
    (
        &[
            "ciencia",
            "science",
            "divulgacion",
            "divulgacion cientifica",
            "popular science",
        ],
        "Ciencia",
    ),
    (
        &[
            "naturaleza",
            "nature",
            "ecologia",
            "ecology",
            "medio ambiente",
        ],
        "Naturaleza",
    ),
    (
        &[
            "salud",
            "health",
            "medicina",
            "medicine",
            "nutricion",
            "nutrition",
        ],
        "Salud",
    ),
    (&["deporte", "deportes", "sports"], "Deporte"),
    (&["cocina", "cooking", "recetas", "gastronomia"], "Cocina"),
    (
        &[
            "arte",
            "art",
            "pintura",
            "painting",
            "diseno",
            "design",
            "diseño grafico",
            "fotografia",
            "photography",
        ],
        "Arte y diseño",
    ),
    (&["musica", "music", "jazz"], "Música"),
    (&["viajes", "travel", "viaje"], "Viajes"),
    (
        &["religion", "religious", "espiritualidad", "spirituality"],
        "Religión y espiritualidad",
    ),
    (&["educacion", "education", "pedagogia"], "Educación"),
    (
        &[
            "idiomas",
            "language",
            "languages",
            "linguistica",
            "linguistics",
        ],
        "Idiomas",
    ),
    (
        &[
            "informatica",
            "computers",
            "programacion",
            "programming",
            "tecnologia",
            "technology",
        ],
        "Tecnología",
    ),
    (
        &[
            "comic",
            "comics",
            "novela grafica",
            "graphic novels",
            "manga",
        ],
        "Cómic",
    ),
    (&["ensayo", "essays", "essay"], "Ensayo"),
    (
        &["relatos", "cuentos", "short stories", "relato"],
        "Relatos",
    ),
    (&["novela", "fiction", "narrativa", "novels"], "Novela"),
];

fn fold(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .map(|c| match c {
            'á' | 'à' | 'ä' => 'a',
            'é' | 'è' | 'ë' => 'e',
            'í' | 'ì' | 'ï' => 'i',
            'ó' | 'ò' | 'ö' => 'o',
            'ú' | 'ù' | 'ü' => 'u',
            _ => c,
        })
        .collect::<String>()
        .trim()
        .to_string()
}

/// Tema canónico si el texto coincide con alguno conocido.
pub fn canonical(s: &str) -> Option<&'static str> {
    let f = fold(s);
    for (syn, name) in MAP {
        if syn.iter().any(|x| fold(x) == f) {
            return Some(name);
        }
    }
    None
}

/// Tema canónico buscando también dentro de frases largas ("Science fiction, American").
fn canonical_loose(s: &str) -> Option<&'static str> {
    if let Some(c) = canonical(s) {
        return Some(c);
    }
    let f = fold(s);
    for (syn, name) in MAP {
        if syn.iter().any(|x| {
            let x = fold(x);
            x.len() >= 5
                && (f.starts_with(&format!("{x},"))
                    || f.starts_with(&format!("{x} "))
                    || f.ends_with(&format!(" {x}")))
        }) {
            return Some(name);
        }
    }
    None
}

/// Normaliza la lista de temas de un libro.
pub fn normalize_subjects(v: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for s in v {
        let t = canonical_loose(s)
            .map(|c| c.to_string())
            .unwrap_or_else(|| s.trim().to_string());
        if !t.is_empty() && !out.iter().any(|o| fold(o) == fold(&t)) {
            out.push(t);
        }
    }
    // "Novela" solo si no hay un género más concreto
    if out.len() > 1 {
        out.retain(|t| t != "Novela");
    }
    out
}

fn fetch_counts(url: &str) -> Vec<(&'static str, usize)> {
    let Ok(out) = std::process::Command::new("curl")
        .args([
            "-s",
            "-L",
            "--max-time",
            "12",
            "-A",
            "Lector/0.1 (ereader)",
            url,
        ])
        .output()
    else {
        return vec![];
    };
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(&out.stdout) else {
        return vec![];
    };
    let mut counts: Vec<(&'static str, usize)> = Vec::new();
    if let Some(docs) = v.get("docs").and_then(|d| d.as_array()) {
        for d in docs {
            if let Some(subs) = d.get("subject").and_then(|s| s.as_array()) {
                for s in subs.iter().filter_map(|s| s.as_str()) {
                    if let Some(c) = canonical_loose(s) {
                        match counts.iter_mut().find(|x| x.0 == c) {
                            Some(x) => x.1 += 1,
                            None => counts.push((c, 1)),
                        }
                    }
                }
            }
        }
    }
    counts
}

/// Busca temas en Open Library (usa el `curl` del sistema; sin código nativo en la app).
pub fn lookup_online(title: &str, author: &str) -> Vec<String> {
    let t = title.split([':', '(', '[']).next().unwrap_or(title).trim();
    if t.is_empty() {
        return vec![];
    }
    let mut url = format!(
        "https://openlibrary.org/search.json?limit=5&fields=subject&title={}",
        crate::tools::url_encode(t)
    );
    let a = normalize_author(author);
    if !a.is_empty() && a != "Autor desconocido" {
        let last = a.split_whitespace().last().unwrap_or("");
        url.push_str(&format!("&author={}", crate::tools::url_encode(last)));
    }
    let mut counts = fetch_counts(&url);
    if counts.is_empty() {
        // segundo intento: búsqueda libre (útil con títulos traducidos)
        let q = format!("{} {}", t, a.split_whitespace().last().unwrap_or(""));
        counts = fetch_counts(&format!(
            "https://openlibrary.org/search.json?limit=8&fields=subject&q={}",
            crate::tools::url_encode(q.trim())
        ));
    }
    counts.sort_by(|a, b| b.1.cmp(&a.1));
    let v: Vec<String> = counts
        .into_iter()
        .take(4)
        .map(|(c, _)| c.to_string())
        .collect();
    normalize_subjects(&v)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn autores() {
        assert_eq!(normalize_author("Huxley, Aldous"), "Aldous Huxley");
        assert_eq!(normalize_author("ALDOUS HUXLEY"), "Aldous Huxley");
        assert_eq!(author_key("Huxley, Aldous"), author_key("Aldous  Huxley."));
    }
    #[test]
    fn temas() {
        assert_eq!(canonical("Ciencia Ficción"), Some("Ciencia ficción"));
        assert_eq!(
            canonical_loose("Science fiction, English"),
            Some("Ciencia ficción")
        );
        assert_eq!(
            normalize_subjects(&["Novela".into(), "Ciencia-ficción".into()]),
            vec!["Ciencia ficción".to_string()]
        );
    }
}
