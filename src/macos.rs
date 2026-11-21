//! macOS: recibir los archivos abiertos desde Finder («Abrir con», doble clic,
//! arrastrar al icono del Dock).

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{NSApplication, NSApplicationDelegate};
use objc2_foundation::{MainThreadMarker, NSArray, NSObject, NSObjectProtocol, NSURL};
use std::sync::Mutex;

pub static OPENED: Mutex<Vec<String>> = Mutex::new(Vec::new());
pub static CTX: std::sync::OnceLock<eframe::egui::Context> = std::sync::OnceLock::new();

define_class!(
    // SAFETY: NSObject no tiene requisitos de subclase y Delegate no implementa Drop.
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[name = "LectorAppDelegate"]
    struct Delegate;

    // SAFETY: sin requisitos.
    unsafe impl NSObjectProtocol for Delegate {}

    // SAFETY: la firma es la del protocolo.
    unsafe impl NSApplicationDelegate for Delegate {
        #[unsafe(method(application:openURLs:))]
        fn open_urls(&self, _application: &NSApplication, urls: &NSArray<NSURL>) {
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
    }
);

impl Delegate {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        // SAFETY: la firma de `init` de NSObject es correcta.
        unsafe { msg_send![super(this), init] }
    }
}

/// Instala el delegado. Llamar después de crear el EventLoop.
pub fn install() {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let delegate = Delegate::new(mtm);
    let app = NSApplication::sharedApplication(mtm);
    app.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    // el delegado es una referencia débil: se mantiene vivo toda la ejecución
    std::mem::forget(delegate);
}

pub fn take_opened() -> Vec<String> {
    OPENED
        .lock()
        .map(|mut o| std::mem::take(&mut *o))
        .unwrap_or_default()
}
