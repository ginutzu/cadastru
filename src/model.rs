//! Modelul de date: coordonate, geometrii, atribute, straturi vectoriale și raster.

use crate::crs::Crs;
use std::fmt;
use std::path::PathBuf;
use crate::i18n::tr;

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Coord {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

impl Coord {
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y, z: 0.0 }
    }
    pub const fn xyz(x: f64, y: f64, z: f64) -> Self {
        Self { x, y, z }
    }
}

/// Dreptunghi de încadrare.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bounds {
    pub minx: f64,
    pub miny: f64,
    pub maxx: f64,
    pub maxy: f64,
}

impl Default for Bounds {
    fn default() -> Self {
        Self::empty()
    }
}

impl Bounds {
    pub const fn empty() -> Self {
        Self {
            minx: f64::INFINITY,
            miny: f64::INFINITY,
            maxx: f64::NEG_INFINITY,
            maxy: f64::NEG_INFINITY,
        }
    }
    pub fn new(minx: f64, miny: f64, maxx: f64, maxy: f64) -> Self {
        Self {
            minx,
            miny,
            maxx,
            maxy,
        }
    }
    pub fn is_valid(&self) -> bool {
        self.minx.is_finite()
            && self.miny.is_finite()
            && self.maxx.is_finite()
            && self.maxy.is_finite()
            && self.minx <= self.maxx
            && self.miny <= self.maxy
    }
    pub fn extend(&mut self, x: f64, y: f64) {
        if !x.is_finite() || !y.is_finite() {
            return;
        }
        self.minx = self.minx.min(x);
        self.miny = self.miny.min(y);
        self.maxx = self.maxx.max(x);
        self.maxy = self.maxy.max(y);
    }
    pub fn merge(&mut self, o: &Bounds) {
        if o.is_valid() {
            self.extend(o.minx, o.miny);
            self.extend(o.maxx, o.maxy);
        }
    }
    pub fn width(&self) -> f64 {
        self.maxx - self.minx
    }
    pub fn height(&self) -> f64 {
        self.maxy - self.miny
    }
    pub fn center(&self) -> (f64, f64) {
        ((self.minx + self.maxx) * 0.5, (self.miny + self.maxy) * 0.5)
    }
    pub fn intersects(&self, o: &Bounds) -> bool {
        self.minx <= o.maxx && self.maxx >= o.minx && self.miny <= o.maxy && self.maxy >= o.miny
    }
    pub fn intersection(&self, o: &Bounds) -> Option<Bounds> {
        let b = Bounds::new(
            self.minx.max(o.minx),
            self.miny.max(o.miny),
            self.maxx.min(o.maxx),
            self.maxy.min(o.maxy),
        );
        if b.minx < b.maxx && b.miny < b.maxy {
            Some(b)
        } else {
            None
        }
    }
    pub fn contains(&self, x: f64, y: f64) -> bool {
        x >= self.minx && x <= self.maxx && y >= self.miny && y <= self.maxy
    }
    pub fn expanded(&self, d: f64) -> Bounds {
        Bounds::new(self.minx - d, self.miny - d, self.maxx + d, self.maxy + d)
    }
}

impl fmt::Display for Bounds {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_valid() {
            write!(
                f,
                "[{:.6}, {:.6}] - [{:.6}, {:.6}]",
                self.minx, self.miny, self.maxx, self.maxy
            )
        } else {
            write!(f, "{}", tr!("(gol)"))
        }
    }
}

pub type Ring = Vec<Coord>;

#[derive(Clone, Debug, PartialEq)]
pub enum Geometry {
    Point(Coord),
    MultiPoint(Vec<Coord>),
    LineString(Vec<Coord>),
    MultiLineString(Vec<Vec<Coord>>),
    /// Primul inel este exteriorul, restul sunt goluri.
    Polygon(Vec<Ring>),
    MultiPolygon(Vec<Vec<Ring>>),
    Collection(Vec<Geometry>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GeomKind {
    Point,
    Line,
    Polygon,
    Mixed,
}

impl GeomKind {
    pub fn label(&self) -> &'static str {
        match self {
            GeomKind::Point => tr!("punct"),
            GeomKind::Line => tr!("linie"),
            GeomKind::Polygon => tr!("poligon"),
            GeomKind::Mixed => tr!("mixt"),
        }
    }
}

impl Geometry {
    pub fn kind(&self) -> GeomKind {
        match self {
            Geometry::Point(_) | Geometry::MultiPoint(_) => GeomKind::Point,
            Geometry::LineString(_) | Geometry::MultiLineString(_) => GeomKind::Line,
            Geometry::Polygon(_) | Geometry::MultiPolygon(_) => GeomKind::Polygon,
            Geometry::Collection(g) => {
                let mut k: Option<GeomKind> = None;
                for x in g {
                    let xk = x.kind();
                    match k {
                        None => k = Some(xk),
                        Some(prev) if prev != xk => return GeomKind::Mixed,
                        _ => {}
                    }
                }
                k.unwrap_or(GeomKind::Mixed)
            }
        }
    }

