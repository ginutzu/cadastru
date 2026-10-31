//! AutoCAD DXF (ASCII) – citire (R12…2018) și scriere (R12/AC1009).

use super::{ReadOptions, WriteOptions, encoding_from_label, read_prj_sidecar, write_prj_sidecar};
use crate::model::*;
use anyhow::{Context, Result, bail};
use encoding_rs::Encoding;
use std::collections::HashMap;
use std::f64::consts::PI;
use std::fs;
use std::path::Path;
use crate::i18n::tr;

type Pairs = Vec<(i32, String)>;

#[derive(Clone, Debug)]
struct Ent {
    kind: String,
    pairs: Pairs,
    subs: Vec<Ent>, // VERTEX / ATTRIB
}

impl Ent {
    fn s(&self, code: i32) -> Option<&str> {
        self.pairs
            .iter()
            .find(|(c, _)| *c == code)
            .map(|(_, v)| v.as_str())
    }
    fn f(&self, code: i32) -> Option<f64> {
        self.s(code).and_then(|v| v.trim().parse().ok())
    }
    fn fd(&self, code: i32, d: f64) -> f64 {
        self.f(code).unwrap_or(d)
    }
    fn i(&self, code: i32) -> i64 {
        self.s(code)
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(0)
    }
    fn pt(&self, base: i32) -> Coord {
        Coord::xyz(
            self.fd(base, 0.0),
            self.fd(base + 10, 0.0),
            self.fd(base + 20, 0.0),
        )
    }
    /// Toate valorile pentru un cod (în ordine).
    fn all(&self, code: i32) -> Vec<f64> {
        self.pairs
            .iter()
            .filter(|(c, _)| *c == code)
            .filter_map(|(_, v)| v.trim().parse().ok())
            .collect()
    }
    /// OCS: extruziune negativă -> oglindire pe X
    fn mirrored(&self) -> bool {
        self.f(230).is_some_and(|z| z < 0.0)
    }
}

fn decode_unicode_escapes(s: &str) -> String {
    if !s.contains("\\U+") && !s.contains("\\u+") {
        return s.to_string();
    }
    let mut out = String::new();
    let mut it = s.char_indices().peekable();
    let b = s.as_bytes();
    while let Some((i, c)) = it.next() {
        if c == '\\'
            && i + 7 <= s.len()
            && (b[i + 1] == b'U' || b[i + 1] == b'u')
            && b[i + 2] == b'+'
        {
            if let Ok(cp) = u32::from_str_radix(&s[i + 3..i + 7], 16)
                && let Some(ch) = char::from_u32(cp)
            {
                out.push(ch);
                for _ in 0..6 {
                    it.next();
                }
                continue;
            }
        }
        out.push(c);
    }
    out
}

fn clean_mtext(s: &str) -> String {
    let s = s.replace("\\P", "\n").replace("\\~", " ");
    let mut out = String::new();
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '{' || c == '}' {
            i += 1;
            continue;
        }
        if c == '\\' && i + 1 < chars.len() {
            let n = chars[i + 1];
            match n {
                '\\' | '{' | '}' => {
                    out.push(n);
                    i += 2;
                }
                'L' | 'l' | 'O' | 'o' | 'K' | 'k' => i += 2,
                _ => {
                    // \fArial|b0;  \H2.5;  \C1; etc. – sărim până la ';'
                    match chars[i..].iter().position(|&x| x == ';') {
                        Some(p) => i += p + 1,
                        None => i += 2,
                    }
                }
            }
            continue;
        }
        out.push(c);
        i += 1;
    }
    out.trim().to_string()
}

fn parse_pairs(text: &str) -> Pairs {
    let mut lines = text.lines();
    let mut v = Vec::new();
    while let (Some(c), Some(val)) = (lines.next(), lines.next()) {
        if let Ok(code) = c.trim().parse::<i32>() {
            v.push((code, val.trim_end_matches('\r').to_string()));
        }
    }
    v
}

fn header_var(pairs: &Pairs, name: &str) -> Option<String> {
    let i = pairs
        .iter()
        .position(|(c, v)| *c == 9 && v.trim() == name)?;
    pairs.get(i + 1).map(|(_, v)| v.trim().to_string())
}

/// Grupează perechile unei secțiuni în entități.
fn group_entities(pairs: &[(i32, String)]) -> Vec<Ent> {
    let mut raw: Vec<Ent> = vec![];
    for (c, v) in pairs {
        if *c == 0 {
            raw.push(Ent {
                kind: v.trim().to_uppercase(),
                pairs: vec![],
                subs: vec![],
            });
        } else if let Some(e) = raw.last_mut() {
            e.pairs.push((*c, v.clone()));
        }
    }
    // atașează VERTEX/ATTRIB la POLYLINE/INSERT
    let mut out: Vec<Ent> = vec![];
    let mut open = false;
    for e in raw {
        match e.kind.as_str() {
            "VERTEX" | "ATTRIB" if open => out.last_mut().unwrap().subs.push(e),
            "SEQEND" => open = false,
            _ => {
                open = e.kind == "POLYLINE" || (e.kind == "INSERT" && e.i(66) == 1);
                out.push(e);
            }
        }
    }
    out
}

