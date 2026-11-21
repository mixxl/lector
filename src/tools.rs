//! Utilidades del sistema: abrir enlaces, diccionario, traducción y voz.

use std::process::{Child, Command, Stdio};

pub fn url_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

pub fn open_url(url: &str) {
    #[cfg(target_os = "macos")]
    let _ = Command::new("open").arg(url).spawn();
    #[cfg(target_os = "windows")]
    let _ = Command::new("cmd").args(["/C", "start", "", url]).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let _ = Command::new("xdg-open").arg(url).spawn();
}

/// Diccionario del sistema (macOS) para una palabra.
pub fn dictionary(word: &str) {
    let w = word.trim().trim_matches(|c: char| !c.is_alphanumeric());
    #[cfg(target_os = "macos")]
    open_url(&format!("dict://{}", url_encode(w)));
    #[cfg(not(target_os = "macos"))]
    open_url(&format!("https://dle.rae.es/{}", url_encode(w)));
}

pub fn rae(word: &str) {
    let w = word.trim().trim_matches(|c: char| !c.is_alphanumeric());
    open_url(&format!(
        "https://dle.rae.es/{}",
        url_encode(&w.to_lowercase())
    ));
}

pub fn translate(text: &str) {
    let t: String = text.chars().take(4000).collect();
    open_url(&format!(
        "https://translate.google.com/?sl=auto&tl=es&op=translate&text={}",
        url_encode(&t)
    ));
}

/// Voz del sistema que corresponde al idioma del libro (macOS `say`).
fn voice_for(lang: &str) -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        use std::sync::OnceLock;
        static VOICES: OnceLock<String> = OnceLock::new();
        let list = VOICES.get_or_init(|| {
            Command::new("say")
                .args(["-v", "?"])
                .output()
                .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
                .unwrap_or_default()
        });
        let code = match lang.get(..2).unwrap_or("es") {
            "" => "es".to_string(),
            c => c.to_string(),
        };
        // preferencia: es_ES, luego cualquier es_*
        let mut fallback = None;
        for line in list.lines() {
            let Some(pos) = line.find(&format!(" {code}_")) else {
                continue;
            };
            let name = line[..pos].trim().to_string();
            if line[pos..].contains(&format!("{code}_ES")) || code != "es" {
                return Some(name);
            }
            fallback.get_or_insert(name);
        }
        fallback
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = lang;
        None
    }
}

/// Lee un texto en voz alta. Devuelve el proceso para poder pararlo.
pub fn speak(text: &str, lang: &str, rate: u32) -> Option<Child> {
    let text: String = text.chars().filter(|c| *c != '\u{ad}').collect();
    if text.trim().is_empty() {
        return None;
    }
    #[cfg(target_os = "macos")]
    {
        let mut c = Command::new("say");
        if let Some(v) = voice_for(lang) {
            c.args(["-v", &v]);
        }
        c.args(["-r", &rate.to_string()]).arg("--").arg(text);
        c.stdout(Stdio::null()).stderr(Stdio::null()).spawn().ok()
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = voice_for(lang);
        Command::new("espeak-ng")
            .args(["-v", lang.get(..2).unwrap_or("es"), "-s", &rate.to_string()])
            .arg(text)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .ok()
    }
}