    pub fn type_name(&self) -> &'static str {
        match self {
            Geometry::Point(_) => "Point",
            Geometry::MultiPoint(_) => "MultiPoint",
            Geometry::LineString(_) => "LineString",
            Geometry::MultiLineString(_) => "MultiLineString",
            Geometry::Polygon(_) => "Polygon",
            Geometry::MultiPolygon(_) => "MultiPolygon",
            Geometry::Collection(_) => "GeometryCollection",
        }
    }

    pub fn is_empty(&self) -> bool {
        match self {
            Geometry::Point(c) => !c.x.is_finite() || !c.y.is_finite(),
            Geometry::MultiPoint(v) | Geometry::LineString(v) => v.is_empty(),
            Geometry::MultiLineString(v) | Geometry::Polygon(v) => v.iter().all(|r| r.is_empty()),
            Geometry::MultiPolygon(v) => v.iter().all(|p| p.iter().all(|r| r.is_empty())),
            Geometry::Collection(v) => v.iter().all(|g| g.is_empty()),
        }
    }

    pub fn for_each_coord(&self, f: &mut impl FnMut(&Coord)) {
        match self {
            Geometry::Point(c) => f(c),
            Geometry::MultiPoint(v) | Geometry::LineString(v) => v.iter().for_each(|c| f(c)),
            Geometry::MultiLineString(v) | Geometry::Polygon(v) => {
                v.iter().flatten().for_each(|c| f(c))
            }
            Geometry::MultiPolygon(v) => v.iter().flatten().flatten().for_each(|c| f(c)),
            Geometry::Collection(v) => v.iter().for_each(|g| g.for_each_coord(f)),
        }
    }

    pub fn for_each_coord_mut(&mut self, f: &mut impl FnMut(&mut Coord)) {
        match self {
            Geometry::Point(c) => f(c),
            Geometry::MultiPoint(v) | Geometry::LineString(v) => v.iter_mut().for_each(|c| f(c)),
            Geometry::MultiLineString(v) | Geometry::Polygon(v) => {
                v.iter_mut().flatten().for_each(|c| f(c))
            }
            Geometry::MultiPolygon(v) => v.iter_mut().flatten().flatten().for_each(|c| f(c)),
            Geometry::Collection(v) => v.iter_mut().for_each(|g| g.for_each_coord_mut(f)),
        }
    }

    pub fn bounds(&self) -> Bounds {
        let mut b = Bounds::empty();
        self.for_each_coord(&mut |c| b.extend(c.x, c.y));
        b
    }

    pub fn num_coords(&self) -> usize {
        let mut n = 0;
        self.for_each_coord(&mut |_| n += 1);
        n
    }

    /// Lista de puncte (pentru geometrii punctuale).
    pub fn points(&self) -> Vec<Coord> {
        match self {
            Geometry::Point(c) => vec![*c],
            Geometry::MultiPoint(v) => v.clone(),
            Geometry::Collection(g) => g.iter().flat_map(|x| x.points()).collect(),
            _ => vec![],
        }
    }

    /// Liniile (pentru geometrii liniare).
    pub fn lines(&self) -> Vec<&Vec<Coord>> {
        match self {
            Geometry::LineString(v) => vec![v],
            Geometry::MultiLineString(v) => v.iter().collect(),
            Geometry::Collection(g) => g.iter().flat_map(|x| x.lines()).collect(),
            _ => vec![],
        }
    }

    /// Poligoanele (fiecare = listă de inele).
    pub fn polygons(&self) -> Vec<&Vec<Ring>> {
        match self {
            Geometry::Polygon(p) => vec![p],
            Geometry::MultiPolygon(v) => v.iter().collect(),
            Geometry::Collection(g) => g.iter().flat_map(|x| x.polygons()).collect(),
            _ => vec![],
        }
    }

    /// Lungimea planară (unități ale sistemului).
    pub fn length(&self) -> f64 {
        let mut l = 0.0;
        for line in self.lines() {
            l += path_length(line);
        }
        for p in self.polygons() {
            for r in p {
                l += path_length(r);
            }
        }
        l
    }