struct Block {
    base: Coord,
    ents: Vec<Ent>,
}

#[derive(Clone, Copy)]
struct Xf {
    // p' = o + R*S*(p - base)
    ox: f64,
    oy: f64,
    oz: f64,
    sx: f64,
    sy: f64,
    sz: f64,
    rot: f64,
    bx: f64,
    by: f64,
    bz: f64,
}

impl Xf {
    fn apply(&self, p: Coord) -> Coord {
        let (x, y) = ((p.x - self.bx) * self.sx, (p.y - self.by) * self.sy);
        let (c, s) = (self.rot.cos(), self.rot.sin());
        Coord::xyz(
            self.ox + x * c - y * s,
            self.oy + x * s + y * c,
            self.oz + (p.z - self.bz) * self.sz,
        )
    }
}

fn arc_points(cx: f64, cy: f64, z: f64, r: f64, a0: f64, a1: f64) -> Vec<Coord> {
    let mut sweep = a1 - a0;
    while sweep <= 0.0 {
        sweep += 2.0 * PI;
    }
    let n = ((sweep / (2.0 * PI) * 72.0).ceil() as usize).max(4);
    (0..=n)
        .map(|i| {
            let a = a0 + sweep * i as f64 / n as f64;
            Coord::xyz(cx + r * a.cos(), cy + r * a.sin(), z)
        })
        .collect()
}

/// Puncte intermediare pentru un segment cu „bulge”.
fn bulge_points(p0: Coord, p1: Coord, bulge: f64, out: &mut Vec<Coord>) {
    if bulge.abs() < 1e-9 {
        return;
    }
    let (dx, dy) = (p1.x - p0.x, p1.y - p0.y);
    let chord = (dx * dx + dy * dy).sqrt();
    if chord == 0.0 {
        return;
    }
    let theta = 4.0 * bulge.atan();
    let r = chord / (2.0 * (theta / 2.0).sin());
    let (mx, my) = ((p0.x + p1.x) / 2.0, (p0.y + p1.y) / 2.0);
    let h = r * (theta / 2.0).cos();
    // normala la stânga coardei
    let (nx, ny) = (-dy / chord, dx / chord);
    let (cx, cy) = (mx - nx * h, my - ny * h);
    let a0 = (p0.y - cy).atan2(p0.x - cx);
    let n = ((theta.abs() / (2.0 * PI) * 72.0).ceil() as usize).max(2);
    for i in 1..n {
        let a = a0 + theta * i as f64 / n as f64;
        let rr = r.abs();
        out.push(Coord::xyz(cx + rr * a.cos(), cy + rr * a.sin(), p0.z));
    }
}

fn poly_with_bulges(verts: &[(Coord, f64)], closed: bool) -> Vec<Coord> {
    let mut out = vec![];
    let n = verts.len();
    for i in 0..n {
        out.push(verts[i].0);
        if i + 1 < n {
            bulge_points(verts[i].0, verts[i + 1].0, verts[i].1, &mut out);
        } else if closed && n > 1 {
            bulge_points(verts[i].0, verts[0].0, verts[i].1, &mut out);
        }
    }
    if closed && n > 2 && out.first() != out.last() {
        out.push(out[0]);
    }
    out
}

fn bspline(ctrl: &[Coord], knots: &[f64], weights: &[f64], p: usize) -> Vec<Coord> {
    let n = ctrl.len();
    if n < 2 || knots.len() < n + p + 1 || p == 0 {
        return ctrl.to_vec();
    }
    let w = |i: usize| weights.get(i).copied().unwrap_or(1.0);
    let (t0, t1) = (knots[p], knots[n]);
    let samples = (n * 10).clamp(16, 1000);
    let mut out = Vec::with_capacity(samples + 1);
    for s in 0..=samples {
        let t = t0 + (t1 - t0) * s as f64 / samples as f64;
        // găsește intervalul
        let mut k = p;
        while k < n - 1 && t >= knots[k + 1] {
            k += 1;
        }
        // de Boor omogen
        let mut d: Vec<(f64, f64, f64, f64)> = (0..=p)
            .map(|j| {
                let idx = j + k - p;
                let c = ctrl[idx];
                let ww = w(idx);
                (c.x * ww, c.y * ww, c.z * ww, ww)
            })
            .collect();
        for r in 1..=p {
            for j in (r..=p).rev() {
                let i = j + k - p;
                let den = knots[i + p + 1 - r] - knots[i];
                let a = if den.abs() < 1e-12 {
                    0.0
                } else {
                    (t - knots[i]) / den
                };
                d[j] = (
                    (1.0 - a) * d[j - 1].0 + a * d[j].0,
                    (1.0 - a) * d[j - 1].1 + a * d[j].1,
                    (1.0 - a) * d[j - 1].2 + a * d[j].2,
                    (1.0 - a) * d[j - 1].3 + a * d[j].3,
                );
            }
        }
        let (x, y, z, ww) = d[p];
        if ww != 0.0 {
            out.push(Coord::xyz(x / ww, y / ww, z / ww));
        }
    }
    out
}

