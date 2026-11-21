# Lector

Lector de **PDF y EPUB** escrito en **Rust puro**, pensado para que leer en pantalla se parezca lo más posible a leer un libro o una revista en papel.

- **Libro abierto a doble página**, con lomo sombreado, canto de hojas que crece o mengua según avanzas y sombra sobre la mesa.
- **Pasar página con curvatura**: arrastra la esquina con el ratón o el trackpad, o haz clic o usa las flechas. La hoja se curva, se ilumina y proyecta su sombra.
- **EPUB maquetado como un libro impreso**: tipografía Crimson Text, justificado palabra a palabra (no letra a letra), sangría de primera línea, control de viudas y huérfanas, capítulos en página nueva, títulos con aire, cabeceras con el título del libro y del capítulo, y folio.
- **PDF nítido**: se renderiza con [hayro](https://github.com/LaurenzV/hayro), un rasterizador de PDF en Rust puro, a la resolución exacta de la pantalla (Retina incluido). Zoom con pellizco, doble clic o ⌘+/⌘−.
- **Revistas**: con «Portada sola», la portada va sola a la derecha y el resto en pliegos, como en papel.
- **Temas**: Día, Sepia, Gris y Noche (en Noche, los PDF se invierten conservando los tonos).
- **Menú lateral** como Apple Books: Leyendo, Para leer, Terminados, Todo, Libros, PDF; **Autores** (índice alfabético) y **Temas** (leídos de los propios EPUB); **Mis colecciones** propias; buscador por título, autor o tema y orden por recientes, título, autor o progreso.
- **Biblioteca** con portadas al estilo de Apple Books, búsqueda, progreso y recuerdo de la última página. Puede **importar los libros de Apple Books**.
- **Portadas**: detecta la portada aunque el EPUB la declare de forma poco estándar. Si un libro no tiene, puedes **crearla** (diseño tipográfico con 6 estilos o una imagen tuya) y **añadirla al propio archivo**: dentro del EPUB o como primera página del PDF. Se guarda antes una copia del original en `~/Library/Application Support/Lector/copias`.
- Botón **«Ver una página / Ver dos páginas»** en la barra del lector.
- Índice, marcadores (cinta roja en la página) y barra de progreso.

## Atajos

| Tecla | Acción |
|---|---|
| → / Espacio / AvPág | Página siguiente |
| ← / RePág / ⌫ | Página anterior |
| Inicio / Fin | Principio / final |
| T | Índice y marcadores |
| B | Marcar página |
| F | Pantalla completa |
| ⌘+ / ⌘− | Tamaño de letra (EPUB) o zoom (PDF) |
| Esc | Cerrar paneles o volver a la biblioteca |

También puedes arrastrar archivos o carpetas a la ventana.

## Instalar en Mac (sin Xcode)

GitHub Actions compila la app en cada `push` y la publica en **Releases** (`Lector-macOS.zip`, universal: Apple Silicon e Intel).

1. Descarga `Lector-macOS.zip` de la última versión en *Releases* y descomprímelo.
2. Mueve `Lector.app` a *Aplicaciones*.
3. La app no está firmada con un certificado de Apple. La primera vez macOS la bloqueará:
   - Abre *Ajustes del Sistema → Privacidad y seguridad* y pulsa **«Abrir igualmente»**, o
   - En Terminal: `xattr -dr com.apple.quarantine /Applications/Lector.app`

## Compilar a mano

```sh
cargo run --release -- libro.epub
```

En Linux o Windows también funciona. `--no-default-features --features glow` usa OpenGL en lugar de wgpu/Metal.

## Estructura

| Archivo | Qué hace |
|---|---|
| `src/epub.rs` | Lee EPUB 2/3 (zip o carpeta), OPF, índice NCX/nav y convierte el XHTML en bloques |
| `src/layout.rs` | Paginación tipográfica y justificación |
| `src/pdfdoc.rs` | Renderizado de PDF en un hilo aparte (hayro) |
| `src/book.rs` | Papel, lomo, canto, sombras y la animación de la hoja curvada |
| `src/covers.rs` | Portadas en segundo plano con caché en disco, importación de carpetas |
| `src/app.rs` | Biblioteca, lector, barras, índice y ajustes |

## Limitaciones conocidas

- PDF con contraseña no soportado (todavía no lo admite hayro).
- El CSS de los EPUB se ignora casi entero: se impone una maquetación de libro clásica.
- Sin DRM: los libros comprados en Apple Books con protección FairPlay no se pueden abrir. Los EPUB y PDF sin DRM sí.

Fuente Crimson Text © The Crimson Text Project Authors, licencia SIL OFL (`assets/fonts/OFL.txt`).