    /// Aria planară (inelul exterior minus goluri).
    pub fn area(&self) -> f64 {
        let mut a = 0.0;
        for p in self.polygons() {
            for (i, r) in p.iter().enumerate() {
                let ra = ring_signed_area(r).abs();
                if i == 0 { a += ra } else { a -= ra }
            }
        }
        a
    }

    /// Punct reprezentativ (pentru etichete).
    pub fn label_point(&self) -> Option<Coord> {
        match self {
            Geometry::Point(c) => Some(*c),
            Geometry::MultiPoint(v) => v.first().copied(),
            Geometry::LineString(v) => midpoint_of_path(v),
            Geometry::MultiLineString(v) => v
                .iter()
                .max_by(|a, b| path_length(a).total_cmp(&path_length(b)))
                .and_then(|l| midpoint_of_path(l)),
            Geometry::Polygon(p) => polygon_label_point(p),
            Geometry::MultiPolygon(v) => v
                .iter()
                .max_by(|a, b| {
                    ring_signed_area(&a[0])
                        .abs()
                        .total_cmp(&ring_signed_area(&b[0]).abs())
                })
                .and_then(|p| polygon_label_point(p)),
            Geometry::Collection(g) => g.iter().find_map(|x| x.label_point()),
        }
    }

    /// Distanța minimă de la un punct la geometrie (0 dacă e în interiorul unui poligon).
    pub fn distance_to(&self, x: f64, y: f64) -> f64 {
        let mut d = f64::INFINITY;
        for c in self.points() {
            d = d.min(((c.x - x).powi(2) + (c.y - y).powi(2)).sqrt());
        }
        for l in self.lines() {
            d = d.min(dist_to_path(l, x, y));
        }
        for p in self.polygons() {
            if point_in_polygon(p, x, y) {
                return 0.0;
            }
            for r in p {
                d = d.min(dist_to_path(r, x, y));
            }
        }
        d
    }

    /// Adevărat dacă geometria atinge dreptunghiul (un vârf sau o latură în interior, ori
    /// dreptunghiul aflat în interiorul unui poligon). Folosit la selecția prin dreptunghi.
    pub fn intersects_bounds(&self, b: &Bounds) -> bool {
        let gb = self.bounds();
        if !gb.is_valid() || !gb.intersects(b) {
            return false;
        }
        if gb.minx >= b.minx && gb.maxx <= b.maxx && gb.miny >= b.miny && gb.maxy <= b.maxy {
            return true;
        }
        if self.points().iter().any(|c| b.contains(c.x, c.y)) {
            return true;
        }
        let path_hits = |path: &[Coord]| {
            path.iter().any(|c| b.contains(c.x, c.y))
                || path.windows(2).any(|w| segment_hits_bounds(&w[0], &w[1], b))
        };
        if self.lines().iter().any(|l| path_hits(l)) {
            return true;
        }
        let (cx, cy) = b.center();
        self.polygons()
            .iter()
            .any(|p| p.iter().any(|r| path_hits(r)) || point_in_polygon(p, cx, cy))
    }

    /// Simplifică într-o colecție doar dacă e nevoie.
    pub fn from_parts(mut parts: Vec<Geometry>) -> Option<Geometry> {
        match parts.len() {
            0 => None,
            1 => parts.pop(),
            _ => {
                let k = parts[0].kind();
                if parts.iter().all(|p| p.kind() == k) {
                    match k {
                        GeomKind::Point => Some(Geometry::MultiPoint(
                            parts.iter().flat_map(|p| p.points()).collect(),
                        )),
                        GeomKind::Line => Some(Geometry::MultiLineString(
                            parts
                                .iter()
                                .flat_map(|p| p.lines().into_iter().cloned())
                                .collect(),
                        )),
                        GeomKind::Polygon => Some(Geometry::MultiPolygon(
                            parts
                                .iter()
                                .flat_map(|p| p.polygons().into_iter().cloned())
                                .collect(),
                        )),
                        GeomKind::Mixed => Some(Geometry::Collection(parts)),
                    }
                } else {
                    Some(Geometry::Collection(parts))
                }
            }
        }
    }
}

pub fn path_length(v: &[Coord]) -> f64 {
    v.windows(2)
        .map(|w| ((w[1].x - w[0].x).powi(2) + (w[1].y - w[0].y).powi(2)).sqrt())
        .sum()
}

/// Arie cu semn: pozitivă = sens trigonometric (anti-orar).
pub fn ring_signed_area(r: &[Coord]) -> f64 {
    if r.len() < 3 {
        return 0.0;
    }
    let mut s = 0.0;
    let (ox, oy) = (r[0].x, r[0].y);
    for i in 0..r.len() {
        let a = r[i];
        let b = r[(i + 1) % r.len()];
        s += (a.x - ox) * (b.y - oy) - (b.x - ox) * (a.y - oy);
    }
    s * 0.5
}