/// Inelele de contur ale unei entități HATCH.
fn hatch_rings(e: &Ent) -> Vec<Vec<Coord>> {
    let p = &e.pairs;
    let elev = e.fd(30, 0.0);
    let mut i = match p.iter().position(|(c, _)| *c == 91) {
        Some(i) => i + 1,
        None => return vec![],
    };
    let npaths: usize = p[i - 1].1.trim().parse().unwrap_or(0);
    let num = |k: usize| -> f64 {
        p.get(k)
            .and_then(|(_, v)| v.trim().parse().ok())
            .unwrap_or(0.0)
    };
    // avansează până la următorul cod dat
    let seek = |i: &mut usize, code: i32| -> bool {
        while *i < p.len() {
            if p[*i].0 == code {
                return true;
            }
            *i += 1;
        }
        false
    };
    let mut rings = vec![];
    for _ in 0..npaths {
        if !seek(&mut i, 92) {
            break;
        }
        let flags = num(i) as i64;
        i += 1;
        let mut ring: Vec<Coord> = vec![];
        if flags & 2 != 0 {
            // polilinie
            let mut has_bulge = false;
            let mut closed = true;
            let mut n = 0usize;
            while i < p.len() && p[i].0 != 10 {
                match p[i].0 {
                    72 => has_bulge = num(i) != 0.0,
                    73 => closed = num(i) != 0.0,
                    93 => n = num(i) as usize,
                    _ => {}
                }
                i += 1;
            }
            let mut verts: Vec<(Coord, f64)> = vec![];
            for _ in 0..n {
                if !seek(&mut i, 10) {
                    break;
                }
                let x = num(i);
                let y = if p.get(i + 1).is_some_and(|q| q.0 == 20) {
                    num(i + 1)
                } else {
                    0.0
                };
                i += 2;
                let mut b = 0.0;
                if has_bulge && p.get(i).is_some_and(|q| q.0 == 42) {
                    b = num(i);
                    i += 1;
                }
                verts.push((Coord::xyz(x, y, elev), b));
            }
            let _ = closed;
            ring = poly_with_bulges(&verts, true);
        } else {
            if !seek(&mut i, 93) {
                break;
            }
            let nedges = num(i) as usize;
            i += 1;
            for _ in 0..nedges {
                if !seek(&mut i, 72) {
                    break;
                }
                let et = num(i) as i64;
                i += 1;
                // colectează valorile muchiei până la următorul 72 / 97 / 92
                let st = i;
                while i < p.len() && !matches!(p[i].0, 72 | 97 | 92) {
                    i += 1;
                }
                let seg = &p[st..i];
                let g = |code: i32| {
                    seg.iter()
                        .find(|(c, _)| *c == code)
                        .and_then(|(_, v)| v.trim().parse::<f64>().ok())
                        .unwrap_or(0.0)
                };
                match et {
                    1 => {
                        ring.push(Coord::xyz(g(10), g(20), elev));
                        ring.push(Coord::xyz(g(11), g(21), elev));
                    }
                    2 => {
                        let ccw = seg
                            .iter()
                            .find(|(c, _)| *c == 73)
                            .map(|(_, v)| v.trim() != "0")
                            .unwrap_or(true);
                        let (a0, a1) = (g(50).to_radians(), g(51).to_radians());
                        let pts = if ccw {
                            arc_points(g(10), g(20), elev, g(40), a0, a1)
                        } else {
                            // sens orar: unghiurile sunt negate; parcurgem de la -a0 la -a1 descrescător
                            let mut v = arc_points(g(10), g(20), elev, g(40), -a1, -a0);
                            v.reverse();
                            v
                        };
                        ring.extend(pts);
                    }
                    3 => {
                        let (cx, cy, mx, my, ratio) = (g(10), g(20), g(11), g(21), g(40));
                        let (t0, mut t1) = (g(50).to_radians(), g(51).to_radians());
                        if t1 <= t0 {
                            t1 += 2.0 * PI;
                        }
                        let a = (mx * mx + my * my).sqrt();
                        let rot = my.atan2(mx);
                        for k in 0..=48 {
                            let t = t0 + (t1 - t0) * k as f64 / 48.0;
                            let (x, y) = (a * t.cos(), a * ratio * t.sin());
                            ring.push(Coord::xyz(
                                cx + x * rot.cos() - y * rot.sin(),
                                cy + x * rot.sin() + y * rot.cos(),
                                elev,
                            ));
                        }
                    }
                    4 => {
                        let deg = seg
                            .iter()
                            .find(|(c, _)| *c == 94)
                            .and_then(|(_, v)| v.trim().parse::<usize>().ok())
                            .unwrap_or(3);
                        let vals = |code: i32| {
                            seg.iter()
                                .filter(|(c, _)| *c == code)
                                .filter_map(|(_, v)| v.trim().parse::<f64>().ok())
                                .collect::<Vec<_>>()
                        };
                        let ctrl: Vec<Coord> = vals(10)
                            .into_iter()
                            .zip(vals(20))
                            .map(|(x, y)| Coord::xyz(x, y, elev))
                            .collect();
                        ring.extend(bspline(&ctrl, &vals(40), &vals(42), deg));
                    }
                    _ => {}
                }
            }
        }
        ring.dedup();
        if ring.len() >= 3 {
            if ring.first() != ring.last() {
                ring.push(ring[0]);
            }
            rings.push(ring);
        }
    }
    rings
}

