//! Dibujo "físico" del libro: papel, sombra del lomo, canto de páginas,
//! y la animación de pasar página con curvatura.

use eframe::egui::{self, Color32, Mesh, Pos2, Rect, Shape, TextureId, epaint::Vertex, pos2, vec2};

#[derive(Clone, Copy, PartialEq)]
pub enum Side {
    Left,
    Right,
}

#[derive(Clone, Copy)]
pub struct Theme {
    pub desk: Color32,
    pub desk2: Color32,
    pub paper: Color32,
    pub ink: Color32,
    pub faint: Color32,
    pub night: bool,
    pub grain: u8,
}

pub fn theme(idx: usize) -> Theme {
    match idx {
        1 => Theme {
            desk: Color32::from_rgb(58, 46, 36),
            desk2: Color32::from_rgb(92, 74, 58),
            paper: Color32::from_rgb(244, 233, 210),
            ink: Color32::from_rgb(60, 44, 30),
            faint: Color32::from_rgb(140, 118, 92),
            night: false,
            grain: 26,
        },
        2 => Theme {
            desk: Color32::from_rgb(40, 42, 46),
            desk2: Color32::from_rgb(70, 73, 78),
            paper: Color32::from_rgb(214, 214, 210),
            ink: Color32::from_rgb(34, 34, 34),
            faint: Color32::from_rgb(110, 110, 108),
            night: false,
            grain: 18,
        },
        3 => Theme {
            desk: Color32::from_rgb(8, 8, 10),
            desk2: Color32::from_rgb(28, 28, 32),
            paper: Color32::from_rgb(30, 30, 32),
            ink: Color32::from_rgb(196, 190, 178),
            faint: Color32::from_rgb(110, 106, 100),
            night: true,
            grain: 10,
        },
        _ => Theme {
            desk: Color32::from_rgb(64, 60, 56),
            desk2: Color32::from_rgb(104, 98, 92),
            paper: Color32::from_rgb(252, 249, 242),
            ink: Color32::from_rgb(28, 26, 24),
            faint: Color32::from_rgb(130, 124, 116),
            night: false,
            grain: 22,
        },
    }
}

pub const THEME_NAMES: [&str; 4] = ["Día", "Sepia", "Gris", "Noche"];

fn mul(c: Color32, f: f32) -> Color32 {
    let f = f.clamp(0.0, 2.0);
    Color32::from_rgba_premultiplied(
        (c.r() as f32 * f).min(255.0) as u8,
        (c.g() as f32 * f).min(255.0) as u8,
        (c.b() as f32 * f).min(255.0) as u8,
        c.a(),
    )
}

/// Oscurecimiento del papel junto al lomo (u = 0 en el lomo, 1 en el borde exterior).
fn gutter_shade(u: f32, night: bool) -> f32 {
    let k = if night { 0.5 } else { 1.0 };
    1.0 - k
        * (0.30 * (-u * 22.0).exp() + 0.07 * (-u * 5.0).exp() + 0.05 * (-(1.0 - u) * 60.0).exp())
}

/// Tira subdividida (para poder curvarla) con textura opcional.
pub fn strip(
    rect: Rect,
    side: Side,
    tex: TextureId,
    uv: Rect,
    base: Color32,
    night: bool,
    shade: bool,
    cols: usize,
) -> Mesh {
    let mut m = Mesh::with_texture(tex);
    for i in 0..=cols {
        let t = i as f32 / cols as f32;
        let x = rect.left() + rect.width() * t;
        let u = if side == Side::Right { t } else { 1.0 - t };
        let col = if shade {
            mul(base, gutter_shade(u, night))
        } else {
            base
        };
        let ux = uv.left() + uv.width() * t;
        m.vertices.push(Vertex {
            pos: pos2(x, rect.top()),
            uv: pos2(ux, uv.top()),
            color: col,
        });
        m.vertices.push(Vertex {
            pos: pos2(x, rect.bottom()),
            uv: pos2(ux, uv.bottom()),
            color: col,
        });
        if i > 0 {
            let a = (i as u32 - 1) * 2;
            m.indices
                .extend_from_slice(&[a, a + 1, a + 2, a + 1, a + 3, a + 2]);
        }
    }
    m
}