/// Segmentul [a, b] intersectează dreptunghiul (decupare Liang–Barsky).
pub fn segment_hits_bounds(a: &Coord, b: &Coord, r: &Bounds) -> bool {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let (mut t0, mut t1) = (0.0f64, 1.0f64);
    for (p, q) in [
        (-dx, a.x - r.minx),
        (dx, r.maxx - a.x),
        (-dy, a.y - r.miny),
        (dy, r.maxy - a.y),
    ] {
        if p == 0.0 {
            if q < 0.0 {
                return false;
            }
        } else {
            let t = q / p;
            if p < 0.0 {
                t0 = t0.max(t);
            } else {
                t1 = t1.min(t);
            }
            if t0 > t1 {
                return false;
            }
        }
    }
    true
}

pub fn point_in_ring(r: &[Coord], x: f64, y: f64) -> bool {
    let mut inside = false;
    let n = r.len();
    if n < 3 {
        return false;
    }
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi, xj, yj) = (r[i].x, r[i].y, r[j].x, r[j].y);
        if ((yi > y) != (yj > y)) && (x < (xj - xi) * (y - yi) / (yj - yi) + xi) {
            inside = !inside;
        }
        j = i;
    }
    inside
}

pub fn point_in_polygon(p: &[Ring], x: f64, y: f64) -> bool {
    if p.is_empty() || !point_in_ring(&p[0], x, y) {
        return false;
    }
    !p[1..].iter().any(|h| point_in_ring(h, x, y))
}

