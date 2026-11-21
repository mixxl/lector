//! macOS: recibir los archivos abiertos desde Finder («Abrir con», doble clic,
//! arrastrar al icono del Dock).
//!
//! winit ya registra su propio delegado de NSApplication; no se sustituye
//! (eso impediría arrancar la app). Solo se le añade el método
//! `application:openURLs:` en tiempo de ejecución.

use objc2::runtime::{AnyClass, AnyObject, Imp, Sel};
use objc2::sel;
use objc2_app_kit::NSApplication;
use objc2_foundation::{MainThreadMarker, NSArray, NSURL};
use std::sync::Mutex;

pub static OPENED: Mutex<Vec<String>> = Mutex::new(Vec::new());
pub static CTX: std::sync::OnceLock<eframe::egui::Context> = std::sync::OnceLock::new();

unsafe extern "C-unwind" fn open_urls(
    _this: *mut AnyObject,
    _cmd: Sel,
    _app: *mut AnyObject,
    urls: *mut NSArray<NSURL>,
) {
    if urls.is_null() {
        return;
    }
    // SAFETY: AppKit pasa un NSArray<NSURL> válido.
    let urls = unsafe { &*urls };
    let mut v = Vec::new();
    for i in 0..urls.count() {
        let u = urls.objectAtIndex(i);
        if let Some(p) = u.path() {
            v.push(p.to_string());
        }
    }
    if let Ok(mut o) = OPENED.lock() {
        o.extend(v);
    }
    if let Some(c) = CTX.get() {
        c.request_repaint();
    }
}

/// Añade `application:openURLs:` al delegado de winit. Llamar después de crear el EventLoop.
pub fn install() {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let app = NSApplication::sharedApplication(mtm);
    let Some(delegate) = app.delegate() else {
        return;
    };
    let ptr: *const AnyObject = objc2::rc::Retained::as_ptr(&delegate).cast();
    // SAFETY: puntero a un objeto Objective-C vivo; la firma coincide con "v@:@@".
    unsafe {
        let cls = objc2::ffi::object_getClass(ptr) as *mut AnyClass;
        if cls.is_null() {
            return;
        }
        let f: unsafe extern "C-unwind" fn(
            *mut AnyObject,
            Sel,
            *mut AnyObject,
            *mut NSArray<NSURL>,
        ) = open_urls;
        let imp: Imp = std::mem::transmute(f);
        let _ =
            objc2::ffi::class_addMethod(cls, sel!(application:openURLs:), imp, c"v@:@@".as_ptr());
    }
}

pub fn take_opened() -> Vec<String> {
    OPENED
        .lock()
        .map(|mut o| std::mem::take(&mut *o))
        .unwrap_or_default()
}