/// Convierte formas egui (texto, imágenes) en mallas, respetando los recortes
/// por triángulo para que puedan deformarse después.
pub fn shapes_to_meshes(ctx: &egui::Context, shapes: Vec<(Rect, Shape)>, ppp: f32) -> Vec<Mesh> {
    let clipped = shapes
        .into_iter()
        .map(|(clip, shape)| egui::epaint::ClippedShape::new(clip, shape))
        .collect();
    let prims = ctx.tessellate(clipped, ppp);
    let mut out = Vec::new();
    for p in prims {
        if let egui::epaint::Primitive::Mesh(m) = p.primitive {
            let clip = p.clip_rect;
            let mut nm = Mesh::with_texture(m.texture_id);
            let mut remap = vec![u32::MAX; m.vertices.len()];
            for tri in m.indices.chunks_exact(3) {
                let c = (m.vertices[tri[0] as usize].pos.to_vec2()
                    + m.vertices[tri[1] as usize].pos.to_vec2()
                    + m.vertices[tri[2] as usize].pos.to_vec2())
                    / 3.0;
                if !clip.contains(c.to_pos2()) {
                    continue;
                }
                for &i in tri {
                    if remap[i as usize] == u32::MAX {
                        remap[i as usize] = nm.vertices.len() as u32;
                        nm.vertices.push(m.vertices[i as usize]);
                    }
                    nm.indices.push(remap[i as usize]);
                }
            }
            if !nm.indices.is_empty() {
                out.push(nm);
            }
        }
    }
    out
}

/// Parámetros de la hoja que gira.
#[derive(Clone, Copy)]
pub struct Flip {
    pub spine: f32,
    pub width: f32,
    pub cy: f32,
    pub theta: f32,
}

impl Flip {
    /// Posición en la hoja a distancia `u` del lomo (0..1) -> (x relativo, profundidad).
    fn sheet(&self, u: f32) -> (f32, f32, f32) {
        let c = 0.55 * self.theta.sin();
        let a = self.theta + c * u;
        let (xs, zs) = if c.abs() < 1e-4 {
            (u * self.theta.cos(), u * self.theta.sin())
        } else {
            (
                ((self.theta + c * u).sin() - self.theta.sin()) / c,
                (self.theta.cos() - (self.theta + c * u).cos()) / c,
            )
        };
        (xs, zs, a)
    }

    pub fn map(&self, p: Pos2, back: bool) -> (Pos2, f32) {
        let u = if back {
            (self.spine - p.x) / self.width
        } else {
            (p.x - self.spine) / self.width
        };
        let u = u.clamp(0.0, 1.2);
        let (xs, zs, a) = self.sheet(u);
        let persp = 1.0 + zs * 0.10;
        let x = self.spine + self.width * xs * persp;
        let y = self.cy + (p.y - self.cy) * persp;
        let facing = a.cos().abs();
        let light = 0.62 + 0.38 * facing.powf(0.6);
        // brillo especular suave
        let spec = (-(a - 0.9).powi(2) * 6.0).exp() * 0.08;
        (pos2(x, y), light + spec)
    }

    pub fn edge_x(&self) -> f32 {
        let (xs, zs, _) = self.sheet(1.0);
        self.spine + self.width * xs * (1.0 + zs * 0.10)
    }

    pub fn transform(&self, meshes: &[Mesh], back: bool) -> Vec<Mesh> {
        meshes
            .iter()
            .map(|m| {
                let mut nm = m.clone();
                for v in &mut nm.vertices {
                    let (p, l) = self.map(v.pos, back);
                    v.pos = p;
                    v.color = mul(v.color, l);
                }
                nm
            })
            .collect()
    }
}

/// Degradado horizontal (sombras).
pub fn hgrad(rect: Rect, left: Color32, right: Color32) -> Shape {
    let mut m = Mesh::default();
    m.colored_vertex(rect.left_top(), left);
    m.colored_vertex(rect.right_top(), right);
    m.colored_vertex(rect.left_bottom(), left);
    m.colored_vertex(rect.right_bottom(), right);
    m.add_triangle(0, 1, 2);
    m.add_triangle(1, 3, 2);
    Shape::mesh(m)
}