fn dist_seg(ax: f64, ay: f64, bx: f64, by: f64, x: f64, y: f64) -> f64 {
    let (dx, dy) = (bx - ax, by - ay);
    let l2 = dx * dx + dy * dy;
    let t = if l2 > 0.0 {
        (((x - ax) * dx + (y - ay) * dy) / l2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let (px, py) = (ax + t * dx, ay + t * dy);
    ((x - px).powi(2) + (y - py).powi(2)).sqrt()
}

pub fn dist_to_path(v: &[Coord], x: f64, y: f64) -> f64 {
    if v.len() == 1 {
        return ((v[0].x - x).powi(2) + (v[0].y - y).powi(2)).sqrt();
    }
    v.windows(2)
        .map(|w| dist_seg(w[0].x, w[0].y, w[1].x, w[1].y, x, y))
        .fold(f64::INFINITY, f64::min)
}

fn midpoint_of_path(v: &[Coord]) -> Option<Coord> {
    if v.is_empty() {
        return None;
    }
    let half = path_length(v) / 2.0;
    let mut acc = 0.0;
    for w in v.windows(2) {
        let l = ((w[1].x - w[0].x).powi(2) + (w[1].y - w[0].y).powi(2)).sqrt();
        if acc + l >= half && l > 0.0 {
            let t = (half - acc) / l;
            return Some(Coord::new(
                w[0].x + t * (w[1].x - w[0].x),
                w[0].y + t * (w[1].y - w[0].y),
            ));
        }
        acc += l;
    }
    v.first().copied()
}

/// Punct garantat în interiorul poligonului (metoda scanării orizontale prin centrul înălțimii).
fn polygon_label_point(p: &[Ring]) -> Option<Coord> {
    let ext = p.first()?;
    if ext.len() < 3 {
        return ext.first().copied();
    }
    // centroid
    let a = ring_signed_area(ext);
    if a.abs() > 0.0 {
        let (mut cx, mut cy) = (0.0, 0.0);
        let (ox, oy) = (ext[0].x, ext[0].y);
        for i in 0..ext.len() {
            let (p0, p1) = (ext[i], ext[(i + 1) % ext.len()]);
            let (x0, y0, x1, y1) = (p0.x - ox, p0.y - oy, p1.x - ox, p1.y - oy);
            let f = x0 * y1 - x1 * y0;
            cx += (x0 + x1) * f;
            cy += (y0 + y1) * f;
        }
        let c = Coord::new(cx / (6.0 * a) + ox, cy / (6.0 * a) + oy);
        if point_in_polygon(p, c.x, c.y) {
            return Some(c);
        }
    }
    // scanare la mijlocul înălțimii
    let b = Geometry::Polygon(p.to_vec()).bounds();
    let y = (b.miny + b.maxy) / 2.0;
    let mut xs = Vec::new();
    for r in p {
        for i in 0..r.len() {
            let (a, c) = (r[i], r[(i + 1) % r.len()]);
            if (a.y > y) != (c.y > y) {
                xs.push(a.x + (y - a.y) * (c.x - a.x) / (c.y - a.y));
            }
        }
    }
    xs.sort_by(|a, b| a.total_cmp(b));
    let mut best: Option<(f64, f64)> = None;
    for pair in xs.chunks(2) {
        if pair.len() == 2 {
            let w = pair[1] - pair[0];
            if best.is_none_or(|(bw, _)| w > bw) {
                best = Some((w, (pair[0] + pair[1]) / 2.0));
            }
        }
    }
    best.map(|(_, x)| Coord::new(x, y))
        .or_else(|| ext.first().copied())
}

// ---------------------------------------------------------------------------
// Atribute
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldType {
    Integer,
    Real,
    Text,
    Boolean,
    Date,
}

impl FieldType {
    pub fn label(&self) -> &'static str {
        match self {
            FieldType::Integer => tr!("întreg"),
            FieldType::Real => tr!("real"),
            FieldType::Text => tr!("text"),
            FieldType::Boolean => tr!("boolean"),
            FieldType::Date => tr!("dată"),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Field {
    pub name: String,
    pub ftype: FieldType,
    /// Lățime (pentru DBF); 0 = automat.
    pub width: usize,
    pub precision: usize,
}

impl Field {
    pub fn new(name: impl Into<String>, ftype: FieldType) -> Self {
        Self {
            name: name.into(),
            ftype,
            width: 0,
            precision: 0,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Null,
    Int(i64),
    Real(f64),
    Text(String),
    Bool(bool),
}

impl Value {
    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Int(i) => Some(*i as f64),
            Value::Real(r) => Some(*r),
            Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
            Value::Text(s) => s.trim().replace(',', ".").parse().ok(),
            Value::Null => None,
        }
    }
    pub fn to_text(&self) -> String {
        match self {
            Value::Null => String::new(),
            Value::Int(i) => i.to_string(),
            Value::Real(r) => fmt_real(*r),
            Value::Text(s) => s.clone(),
            Value::Bool(b) => {
                if *b {
                    "true".into()
                } else {
                    "false".into()
                }
            }
        }
    }
    /// Convertește valoarea la tipul câmpului.
    pub fn coerce(&self, t: FieldType) -> Value {
        match (self, t) {
            (Value::Null, _) => Value::Null,
            (Value::Int(_), FieldType::Integer)
            | (Value::Real(_), FieldType::Real)
            | (Value::Bool(_), FieldType::Boolean) => self.clone(),
            (Value::Text(_), FieldType::Text) | (Value::Text(_), FieldType::Date) => self.clone(),
            (_, FieldType::Text) | (_, FieldType::Date) => Value::Text(self.to_text()),
            (v, FieldType::Integer) => v
                .as_f64()
                .map(|f| Value::Int(f.round() as i64))
                .unwrap_or(Value::Null),
            (v, FieldType::Real) => v.as_f64().map(Value::Real).unwrap_or(Value::Null),
            (Value::Text(s), FieldType::Boolean) => match s.trim().to_lowercase().as_str() {
                "1" | "true" | "t" | "y" | "yes" | "da" | "d" => Value::Bool(true),
                "0" | "false" | "f" | "n" | "no" | "nu" => Value::Bool(false),
                _ => Value::Null,
            },
            (v, FieldType::Boolean) => v
                .as_f64()
                .map(|f| Value::Bool(f != 0.0))
                .unwrap_or(Value::Null),
        }
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_text())
    }
}

pub fn fmt_real(r: f64) -> String {
    if r.fract() == 0.0 && r.abs() < 1e15 {
        format!("{:.1}", r)
    } else {
        let s = format!("{:.10}", r);
        let s = s.trim_end_matches('0');
        s.trim_end_matches('.').to_string()
    }
}

/// Deduce tipul unei valori text.
pub fn infer_value(s: &str) -> Value {
    let t = s.trim();
    if t.is_empty() {
        return Value::Null;
    }
    if let Ok(i) = t.parse::<i64>() {
        // păstrăm ca text codurile cu zerouri la început (ex. SIRUTA "01234")
        if !(t.len() > 1 && t.starts_with('0')) {
            return Value::Int(i);
        }
        return Value::Text(t.to_string());
    }
    if let Ok(f) = t.parse::<f64>() {
        return Value::Real(f);
    }
    Value::Text(t.to_string())
}

/// Determină tipul unei coloane pe baza valorilor.
pub fn infer_field_type<'a>(vals: impl Iterator<Item = &'a Value>) -> FieldType {
    let mut t: Option<FieldType> = None;
    for v in vals {
        let vt = match v {
            Value::Null => continue,
            Value::Int(_) => FieldType::Integer,
            Value::Real(_) => FieldType::Real,
            Value::Bool(_) => FieldType::Boolean,
            Value::Text(_) => FieldType::Text,
        };
        t = Some(match (t, vt) {
            (None, x) => x,
            (Some(a), b) if a == b => a,
            (Some(FieldType::Integer), FieldType::Real)
            | (Some(FieldType::Real), FieldType::Integer) => FieldType::Real,
            _ => FieldType::Text,
        });
        if t == Some(FieldType::Text) {
            break;
        }
    }
    t.unwrap_or(FieldType::Text)
}

#[derive(Clone, Debug, PartialEq)]
pub struct Feature {
    pub geom: Option<Geometry>,
    pub attrs: Vec<Value>,
}

#[derive(Clone, Debug)]
pub struct VectorLayer {
    pub name: String,
    pub source: Option<PathBuf>,
    pub fields: Vec<Field>,
    pub features: Vec<Feature>,
    pub crs: Option<Crs>,
    pub has_z: bool,
}

impl VectorLayer {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            source: None,
            fields: vec![],
            features: vec![],
            crs: None,
            has_z: false,
        }
    }

    pub fn bounds(&self) -> Bounds {
        let mut b = Bounds::empty();
        for f in &self.features {
            if let Some(g) = &f.geom {
                b.merge(&g.bounds());
            }
        }
        b
    }

    pub fn field_index(&self, name: &str) -> Option<usize> {
        self.fields.iter().position(|f| f.name == name).or_else(|| {
            self.fields
                .iter()
                .position(|f| f.name.eq_ignore_ascii_case(name))
        })
    }

    /// Tipul dominant de geometrie.
    pub fn geom_kind(&self) -> GeomKind {
        let mut k: Option<GeomKind> = None;
        for f in &self.features {
            if let Some(g) = &f.geom {
                let gk = g.kind();
                match k {
                    None => k = Some(gk),
                    Some(p) if p != gk => return GeomKind::Mixed,
                    _ => {}
                }
            }
        }
        k.unwrap_or(GeomKind::Mixed)
    }

    /// Adaugă un câmp și extinde atributele existente cu NULL.
    pub fn add_field(&mut self, f: Field) -> usize {
        self.fields.push(f);
        for ft in &mut self.features {
            ft.attrs.resize(self.fields.len(), Value::Null);
        }
        self.fields.len() - 1
    }

    /// Construiește un strat din înregistrări cu atribute numite (deduce schema).
    pub fn from_named_records(
        name: &str,
        recs: Vec<(Option<Geometry>, Vec<(String, Value)>)>,
    ) -> Self {
        let mut layer = VectorLayer::new(name);
        let mut names: Vec<String> = Vec::new();
        let mut idx = std::collections::HashMap::new();
        for (_, attrs) in &recs {
            for (k, _) in attrs {
                if !idx.contains_key(k) {
                    idx.insert(k.clone(), names.len());
                    names.push(k.clone());
                }
            }
        }
        let mut rows: Vec<Feature> = Vec::with_capacity(recs.len());
        for (g, attrs) in recs {
            let mut vals = vec![Value::Null; names.len()];
            for (k, v) in attrs {
                vals[idx[&k]] = v;
            }
            rows.push(Feature {
                geom: g,
                attrs: vals,
            });
        }
        for (i, n) in names.iter().enumerate() {
            let t = infer_field_type(rows.iter().map(|r| &r.attrs[i]));
            layer.fields.push(Field::new(n.clone(), t));
        }
        for r in &mut rows {
            for (i, f) in layer.fields.iter().enumerate() {
                r.attrs[i] = r.attrs[i].coerce(f.ftype);
            }
        }
        layer.features = rows;
        layer.has_z = layer.features.iter().any(|f| {
            let mut z = false;
            if let Some(g) = &f.geom {
                g.for_each_coord(&mut |c| z |= c.z != 0.0);
            }
            z
        });
        layer
    }
}

