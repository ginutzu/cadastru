//! Codificare/decodificare geometrii: WKB (binar) și WKT (text).

use crate::model::{Coord, Geometry, Ring};
use anyhow::{Result, anyhow, bail};
use crate::i18n::tr;

// ---------------------------------------------------------------------------
// WKB
// ---------------------------------------------------------------------------

struct WkbReader<'a> {
    b: &'a [u8],
    p: usize,
}

impl<'a> WkbReader<'a> {
    fn need(&self, n: usize) -> Result<()> {
        if self.p + n > self.b.len() {
            bail!(tr!("WKB trunchiat"))
        } else {
            Ok(())
        }
    }
    fn u8(&mut self) -> Result<u8> {
        self.need(1)?;
        self.p += 1;
        Ok(self.b[self.p - 1])
    }
    fn u32(&mut self, le: bool) -> Result<u32> {
        self.need(4)?;
        let a: [u8; 4] = self.b[self.p..self.p + 4].try_into().unwrap();
        self.p += 4;
        Ok(if le {
            u32::from_le_bytes(a)
        } else {
            u32::from_be_bytes(a)
        })
    }
    fn f64(&mut self, le: bool) -> Result<f64> {
        self.need(8)?;
        let a: [u8; 8] = self.b[self.p..self.p + 8].try_into().unwrap();
        self.p += 8;
        Ok(if le {
            f64::from_le_bytes(a)
        } else {
            f64::from_be_bytes(a)
        })
    }
    fn coord(&mut self, le: bool, z: bool, m: bool) -> Result<Coord> {
        let x = self.f64(le)?;
        let y = self.f64(le)?;
        let zz = if z { self.f64(le)? } else { 0.0 };
        if m {
            self.f64(le)?;
        }
        Ok(Coord::xyz(x, y, zz))
    }
    fn coords(&mut self, le: bool, z: bool, m: bool) -> Result<Vec<Coord>> {
        let n = self.u32(le)? as usize;
        if n > self.b.len() {
            bail!(tr!("WKB invalid"));
        }
        (0..n).map(|_| self.coord(le, z, m)).collect()
    }
    fn rings(&mut self, le: bool, z: bool, m: bool) -> Result<Vec<Ring>> {
        let n = self.u32(le)? as usize;
        if n > self.b.len() {
            bail!(tr!("WKB invalid"));
        }
        (0..n).map(|_| self.coords(le, z, m)).collect()
    }
    fn geom(&mut self) -> Result<Geometry> {
        let le = self.u8()? == 1;
        let mut t = self.u32(le)?;
        // EWKB
        let mut z = t & 0x8000_0000 != 0;
        let mut m = t & 0x4000_0000 != 0;
        if t & 0x2000_0000 != 0 {
            self.u32(le)?; // SRID
        }
        t &= 0x0FFF_FFFF;
        // ISO
        match t / 1000 {
            1 => z = true,
            2 => m = true,
            3 => {
                z = true;
                m = true
            }
            _ => {}
        }
        let base = t % 1000;
        Ok(match base {
            1 => Geometry::Point(self.coord(le, z, m)?),
            2 => Geometry::LineString(self.coords(le, z, m)?),
            3 => Geometry::Polygon(self.rings(le, z, m)?),
            4 => {
                let n = self.u32(le)?;
                let mut v = Vec::new();
                for _ in 0..n {
                    v.extend(self.geom()?.points());
                }
                Geometry::MultiPoint(v)
            }
            5 => {
                let n = self.u32(le)?;
                let mut v = Vec::new();
                for _ in 0..n {
                    if let Geometry::LineString(l) = self.geom()? {
                        v.push(l)
                    }
                }
                Geometry::MultiLineString(v)
            }
            6 => {
                let n = self.u32(le)?;
                let mut v = Vec::new();
                for _ in 0..n {
                    if let Geometry::Polygon(p) = self.geom()? {
                        v.push(p)
                    }
                }
                Geometry::MultiPolygon(v)
            }
            7 => {
                let n = self.u32(le)?;
                let mut v = Vec::new();
                for _ in 0..n {
                    v.push(self.geom()?);
                }
                Geometry::Collection(v)
            }
            // CircularString, CompoundCurve, CurvePolygon etc. – nesuportate
            _ => bail!("{}", tr!("Tip WKB nesuportat: {t}")),
        })
    }
}

