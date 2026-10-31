//! Filtrul pe zonă la încărcare: păstrează doar entitățile aflate în interiorul unui dreptunghi
//! dat în Stereo 70 (EPSG:3844) – implicit încadrarea României.
//!
//! Coordonatele entităților se transformă în EPSG:3844 doar pentru test (stratul își păstrează
//! sistemul). Un strat fără sistem de coordonate este luat în sistemul ghicit după extindere
//! (WGS84 sau Stereo 70) sau, dacă nu se poate ghici, în Stereo 70.

use crate::crs::{Crs, Transformer, guess_crs};
use crate::i18n::tr;
use crate::model::{Bounds, VectorLayer};

/// Încadrarea României în Stereo 70 (EPSG:3844): dreptunghiul 20,22°–29,76° E, 43,62°–48,27° N
/// transformat și rotunjit în afară la kilometru (extremele țării: X 134 050–870 648,
/// Y 235 495–753 097).
pub const ROMANIA_3844: [f64; 4] = [114_000.0, 235_000.0, 885_000.0, 764_000.0];

/// Filtrul pe zonă.
#[derive(Clone, Debug, PartialEq)]
pub struct AreaFilter {
    /// dreptunghiul, în EPSG:3844
    pub bounds: Bounds,
    /// `false` = entitatea trebuie să fie în întregime în interior; `true` = ajunge să atingă zona
    pub partial: bool,
}

impl AreaFilter {
    /// Încadrarea României, entități în întregime în interior.
    pub fn romania() -> AreaFilter {
        let [a, b, c, d] = ROMANIA_3844;
        AreaFilter {
            bounds: Bounds::new(a, b, c, d),
            partial: false,
        }
    }
    pub fn is_romania(&self) -> bool {
        let [a, b, c, d] = ROMANIA_3844;
        self.bounds == Bounds::new(a, b, c, d)
    }
    /// Descrierea dreptunghiului: „114000, 235000 – 885000, 764000”.
    pub fn describe(&self) -> String {
        let b = &self.bounds;
        format!("{:.0}, {:.0} – {:.0}, {:.0}", b.minx, b.miny, b.maxx, b.maxy)
    }
}

/// Citește un dreptunghi scris ca `minX, minY, maxX, maxY` (separat prin virgulă, punct și
/// virgulă sau spații).
pub fn parse_bounds(s: &str) -> Option<Bounds> {
    let v: Vec<f64> = s
        .split(|c: char| c == ',' || c == ';' || c.is_whitespace())
        .filter(|t| !t.is_empty())
        .map(|t| t.parse::<f64>())
        .collect::<Result<_, _>>()
        .ok()?;
    if v.len() != 4 || v.iter().any(|x| !x.is_finite()) {
        return None;
    }
    let b = Bounds::new(v[0].min(v[2]), v[1].min(v[3]), v[0].max(v[2]), v[1].max(v[3]));
    (b.width() > 0.0 && b.height() > 0.0).then_some(b)
}

/// Rezultatul filtrării unui strat.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AreaStats {
    pub total: usize,
    pub kept: usize,
    /// entități în afara zonei
    pub outside: usize,
    /// entități fără geometrie (scoase)
    pub no_geom: usize,
    /// sistemul stratului a fost presupus
    pub assumed_crs: bool,
    /// entități scoase înainte, de expresia de filtrare (doar pentru mesaj)
    pub by_expr: usize,
}

impl AreaStats {
    pub fn removed(&self) -> usize {
        self.total - self.kept
    }
    /// Mesajul pentru utilizator: „parcele: 1200 din 1350 entități (150 în afara zonei)”.
    pub fn message(&self, layer: &str) -> String {
        let mut extra = vec![];
        if self.by_expr > 0 {
            extra.push(tr!("{} prin expresie", self.by_expr));
        }
        if self.outside > 0 {
            extra.push(tr!("{} în afara zonei", self.outside));
        }
        if self.no_geom > 0 {
            extra.push(tr!("{} fără geometrie", self.no_geom));
        }
        if self.assumed_crs {
            extra.push(tr!("sistem presupus").to_string());
        }
        let mut s = tr!("{}: păstrate {} din {}", layer, self.kept, self.total);
        if !extra.is_empty() {
            s.push_str(&format!(" ({})", extra.join(", ")));
        }
        s
    }
}