// ---------------------------------------------------------------------------
// Raster
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub enum BandData {
    U8(Vec<u8>),
    I8(Vec<i8>),
    U16(Vec<u16>),
    I16(Vec<i16>),
    U32(Vec<u32>),
    I32(Vec<i32>),
    F32(Vec<f32>),
    F64(Vec<f64>),
}

impl BandData {
    pub fn len(&self) -> usize {
        match self {
            BandData::U8(v) => v.len(),
            BandData::I8(v) => v.len(),
            BandData::U16(v) => v.len(),
            BandData::I16(v) => v.len(),
            BandData::U32(v) => v.len(),
            BandData::I32(v) => v.len(),
            BandData::F32(v) => v.len(),
            BandData::F64(v) => v.len(),
        }
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    #[inline]
    pub fn get(&self, i: usize) -> f64 {
        match self {
            BandData::U8(v) => v[i] as f64,
            BandData::I8(v) => v[i] as f64,
            BandData::U16(v) => v[i] as f64,
            BandData::I16(v) => v[i] as f64,
            BandData::U32(v) => v[i] as f64,
            BandData::I32(v) => v[i] as f64,
            BandData::F32(v) => v[i] as f64,
            BandData::F64(v) => v[i],
        }
    }
    #[inline]
    pub fn set(&mut self, i: usize, x: f64) {
        match self {
            BandData::U8(v) => v[i] = x.round().clamp(0.0, 255.0) as u8,
            BandData::I8(v) => v[i] = x.round().clamp(-128.0, 127.0) as i8,
            BandData::U16(v) => v[i] = x.round().clamp(0.0, 65535.0) as u16,
            BandData::I16(v) => v[i] = x.round().clamp(-32768.0, 32767.0) as i16,
            BandData::U32(v) => v[i] = x.round().clamp(0.0, u32::MAX as f64) as u32,
            BandData::I32(v) => v[i] = x.round().clamp(i32::MIN as f64, i32::MAX as f64) as i32,
            BandData::F32(v) => v[i] = x as f32,
            BandData::F64(v) => v[i] = x,
        }
    }
    /// Bandă nouă de același tip, umplută cu `fill`.
    pub fn new_like(&self, n: usize, fill: f64) -> BandData {
        let mut b = match self {
            BandData::U8(_) => BandData::U8(vec![0; n]),
            BandData::I8(_) => BandData::I8(vec![0; n]),
            BandData::U16(_) => BandData::U16(vec![0; n]),
            BandData::I16(_) => BandData::I16(vec![0; n]),
            BandData::U32(_) => BandData::U32(vec![0; n]),
            BandData::I32(_) => BandData::I32(vec![0; n]),
            BandData::F32(_) => BandData::F32(vec![0.0; n]),
            BandData::F64(_) => BandData::F64(vec![0.0; n]),
        };
        if fill != 0.0 {
            for i in 0..n {
                b.set(i, fill);
            }
        }
        b
    }
    pub fn type_name(&self) -> &'static str {
        match self {
            BandData::U8(_) => "Byte",
            BandData::I8(_) => "Int8",
            BandData::U16(_) => "UInt16",
            BandData::I16(_) => "Int16",
            BandData::U32(_) => "UInt32",
            BandData::I32(_) => "Int32",
            BandData::F32(_) => "Float32",
            BandData::F64(_) => "Float64",
        }
    }
    pub fn is_float(&self) -> bool {
        matches!(self, BandData::F32(_) | BandData::F64(_))
    }
}