/// După asamblare (toate inelele în sens orar devin exterioare), mută inelele conținute în alte inele ca goluri.
fn nest_holes(g: Geometry) -> Geometry {
    let polys: Vec<Vec<Ring>> = g.polygons().into_iter().cloned().collect();
    if polys.len() < 2 {
        return g;
    }
    let mut exts: Vec<Ring> = polys.into_iter().flat_map(|p| p.into_iter()).collect();
    exts.sort_by(|a, b| {
        ring_signed_area(b)
            .abs()
            .total_cmp(&ring_signed_area(a).abs())
    });
    let mut out: Vec<Vec<Ring>> = vec![];
    // nivel de imbricare: par = exterior, impar = gol
    for r in exts {
        let t = r[0];
        let containers: Vec<usize> = out
            .iter()
            .enumerate()
            .filter(|(_, p)| point_in_ring(&p[0], t.x, t.y))
            .map(|(i, _)| i)
            .collect();
        let depth = out
            .iter()
            .filter(|p| point_in_ring(&p[0], t.x, t.y))
            .count()
            + out
                .iter()
                .flat_map(|p| p.iter().skip(1))
                .filter(|h| point_in_ring(h, t.x, t.y))
                .count();
        if depth % 2 == 1 {
            if let Some(&ci) = containers.last() {
                out[ci].push(r);
                continue;
            }
        }
        out.push(vec![r]);
    }
    if out.len() == 1 {
        Geometry::Polygon(out.pop().unwrap())
    } else {
        Geometry::MultiPolygon(out)
    }
}

struct Ctx<'a> {
    blocks: &'a HashMap<String, Block>,
    closed_as_lines: bool,
    recs: Vec<(Option<Geometry>, Vec<(String, Value)>)>,
}

impl<'a> Ctx<'a> {
    fn closed(&self, ring: Vec<Coord>) -> Geometry {
        if self.closed_as_lines || ring.len() < 4 {
            Geometry::LineString(ring)
        } else {
            Geometry::Polygon(vec![ring])
        }
    }

    fn emit(
        &mut self,
        e: &Ent,
        g: Geometry,
        tf: &dyn Fn(Coord) -> Coord,
        block: Option<&str>,
        extra: Vec<(String, Value)>,
    ) {
        let mut g = g;
        let mir = e.mirrored();
        g.for_each_coord_mut(&mut |c| {
            if mir {
                c.x = -c.x;
            }
            *c = tf(*c);
        });
        let mut attrs = vec![
            (
                "Layer".to_string(),
                Value::Text(e.s(8).unwrap_or("0").to_string()),
            ),
            ("EntityType".to_string(), Value::Text(e.kind.clone())),
            (
                "Color".to_string(),
                e.s(62)
                    .map(|c| Value::Int(c.trim().parse().unwrap_or(256)))
                    .unwrap_or(Value::Int(256)),
            ),
            (
                "Linetype".to_string(),
                e.s(6)
                    .map(|s| Value::Text(s.to_string()))
                    .unwrap_or(Value::Null),
            ),
            (
                "Block".to_string(),
                block
                    .map(|b| Value::Text(b.to_string()))
                    .unwrap_or(Value::Null),
            ),
        ];
        attrs.extend(extra);
        self.recs.push((Some(g), attrs));
    }