/// Păstrează în strat doar entitățile din zonă.
pub fn filter_layer(layer: &mut VectorLayer, f: &AreaFilter) -> AreaStats {
    let mut st = AreaStats {
        total: layer.features.len(),
        ..Default::default()
    };
    let src = match &layer.crs {
        Some(c) => c.clone(),
        None => {
            st.assumed_crs = true;
            let b = layer.bounds();
            guess_crs(b.minx, b.miny, b.maxx, b.maxy)
                .or_else(|| Crs::from_epsg(3844).ok())
                .expect("EPSG:3844")
        }
    };
    let dst = Crs::from_epsg(3844).expect("EPSG:3844");
    let t = Transformer::new(&src, &dst).ok();
    let b = f.bounds;
    layer.features.retain(|feat| {
        let Some(g) = &feat.geom else {
            st.no_geom += 1;
            return false;
        };
        if g.is_empty() {
            st.no_geom += 1;
            return false;
        }
        let inside = match &t {
            // sistem necunoscut pentru transformare: coordonatele se iau ca atare
            None => test(g, &b, f.partial),
            Some(t) if t.is_identity() => test(g, &b, f.partial),
            Some(t) if !f.partial => {
                // în întregime în interior: fiecare vârf transformat în dreptunghi
                let mut ok = true;
                g.for_each_coord(&mut |c| {
                    if ok {
                        ok = t.xy(c.x, c.y).is_some_and(|(x, y)| b.contains(x, y));
                    }
                });
                ok
            }
            Some(t) => {
                let mut g2 = g.clone();
                t.geometry(&mut g2);
                g2.intersects_bounds(&b)
            }
        };
        if !inside {
            st.outside += 1;
        }
        inside
    });
    st.kept = layer.features.len();
    st
}

fn test(g: &crate::model::Geometry, b: &Bounds, partial: bool) -> bool {
    if partial {
        g.intersects_bounds(b)
    } else {
        let gb = g.bounds();
        gb.is_valid() && gb.minx >= b.minx && gb.maxx <= b.maxx && gb.miny >= b.miny && gb.maxy <= b.maxy
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Coord, Feature, Geometry};

    fn pt(x: f64, y: f64) -> Feature {
        Feature {
            geom: Some(Geometry::Point(Coord::new(x, y))),
            attrs: vec![],
        }
    }
    fn line(a: (f64, f64), b: (f64, f64)) -> Feature {
        Feature {
            geom: Some(Geometry::LineString(vec![Coord::new(a.0, a.1), Coord::new(b.0, b.1)])),
            attrs: vec![],
        }
    }

    #[test]
    fn stereo70() {
        let mut l = VectorLayer::new("t");
        l.crs = Crs::from_epsg(3844).ok();
        l.features = vec![
            pt(500_000.0, 300_000.0),
            pt(0.0, 0.0),
            line((500_000.0, 300_000.0), (950_000.0, 300_000.0)),
            Feature { geom: None, attrs: vec![] },
        ];
        let mut l2 = l.clone();
        let st = filter_layer(&mut l, &AreaFilter::romania());
        assert_eq!((st.total, st.kept, st.outside, st.no_geom), (4, 1, 2, 1));
        assert!(st.message("t").contains("1"));
        // parțial: linia care iese din zonă rămâne
        let f = AreaFilter { partial: true, ..AreaFilter::romania() };
        let st = filter_layer(&mut l2, &f);
        assert_eq!(st.kept, 2);
    }

    #[test]
    fn wgs84_and_unknown() {
        let mut l = VectorLayer::new("t");
        l.crs = Some(Crs::wgs84());
        // București, Viena, Constanța
        l.features = vec![pt(26.10, 44.43), pt(16.37, 48.21), pt(28.63, 44.17)];
        let st = filter_layer(&mut l, &AreaFilter::romania());
        assert_eq!(st.kept, 2);
        assert!(!st.assumed_crs);
        // fără sistem: ghicit după extindere (grade -> WGS84)
        let mut l = VectorLayer::new("t");
        l.features = vec![pt(26.10, 44.43), pt(16.37, 48.21)];
        let st = filter_layer(&mut l, &AreaFilter::romania());
        assert_eq!(st.kept, 1);
        assert!(st.assumed_crs);
    }

    #[test]
    fn parse() {
        let b = parse_bounds("114000, 235000, 885000, 764000").unwrap();
        assert_eq!(b, Bounds::new(114000.0, 235000.0, 885000.0, 764000.0));
        assert_eq!(parse_bounds("885000 764000 114000 235000"), Some(b));
        assert!(parse_bounds("1,2,3").is_none());
        assert!(parse_bounds("1,1,1,5").is_none());
        assert!(AreaFilter::romania().is_romania());
    }
}