#[derive(Clone, Debug, Default)]
pub struct BandStats {
    pub min: f64,
    pub max: f64,
    pub mean: f64,
    pub stddev: f64,
    /// percentile 2% și 98% (pentru întinderea contrastului)
    pub p2: f64,
    pub p98: f64,
    pub valid: usize,
}

#[derive(Clone, Debug)]
pub struct Raster {
    pub name: String,
    pub source: Option<PathBuf>,
    pub width: usize,
    pub height: usize,
    pub bands: Vec<BandData>,
    /// Geotransformare tip GDAL: x = gt0 + col*gt1 + row*gt2 ; y = gt3 + col*gt4 + row*gt5
    pub gt: [f64; 6],
    pub nodata: Option<f64>,
    pub crs: Option<Crs>,
}

impl Raster {
    pub fn pixel_to_world(&self, col: f64, row: f64) -> (f64, f64) {
        let g = &self.gt;
        (
            g[0] + col * g[1] + row * g[2],
            g[3] + col * g[4] + row * g[5],
        )
    }

    /// Inversa geotransformării: (x, y) -> (col, row).
    pub fn inv_gt(&self) -> Option<[f64; 6]> {
        invert_gt(&self.gt)
    }

    pub fn bounds(&self) -> Bounds {
        let mut b = Bounds::empty();
        for (c, r) in [
            (0.0, 0.0),
            (self.width as f64, 0.0),
            (0.0, self.height as f64),
            (self.width as f64, self.height as f64),
        ] {
            let (x, y) = self.pixel_to_world(c, r);
            b.extend(x, y);
        }
        b
    }

    pub fn pixel_size(&self) -> (f64, f64) {
        (
            (self.gt[1].powi(2) + self.gt[4].powi(2)).sqrt(),
            (self.gt[2].powi(2) + self.gt[5].powi(2)).sqrt(),
        )
    }