    fn entity(&mut self, e: &Ent, tf: &dyn Fn(Coord) -> Coord, block: Option<&str>, depth: usize) {
        match e.kind.as_str() {
            "POINT" => self.emit(e, Geometry::Point(e.pt(10)), tf, block, vec![]),
            "LINE" => self.emit(
                e,
                Geometry::LineString(vec![e.pt(10), e.pt(11)]),
                tf,
                block,
                vec![],
            ),
            "LWPOLYLINE" => {
                let elev = e.fd(38, 0.0);
                let mut verts: Vec<(Coord, f64)> = vec![];
                let mut cur: Option<Coord> = None;
                for (c, v) in &e.pairs {
                    let val: f64 = v.trim().parse().unwrap_or(0.0);
                    match c {
                        10 => {
                            if let Some(p) = cur.take() {
                                verts.push((p, 0.0));
                            }
                            cur = Some(Coord::xyz(val, 0.0, elev));
                        }
                        20 => {
                            if let Some(p) = cur.as_mut() {
                                p.y = val
                            }
                        }
                        42 => {
                            if let Some(p) = cur.take() {
                                verts.push((p, val));
                            }
                        }
                        _ => {}
                    }
                }
                if let Some(p) = cur.take() {
                    verts.push((p, 0.0));
                }
                // dacă bulge vine după 42 pentru un vârf deja închis, ordinea e păstrată
                let closed = e.i(70) & 1 == 1;
                let pts = poly_with_bulges(&verts, closed);
                let g = if closed {
                    self.closed(pts)
                } else {
                    Geometry::LineString(pts)
                };
                self.emit(e, g, tf, block, vec![]);
            }
            "POLYLINE" => {
                let flags = e.i(70);
                if flags & 64 != 0 {
                    // polyface mesh
                    let verts: Vec<Coord> = e
                        .subs
                        .iter()
                        .filter(|v| v.i(70) & 64 != 0)
                        .map(|v| v.pt(10))
                        .collect();
                    let mut polys = vec![];
                    for f in e
                        .subs
                        .iter()
                        .filter(|v| v.i(70) & 128 != 0 && v.i(70) & 64 == 0)
                    {
                        let mut ring: Vec<Coord> = [71, 72, 73, 74]
                            .iter()
                            .map(|c| f.i(*c).unsigned_abs() as usize)
                            .filter(|&i| i > 0 && i <= verts.len())
                            .map(|i| verts[i - 1])
                            .collect();
                        if ring.len() >= 3 {
                            ring.push(ring[0]);
                            polys.push(Geometry::Polygon(vec![ring]));
                        }
                    }
                    if let Some(g) = Geometry::from_parts(polys) {
                        self.emit(e, g, tf, block, vec![]);
                    }
                    return;
                }
                let elev = e.fd(30, 0.0);
                let is3d = flags & 8 != 0;
                let verts: Vec<(Coord, f64)> = e
                    .subs
                    .iter()
                    .filter(|v| v.kind == "VERTEX" && v.i(70) & 16 == 0)
                    .map(|v| {
                        let mut p = v.pt(10);
                        if !is3d {
                            p.z = elev;
                        }
                        (p, v.fd(42, 0.0))
                    })
                    .collect();
                let closed = flags & 1 == 1;
                let pts = poly_with_bulges(&verts, closed);
                let g = if closed {
                    self.closed(pts)
                } else {
                    Geometry::LineString(pts)
                };
                self.emit(e, g, tf, block, vec![]);
            }
            "CIRCLE" => {
                let c = e.pt(10);
                let pts = arc_points(c.x, c.y, c.z, e.fd(40, 0.0), 0.0, 2.0 * PI);
                let g = self.closed(pts);
                self.emit(
                    e,
                    g,
                    tf,
                    block,
                    vec![("Radius".into(), Value::Real(e.fd(40, 0.0)))],
                );
            }
            "ARC" => {
                let c = e.pt(10);
                let pts = arc_points(
                    c.x,
                    c.y,
                    c.z,
                    e.fd(40, 0.0),
                    e.fd(50, 0.0).to_radians(),
                    e.fd(51, 360.0).to_radians(),
                );
                self.emit(
                    e,
                    Geometry::LineString(pts),
                    tf,
                    block,
                    vec![("Radius".into(), Value::Real(e.fd(40, 0.0)))],
                );
            }
            "ELLIPSE" => {
                let c = e.pt(10);
                let (mx, my) = (e.fd(11, 1.0), e.fd(21, 0.0));
                let ratio = e.fd(40, 1.0);
                let (t0, mut t1) = (e.fd(41, 0.0), e.fd(42, 2.0 * PI));
                if t1 <= t0 {
                    t1 += 2.0 * PI;
                }
                let a = (mx * mx + my * my).sqrt();
                let rot = my.atan2(mx);
                let n = 72;
                let pts: Vec<Coord> = (0..=n)
                    .map(|i| {
                        let t = t0 + (t1 - t0) * i as f64 / n as f64;
                        let (x, y) = (a * t.cos(), a * ratio * t.sin());
                        Coord::xyz(
                            c.x + x * rot.cos() - y * rot.sin(),
                            c.y + x * rot.sin() + y * rot.cos(),
                            c.z,
                        )
                    })
                    .collect();
                let full = (t1 - t0 - 2.0 * PI).abs() < 1e-6;
                let g = if full {
                    self.closed(pts)
                } else {
                    Geometry::LineString(pts)
                };
                self.emit(e, g, tf, block, vec![]);
            }
            "SPLINE" => {
                let fx = e.all(11);
                let fy = e.all(21);
                let pts = if fx.len() >= 2 && fx.len() == fy.len() {
                    let fz = e.all(31);
                    fx.iter()
                        .zip(&fy)
                        .enumerate()
                        .map(|(i, (x, y))| Coord::xyz(*x, *y, fz.get(i).copied().unwrap_or(0.0)))
                        .collect()
                } else {
                    let (cx, cy, cz) = (e.all(10), e.all(20), e.all(30));
                    let ctrl: Vec<Coord> = cx
                        .iter()
                        .zip(&cy)
                        .enumerate()
                        .map(|(i, (x, y))| Coord::xyz(*x, *y, cz.get(i).copied().unwrap_or(0.0)))
                        .collect();
                    bspline(&ctrl, &e.all(40), &e.all(41), e.i(71).max(1) as usize)
                };
                let g = if e.i(70) & 1 == 1 && pts.len() > 2 {
                    let mut p = pts;
                    if p.first() != p.last() {
                        p.push(p[0]);
                    }
                    self.closed(p)
                } else {
                    Geometry::LineString(pts)
                };
                self.emit(e, g, tf, block, vec![]);
            }
            "HATCH" => {
                let rings = hatch_rings(e);
                if let Some(g) = crate::formats::shp::assemble_rings(
                    rings
                        .into_iter()
                        .map(|mut r| {
                            // orientare pentru asamblare: toate în sens orar, apoi golurile se detectează prin conținere
                            if ring_signed_area(&r) > 0.0 {
                                r.reverse();
                            }
                            r
                        })
                        .collect(),
                ) {
                    let g = nest_holes(g);
                    let g = if self.closed_as_lines {
                        Geometry::from_parts(
                            g.polygons()
                                .iter()
                                .flat_map(|p| p.iter().map(|r| Geometry::LineString(r.clone())))
                                .collect(),
                        )
                        .unwrap_or(g)
                    } else {
                        g
                    };
                    self.emit(
                        e,
                        g,
                        tf,
                        block,
                        vec![(
                            "Pattern".into(),
                            e.s(2)
                                .map(|s| Value::Text(s.to_string()))
                                .unwrap_or(Value::Null),
                        )],
                    );
                }
            }
            "3DFACE" | "SOLID" | "TRACE" => {
                let mut pts = vec![e.pt(10), e.pt(11)];
                let (p2, p3) = (e.pt(12), e.pt(13));
                if e.kind == "3DFACE" {
                    pts.push(p2);
                    if p3 != p2 {
                        pts.push(p3);
                    }
                } else {
                    // SOLID: ordinea vârfurilor este 1,2,4,3
                    if p3 != p2 {
                        pts.push(p3);
                    }
                    pts.push(p2);
                }
                pts.push(pts[0]);
                self.emit(e, Geometry::Polygon(vec![pts]), tf, block, vec![]);
            }
            "TEXT" => {
                let align = e.i(72) != 0 || e.i(73) != 0;
                let p = if align && e.s(11).is_some() {
                    e.pt(11)
                } else {
                    e.pt(10)
                };
                let txt = decode_unicode_escapes(e.s(1).unwrap_or(""));
                self.emit(
                    e,
                    Geometry::Point(p),
                    tf,
                    block,
                    vec![
                        ("Text".into(), Value::Text(txt)),
                        ("TextHeight".into(), Value::Real(e.fd(40, 0.0))),
                        ("Rotation".into(), Value::Real(e.fd(50, 0.0))),
                    ],
                );
            }
            "MTEXT" => {
                let mut txt = String::new();
                for (c, v) in &e.pairs {
                    if *c == 3 {
                        txt.push_str(v);
                    }
                }
                txt.push_str(e.s(1).unwrap_or(""));
                let txt = clean_mtext(&decode_unicode_escapes(&txt));
                self.emit(
                    e,
                    Geometry::Point(e.pt(10)),
                    tf,
                    block,
                    vec![
                        ("Text".into(), Value::Text(txt)),
                        ("TextHeight".into(), Value::Real(e.fd(40, 0.0))),
                        ("Rotation".into(), Value::Real(e.fd(50, 0.0))),
                    ],
                );
            }
            "INSERT" => {
                let name = e.s(2).unwrap_or("").to_string();
                let ins = e.pt(10);
                let mir = e.mirrored();
                // atributele blocului -> punct cu câmpuri
                let mut extra: Vec<(String, Value)> =
                    vec![("Rotation".into(), Value::Real(e.fd(50, 0.0)))];
                for a in &e.subs {
                    if a.kind == "ATTRIB"
                        && let Some(tag) = a.s(2)
                    {
                        extra.push((
                            tag.to_string(),
                            infer_value(&decode_unicode_escapes(a.s(1).unwrap_or(""))),
                        ));
                    }
                }
                self.emit(e, Geometry::Point(ins), tf, Some(&name), extra);
                if depth > 8 {
                    return;
                }
                let blocks = self.blocks;
                let Some(b) = blocks.get(&name) else { return };
                let xf = Xf {
                    ox: ins.x,
                    oy: ins.y,
                    oz: ins.z,
                    sx: e.fd(41, 1.0),
                    sy: e.fd(42, 1.0),
                    sz: e.fd(43, 1.0),
                    rot: e.fd(50, 0.0).to_radians(),
                    bx: b.base.x,
                    by: b.base.y,
                    bz: b.base.z,
                };
                let composed = |p: Coord| {
                    let mut q = xf.apply(p);
                    if mir {
                        q.x = -q.x;
                    }
                    tf(q)
                };
                for se in &b.ents {
                    self.entity(se, &composed, Some(&name), depth + 1);
                }
            }
            _ => {}
        }
    }
}