/// Fondo de mesa con viñeta.
pub fn desk(painter: &egui::Painter, rect: Rect, t: &Theme) {
    painter.rect_filled(rect, 0.0, t.desk);
    let mut m = Mesh::default();
    let c = rect.center();
    let n = 48;
    m.colored_vertex(c, t.desk2);
    for i in 0..=n {
        let a = i as f32 / n as f32 * std::f32::consts::TAU;
        let p = c + vec2(
            a.cos() * rect.width() * 0.75,
            a.sin() * rect.height() * 0.85,
        );
        m.colored_vertex(p, t.desk);
        if i > 0 {
            m.add_triangle(0, i, i + 1);
        }
    }
    painter.add(Shape::mesh(m));
}

/// Canto de hojas apiladas a un lado del libro.
pub fn page_stack(painter: &egui::Painter, rect: Rect, side: Side, n: usize, t: &Theme) {
    let n = n.min(7);
    for i in (1..=n).rev() {
        let d = i as f32 * 1.6;
        let r = match side {
            Side::Left => rect.translate(vec2(-d, d * 0.35)),
            Side::Right => rect.translate(vec2(d, d * 0.35)),
        };
        let shade = 1.0 - i as f32 * 0.045;
        painter.rect_filled(r, 1.0, mul(t.paper, shade));
        let edge_x = if side == Side::Left {
            r.left()
        } else {
            r.right()
        };
        painter.line_segment(
            [pos2(edge_x, r.top() + 1.0), pos2(edge_x, r.bottom() - 1.0)],
            egui::Stroke::new(0.6, mul(t.paper, shade * 0.82)),
        );
    }
}

/// Sombra proyectada del libro sobre la mesa.
pub fn book_shadow(painter: &egui::Painter, rect: Rect, night: bool) {
    let shadow = egui::epaint::Shadow {
        offset: [0, 10],
        blur: 40,
        spread: 2,
        color: Color32::from_black_alpha(if night { 200 } else { 150 }),
    };
    painter.add(shadow.as_shape(rect, 3.0));
}

/// Cinta de marcador.
pub fn ribbon(painter: &egui::Painter, page: Rect) {
    let w = 16.0;
    let x = page.right() - 42.0;
    let top = page.top() - 4.0;
    let bot = page.top() + 54.0;
    let col = Color32::from_rgb(170, 30, 36);
    let pts = vec![
        pos2(x, top),
        pos2(x + w, top),
        pos2(x + w, bot),
        pos2(x + w / 2.0, bot - 8.0),
        pos2(x, bot),
    ];
    painter.add(Shape::convex_polygon(pts, col, egui::Stroke::NONE));
    painter.line_segment(
        [pos2(x + 2.0, top), pos2(x + 2.0, bot - 2.0)],
        egui::Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 255, 255, 40)),
    );
}

/// Textura de grano de papel (ruido suave, repetible).
pub fn grain_image(size: usize) -> egui::ColorImage {
    let mut px = Vec::with_capacity(size * size);
    let mut s: u32 = 0x9E3779B9;
    let mut rnd = || {
        s ^= s << 13;
        s ^= s >> 17;
        s ^= s << 5;
        (s % 1000) as f32 / 1000.0
    };
    let mut vals = vec![0f32; size * size];
    for v in vals.iter_mut() {
        *v = rnd();
    }
    // suavizado ligero para que parezca fibra
    for y in 0..size {
        for x in 0..size {
            let i = y * size + x;
            let r = vals[i] * 0.5
                + vals[y * size + (x + 1) % size] * 0.3
                + vals[((y + 1) % size) * size + x] * 0.2;
            let a = ((r - 0.35).max(0.0) * 255.0) as u8;
            px.push(Color32::from_rgba_premultiplied(0, 0, 0, a / 2));
        }
    }
    egui::ColorImage::new([size, size], px)
}