pub fn read_wkb(b: &[u8]) -> Result<Geometry> {
    WkbReader { b, p: 0 }.geom()
}

fn w_coord(out: &mut Vec<u8>, c: &Coord, z: bool) {
    out.extend_from_slice(&c.x.to_le_bytes());
    out.extend_from_slice(&c.y.to_le_bytes());
    if z {
        out.extend_from_slice(&c.z.to_le_bytes());
    }
}

fn w_head(out: &mut Vec<u8>, t: u32, z: bool) {
    out.push(1);
    out.extend_from_slice(&(if z { t + 1000 } else { t }).to_le_bytes());
}

fn w_coords(out: &mut Vec<u8>, v: &[Coord], z: bool) {
    out.extend_from_slice(&(v.len() as u32).to_le_bytes());
    for c in v {
        w_coord(out, c, z);
    }
}

fn w_rings(out: &mut Vec<u8>, p: &[Ring], z: bool) {
    out.extend_from_slice(&(p.len() as u32).to_le_bytes());
    for r in p {
        w_coords(out, r, z);
    }
}

/// Scrie WKB ISO little-endian (cu Z dacă `z`).
pub fn write_wkb(g: &Geometry, z: bool, out: &mut Vec<u8>) {
    match g {
        Geometry::Point(c) => {
            w_head(out, 1, z);
            w_coord(out, c, z);
        }
        Geometry::LineString(v) => {
            w_head(out, 2, z);
            w_coords(out, v, z);
        }
        Geometry::Polygon(p) => {
            w_head(out, 3, z);
            w_rings(out, p, z);
        }
        Geometry::MultiPoint(v) => {
            w_head(out, 4, z);
            out.extend_from_slice(&(v.len() as u32).to_le_bytes());
            for c in v {
                write_wkb(&Geometry::Point(*c), z, out);
            }
        }
        Geometry::MultiLineString(v) => {
            w_head(out, 5, z);
            out.extend_from_slice(&(v.len() as u32).to_le_bytes());
            for l in v {
                w_head(out, 2, z);
                w_coords(out, l, z);
            }
        }
        Geometry::MultiPolygon(v) => {
            w_head(out, 6, z);
            out.extend_from_slice(&(v.len() as u32).to_le_bytes());
            for p in v {
                w_head(out, 3, z);
                w_rings(out, p, z);
            }
        }
        Geometry::Collection(v) => {
            w_head(out, 7, z);
            out.extend_from_slice(&(v.len() as u32).to_le_bytes());
            for g in v {
                write_wkb(g, z, out);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// WKT
// ---------------------------------------------------------------------------

fn fmt_num(v: f64) -> String {
    if v.fract() == 0.0 && v.abs() < 1e15 {
        format!("{}", v as i64)
    } else {
        let s = format!("{:.9}", v);
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

fn wkt_coord(c: &Coord, z: bool) -> String {
    if z {
        format!("{} {} {}", fmt_num(c.x), fmt_num(c.y), fmt_num(c.z))
    } else {
        format!("{} {}", fmt_num(c.x), fmt_num(c.y))
    }
}

fn wkt_list(v: &[Coord], z: bool) -> String {
    format!(
        "({})",
        v.iter()
            .map(|c| wkt_coord(c, z))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

fn wkt_rings(p: &[Ring], z: bool) -> String {
    format!(
        "({})",
        p.iter()
            .map(|r| wkt_list(r, z))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

pub fn to_wkt(g: &Geometry, z: bool) -> String {
    let zs = if z { " Z" } else { "" };
    match g {
        Geometry::Point(c) => format!("POINT{zs} ({})", wkt_coord(c, z)),
        Geometry::MultiPoint(v) => format!(
            "MULTIPOINT{zs} ({})",
            v.iter()
                .map(|c| format!("({})", wkt_coord(c, z)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Geometry::LineString(v) => format!("LINESTRING{zs} {}", wkt_list(v, z)),
        Geometry::MultiLineString(v) => format!(
            "MULTILINESTRING{zs} ({})",
            v.iter()
                .map(|l| wkt_list(l, z))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Geometry::Polygon(p) => format!("POLYGON{zs} {}", wkt_rings(p, z)),
        Geometry::MultiPolygon(v) => format!(
            "MULTIPOLYGON{zs} ({})",
            v.iter()
                .map(|p| wkt_rings(p, z))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Geometry::Collection(v) => format!(
            "GEOMETRYCOLLECTION ({})",
            v.iter()
                .map(|g| to_wkt(g, z))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

struct WktParser<'a> {
    s: &'a [u8],
    p: usize,
}

impl<'a> WktParser<'a> {
    fn ws(&mut self) {
        while self.p < self.s.len() && self.s[self.p].is_ascii_whitespace() {
            self.p += 1;
        }
    }
    fn peek(&mut self) -> Option<u8> {
        self.ws();
        self.s.get(self.p).copied()
    }
    fn expect(&mut self, c: u8) -> Result<()> {
        if self.peek() == Some(c) {
            self.p += 1;
            Ok(())
        } else {
            bail!("{}", tr!("WKT: se aștepta '{}' la poziția {}", c as char, self.p))
        }
    }
    fn word(&mut self) -> String {
        self.ws();
        let st = self.p;
        while self.p < self.s.len() && self.s[self.p].is_ascii_alphabetic() {
            self.p += 1;
        }
        String::from_utf8_lossy(&self.s[st..self.p]).to_uppercase()
    }
    fn number(&mut self) -> Result<f64> {
        self.ws();
        let st = self.p;
        while self.p < self.s.len()
            && (self.s[self.p].is_ascii_digit() || b"+-.eE".contains(&self.s[self.p]))
        {
            self.p += 1;
        }
        std::str::from_utf8(&self.s[st..self.p])?
            .parse::<f64>()
            .map_err(|_| anyhow!("{}", tr!("WKT: număr invalid la poziția {st}")))
    }
    fn coord(&mut self, dims: usize) -> Result<Coord> {
        let x = self.number()?;
        let y = self.number()?;
        let mut vals = vec![];
        while matches!(self.peek(), Some(c) if c.is_ascii_digit() || c == b'-' || c == b'+' || c == b'.')
        {
            vals.push(self.number()?);
        }
        let z = if dims >= 3 || !vals.is_empty() {
            vals.first().copied().unwrap_or(0.0)
        } else {
            0.0
        };
        Ok(Coord::xyz(x, y, z))
    }
    fn coord_list(&mut self, dims: usize) -> Result<Vec<Coord>> {
        self.expect(b'(')?;
        let mut v = vec![];
        loop {
            // MULTIPOINT poate avea (x y) sau x y
            if self.peek() == Some(b'(') {
                self.p += 1;
                v.push(self.coord(dims)?);
                self.expect(b')')?;
            } else {
                v.push(self.coord(dims)?);
            }
            match self.peek() {
                Some(b',') => self.p += 1,
                _ => break,
            }
        }
        self.expect(b')')?;
        Ok(v)
    }
    fn ring_list(&mut self, dims: usize) -> Result<Vec<Ring>> {
        self.expect(b'(')?;
        let mut v = vec![];
        loop {
            v.push(self.coord_list(dims)?);
            match self.peek() {
                Some(b',') => self.p += 1,
                _ => break,
            }
        }
        self.expect(b')')?;
        Ok(v)
    }
    fn is_empty(&mut self) -> bool {
        let save = self.p;
        if self.word() == "EMPTY" {
            return true;
        }
        self.p = save;
        false
    }
    fn geom(&mut self) -> Result<Geometry> {
        let t = self.word();
        let save = self.p;
        let mut dims = 2;
        match self.word().as_str() {
            "Z" | "M" => dims = 3,
            "ZM" => dims = 4,
            _ => self.p = save,
        }
        if self.is_empty() {
            return Ok(match t.as_str() {
                "POINT" => Geometry::Point(Coord::new(f64::NAN, f64::NAN)),
                "LINESTRING" => Geometry::LineString(vec![]),
                "POLYGON" => Geometry::Polygon(vec![]),
                "MULTIPOINT" => Geometry::MultiPoint(vec![]),
                "MULTILINESTRING" => Geometry::MultiLineString(vec![]),
                "MULTIPOLYGON" => Geometry::MultiPolygon(vec![]),
                _ => Geometry::Collection(vec![]),
            });
        }
        Ok(match t.as_str() {
            "POINT" => {
                self.expect(b'(')?;
                let c = self.coord(dims)?;
                self.expect(b')')?;
                Geometry::Point(c)
            }
            "LINESTRING" => Geometry::LineString(self.coord_list(dims)?),
            "POLYGON" => Geometry::Polygon(self.ring_list(dims)?),
            "MULTIPOINT" => Geometry::MultiPoint(self.coord_list(dims)?),
            "MULTILINESTRING" => Geometry::MultiLineString(self.ring_list(dims)?),
            "MULTIPOLYGON" => {
                self.expect(b'(')?;
                let mut v = vec![];
                loop {
                    v.push(self.ring_list(dims)?);
                    match self.peek() {
                        Some(b',') => self.p += 1,
                        _ => break,
                    }
                }
                self.expect(b')')?;
                Geometry::MultiPolygon(v)
            }
            "GEOMETRYCOLLECTION" => {
                self.expect(b'(')?;
                let mut v = vec![];
                loop {
                    v.push(self.geom()?);
                    match self.peek() {
                        Some(b',') => self.p += 1,
                        _ => break,
                    }
                }
                self.expect(b')')?;
                Geometry::Collection(v)
            }
            other => bail!("{}", tr!("WKT: tip de geometrie necunoscut „{other}”")),
        })
    }
}

pub fn parse_wkt(s: &str) -> Result<Geometry> {
    let s = s.trim();
    // EWKT: SRID=4326;POINT(...)
    let s = match s.find(';') {
        Some(i) if s[..i].to_uppercase().starts_with("SRID") => &s[i + 1..],
        _ => s,
    };
    WktParser {
        s: s.as_bytes(),
        p: 0,
    }
    .geom()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wkt_wkb_roundtrip() {
        for w in [
            "POINT (1 2)",
            "LINESTRING (0 0, 1 1, 2 0)",
            "POLYGON ((0 0, 10 0, 10 10, 0 10, 0 0), (2 2, 3 2, 3 3, 2 2))",
            "MULTIPOLYGON (((0 0, 1 0, 1 1, 0 0)), ((5 5, 6 5, 6 6, 5 5)))",
            "MULTIPOINT ((1 2), (3 4))",
            "MULTILINESTRING ((0 0, 1 1), (2 2, 3 3))",
            "POINT Z (1 2 3)",
        ] {
            let g = parse_wkt(w).unwrap();
            let z = w.contains(" Z");
            assert_eq!(to_wkt(&g, z), w);
            let mut b = vec![];
            write_wkb(&g, z, &mut b);
            assert_eq!(read_wkb(&b).unwrap(), g);
        }
        assert_eq!(
            parse_wkt("MULTIPOINT (1 2, 3 4)").unwrap(),
            parse_wkt("MULTIPOINT ((1 2), (3 4))").unwrap()
        );
    }
}