pub fn read(path: &Path, opts: &ReadOptions) -> Result<VectorLayer> {
    let bytes = fs::read(path).with_context(|| tr!("Nu pot citi {}", path.display()))?;
    if bytes.starts_with(b"AutoCAD Binary DXF") {
        bail!(tr!("DXF binar nu este suportat – salvați fișierul ca DXF ASCII"));
    }
    // prima trecere în Latin-1 pentru a afla versiunea și codepage-ul
    let latin: String = bytes.iter().map(|&b| b as char).collect();
    let head = parse_pairs(&latin[..latin.len().min(200_000)]);
    let ver = header_var(&head, "$ACADVER").unwrap_or_default();
    let cp = header_var(&head, "$DWGCODEPAGE").unwrap_or_default();
    let enc: &'static Encoding =
        if let Some(e) = opts.encoding.as_deref().and_then(encoding_from_label) {
            e
        } else if ver.as_str() >= "AC1021" || std::str::from_utf8(&bytes).is_ok() {
            encoding_rs::UTF_8
        } else {
            let c = cp.to_lowercase().replace("ansi_", "");
            encoding_from_label(&c).unwrap_or(encoding_rs::WINDOWS_1250)
        };
    let text = enc.decode(&bytes).0;
    let pairs = parse_pairs(text.trim_start_matches('\u{feff}'));

    // secțiuni
    let mut sections: HashMap<String, Pairs> = HashMap::new();
    let mut i = 0;
    while i < pairs.len() {
        if pairs[i].0 == 0 && pairs[i].1.trim() == "SECTION" && i + 1 < pairs.len() {
            let name = pairs[i + 1].1.trim().to_uppercase();
            let start = i + 2;
            let mut j = start;
            while j < pairs.len() && !(pairs[j].0 == 0 && pairs[j].1.trim() == "ENDSEC") {
                j += 1;
            }
            sections.insert(name, pairs[start..j].to_vec());
            i = j + 1;
        } else {
            i += 1;
        }
    }
    // blocuri
    let mut blocks: HashMap<String, Block> = HashMap::new();
    if let Some(bp) = sections.get("BLOCKS") {
        let ents = group_entities(bp);
        let mut cur: Option<(String, Block)> = None;
        for e in ents {
            match e.kind.as_str() {
                "BLOCK" => {
                    cur = Some((
                        e.s(2).unwrap_or("").to_string(),
                        Block {
                            base: e.pt(10),
                            ents: vec![],
                        },
                    ))
                }
                "ENDBLK" => {
                    if let Some((n, b)) = cur.take() {
                        blocks.insert(n, b);
                    }
                }
                _ => {
                    if let Some((_, b)) = cur.as_mut() {
                        b.ents.push(e);
                    }
                }
            }
        }
    }
    let ents = sections
        .get("ENTITIES")
        .map(|p| group_entities(p))
        .unwrap_or_default();
    let mut ctx = Ctx {
        blocks: &blocks,
        closed_as_lines: opts.dxf_closed_as_lines,
        recs: vec![],
    };
    let id = |c: Coord| c;
    for e in &ents {
        ctx.entity(e, &id, None, 0);
    }
    let name = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let mut layer = VectorLayer::from_named_records(&name, ctx.recs);
    layer.source = Some(path.to_path_buf());
    layer.crs = read_prj_sidecar(path);
    Ok(layer)
}