    pub fn is_nodata(&self, v: f64) -> bool {
        match self.nodata {
            Some(nd) => v == nd || (nd.is_nan() && v.is_nan()),
            None => v.is_nan(),
        }
    }

    pub fn stats(&self, band: usize) -> BandStats {
        let b = &self.bands[band];
        let n = b.len();
        let step = (n / 2_000_000).max(1);
        let mut vals: Vec<f64> = Vec::with_capacity(n / step + 1);
        let mut i = 0;
        while i < n {
            let v = b.get(i);
            if !self.is_nodata(v) && v.is_finite() {
                vals.push(v);
            }
            i += step;
        }
        if vals.is_empty() {
            return BandStats::default();
        }
        let sum: f64 = vals.iter().sum();
        let mean = sum / vals.len() as f64;
        let var = vals.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / vals.len() as f64;
        vals.sort_by(|a, b| a.total_cmp(b));
        let pct = |p: f64| vals[((vals.len() - 1) as f64 * p).round() as usize];
        BandStats {
            min: vals[0],
            max: vals[vals.len() - 1],
            mean,
            stddev: var.sqrt(),
            p2: pct(0.02),
            p98: pct(0.98),
            valid: vals.len() * step,
        }
    }
}

pub fn invert_gt(g: &[f64; 6]) -> Option<[f64; 6]> {
    let det = g[1] * g[5] - g[2] * g[4];
    if det.abs() < 1e-300 {
        return None;
    }
    let inv = 1.0 / det;
    let i1 = g[5] * inv;
    let i2 = -g[2] * inv;
    let i4 = -g[4] * inv;
    let i5 = g[1] * inv;
    let i0 = -(g[0] * i1 + g[3] * i2);
    let i3 = -(g[0] * i4 + g[3] * i5);
    Some([i0, i1, i2, i3, i4, i5])
}

/// Un set de date încărcat dintr-un fișier.
#[derive(Clone, Debug)]
pub enum Dataset {
    Vector(VectorLayer),
    Raster(Raster),
}

impl Dataset {
    pub fn name(&self) -> &str {
        match self {
            Dataset::Vector(v) => &v.name,
            Dataset::Raster(r) => &r.name,
        }
    }
    pub fn crs(&self) -> Option<&Crs> {
        match self {
            Dataset::Vector(v) => v.crs.as_ref(),
            Dataset::Raster(r) => r.crs.as_ref(),
        }
    }
    pub fn bounds(&self) -> Bounds {
        match self {
            Dataset::Vector(v) => v.bounds(),
            Dataset::Raster(r) => r.bounds(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sq(x0: f64, y0: f64, s: f64) -> Geometry {
        Geometry::Polygon(vec![vec![
            Coord::new(x0, y0),
            Coord::new(x0 + s, y0),
            Coord::new(x0 + s, y0 + s),
            Coord::new(x0, y0 + s),
            Coord::new(x0, y0),
        ]])
    }

    #[test]
    fn rectangle_intersection() {
        let b = Bounds::new(10.0, 10.0, 20.0, 20.0);
        // poligon mare care conține dreptunghiul (niciun vârf înăuntru)
        assert!(sq(0.0, 0.0, 100.0).intersects_bounds(&b));
        // poligon complet în dreptunghi
        assert!(sq(12.0, 12.0, 2.0).intersects_bounds(&b));
        // poligon alături, cu încadrarea suprapusă doar pe o axă
        assert!(!sq(25.0, 12.0, 5.0).intersects_bounds(&b));
        // linie care traversează dreptunghiul fără vârfuri în el
        let l = Geometry::LineString(vec![Coord::new(0.0, 15.0), Coord::new(30.0, 15.0)]);
        assert!(l.intersects_bounds(&b));
        // linie diagonală pe lângă colț
        let l2 = Geometry::LineString(vec![Coord::new(0.0, 25.0), Coord::new(15.0, 40.0)]);
        assert!(!l2.intersects_bounds(&b));
        // puncte
        assert!(Geometry::MultiPoint(vec![Coord::new(0.0, 0.0), Coord::new(15.0, 15.0)]).intersects_bounds(&b));
        assert!(!Geometry::Point(Coord::new(9.0, 15.0)).intersects_bounds(&b));
        // poligon cu gol în care se află dreptunghiul: nu îl atinge
        let mut donut = sq(0.0, 0.0, 100.0);
        if let Geometry::Polygon(r) = &mut donut {
            r.push(vec![
                Coord::new(5.0, 5.0),
                Coord::new(5.0, 25.0),
                Coord::new(25.0, 25.0),
                Coord::new(25.0, 5.0),
                Coord::new(5.0, 5.0),
            ]);
        }
        assert!(!donut.intersects_bounds(&b));
    }
}