// ---------------------------------------------------------------------------
// Scriere DXF R12
// ---------------------------------------------------------------------------

fn enc_str(enc: &'static Encoding, s: &str) -> Vec<u8> {
    let mut out = vec![];
    let mut buf = [0u8; 4];
    for ch in s.chars() {
        if ch == '\n' || ch == '\r' {
            out.push(b' ');
            continue;
        }
        let st = ch.encode_utf8(&mut buf);
        let (b, _, err) = enc.encode(st);
        if err || (enc != encoding_rs::UTF_8 && b.len() != 1 && !ch.is_ascii()) {
            out.extend_from_slice(format!("\\U+{:04X}", ch as u32).as_bytes());
        } else {
            out.extend_from_slice(&b);
        }
    }
    out
}

fn layer_name(s: &str) -> String {
    let t: String = s
        .chars()
        .map(|c| {
            if "<>/\\\":;?*|=`,".contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    if t.trim().is_empty() { "0".into() } else { t }
}

pub fn write(layer: &VectorLayer, path: &Path, opts: &WriteOptions) -> Result<()> {
    let enc = opts
        .encoding
        .as_deref()
        .and_then(encoding_from_label)
        .unwrap_or(encoding_rs::WINDOWS_1250);
    let geo = layer.crs.as_ref().is_some_and(|c| c.is_geographic());
    let prec = if geo { 9 } else { 4 };
    let f = |v: f64| {
        let s = format!("{:.*}", prec, v);
        let s = s.trim_end_matches('0').trim_end_matches('.').to_string();
        if s.is_empty() || s == "-" {
            "0".to_string()
        } else {
            s
        }
    };
    let li = layer.field_index("Layer");
    let ci = layer.field_index("Color");
    let ti = layer.field_index("Text").or_else(|| {
        opts.name_field
            .as_deref()
            .and_then(|n| layer.field_index(n))
    });
    let hi = layer.field_index("TextHeight");
    let ri = layer.field_index("Rotation");
    let def_layer = layer_name(&layer.name);
    let z = layer.has_z;

    let mut out: Vec<u8> = vec![];
    let mut put = |code: i32, val: &[u8]| {
        out.extend_from_slice(format!("{code:>3}\r\n").as_bytes());
        out.extend_from_slice(val);
        out.extend_from_slice(b"\r\n");
    };
    let mut layers: Vec<String> = vec!["0".into()];
    let lname = |ft: &Feature| {
        li.map(|i| layer_name(&ft.attrs[i].to_text()))
            .unwrap_or_else(|| def_layer.clone())
    };
    for ft in &layer.features {
        let n = lname(ft);
        if !layers.contains(&n) {
            layers.push(n);
        }
    }
    let b = layer.bounds();
    put(0, b"SECTION");
    put(2, b"HEADER");
    put(9, b"$ACADVER");
    put(1, b"AC1009");
    put(9, b"$DWGCODEPAGE");
    put(
        3,
        format!("ANSI_{}", enc.name().trim_start_matches("windows-")).as_bytes(),
    );
    if b.is_valid() {
        put(9, b"$EXTMIN");
        put(10, f(b.minx).as_bytes());
        put(20, f(b.miny).as_bytes());
        put(9, b"$EXTMAX");
        put(10, f(b.maxx).as_bytes());
        put(20, f(b.maxy).as_bytes());
    }
    put(0, b"ENDSEC");
    put(0, b"SECTION");
    put(2, b"TABLES");
    put(0, b"TABLE");
    put(2, b"LTYPE");
    put(70, b"1");
    put(0, b"LTYPE");
    put(2, b"CONTINUOUS");
    put(70, b"0");
    put(3, b"Solid line");
    put(72, b"65");
    put(73, b"0");
    put(40, b"0.0");
    put(0, b"ENDTAB");
    put(0, b"TABLE");
    put(2, b"LAYER");
    put(70, layers.len().to_string().as_bytes());
    for l in &layers {
        put(0, b"LAYER");
        put(2, &enc_str(enc, l));
        put(70, b"0");
        put(62, b"7");
        put(6, b"CONTINUOUS");
    }
    put(0, b"ENDTAB");
    put(0, b"ENDSEC");
    put(0, b"SECTION");
    put(2, b"ENTITIES");
    for ft in &layer.features {
        let Some(g) = &ft.geom else { continue };
        let ln = enc_str(enc, &lname(ft));
        let color = ci
            .and_then(|i| ft.attrs[i].as_f64())
            .map(|c| c as i64)
            .filter(|c| (1..=255).contains(c));
        let common = |put: &mut dyn FnMut(i32, &[u8])| {
            put(8, &ln);
            if let Some(c) = color {
                put(62, c.to_string().as_bytes());
            }
        };
        for p in g.points() {
            let txt = ti.map(|i| ft.attrs[i].to_text()).filter(|s| !s.is_empty());
            if let Some(t) = txt {
                put(0, b"TEXT");
                common(&mut put);
                put(10, f(p.x).as_bytes());
                put(20, f(p.y).as_bytes());
                put(30, f(p.z).as_bytes());
                let h = hi
                    .and_then(|i| ft.attrs[i].as_f64())
                    .filter(|h| *h > 0.0)
                    .unwrap_or(if geo { 0.0001 } else { 1.0 });
                put(40, f(h).as_bytes());
                put(1, &enc_str(enc, &t));
                if let Some(r) = ri.and_then(|i| ft.attrs[i].as_f64()) {
                    put(50, f(r).as_bytes());
                }
            } else {
                put(0, b"POINT");
                common(&mut put);
                put(10, f(p.x).as_bytes());
                put(20, f(p.y).as_bytes());
                put(30, f(p.z).as_bytes());
            }
        }
        let mut paths: Vec<(&Vec<Coord>, bool)> =
            g.lines().into_iter().map(|l| (l, false)).collect();
        for poly in g.polygons() {
            for r in poly {
                paths.push((r, true));
            }
        }
        for (pts, closed) in paths {
            if pts.len() < 2 {
                continue;
            }
            let mut pts: &[Coord] = pts;
            if closed && pts.len() > 2 && pts.first() == pts.last() {
                pts = &pts[..pts.len() - 1];
            }
            put(0, b"POLYLINE");
            common(&mut put);
            put(66, b"1");
            put(10, b"0");
            put(20, b"0");
            put(30, b"0");
            let flags = (if closed { 1 } else { 0 }) | (if z { 8 } else { 0 });
            put(70, flags.to_string().as_bytes());
            for p in pts {
                put(0, b"VERTEX");
                put(8, &ln);
                put(10, f(p.x).as_bytes());
                put(20, f(p.y).as_bytes());
                put(30, f(if z { p.z } else { 0.0 }).as_bytes());
                if z {
                    put(70, b"32");
                }
            }
            put(0, b"SEQEND");
            put(8, &ln);
        }
    }
    put(0, b"ENDSEC");
    put(0, b"EOF");
    fs::write(path, &out).with_context(|| tr!("Nu pot scrie {}", path.display()))?;
    if let Some(c) = &layer.crs {
        write_prj_sidecar(path, c)?;
    }
    Ok(())
}
