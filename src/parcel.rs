//! Parcelarea și detașarea poligoanelor: împărțirea unui teren în loturi de
//! suprafețe date, prin linii de tăiere paralele (cu o latură, perpendiculare pe
//! ea sau după o orientare) sau prin linii care trec printr-un punct fix (în
//! evantai, rotite în jurul unui vârf).
//!
//! Calculul se face în plan, în unitățile sistemului (metri pentru Stereo 70).
//! Pentru fiecare linie de tăiere poziția se găsește prin bisecție: suprafața
//! părții tăiate crește monoton cu poziția liniei (sau cu unghiul ei), deci se
//! ajunge la suprafața cerută cu precizie de sub 0,0001 m². Intersecțiile cu
//! poligonul (inclusiv cu goluri sau cu mai multe părți) se fac cu operațiile
//! booleene din crate-ul `geo`.

use crate::i18n::tr;
use crate::model::{Coord, Geometry};
use anyhow::{Result, bail};
use geo::{Area, BooleanOps, Coord as GC, LineString, MultiPolygon, Polygon};
use std::f64::consts::{PI, TAU};

/// Precizia la care se oprește căutarea suprafeței, în unități pătrate.
pub const AREA_TOL: f64 = 1e-5;

/// Cum se trag liniile de tăiere.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Method {
    /// Linii paralele cu direcția `angle` (radiani, față de axa x, în sens
    /// trigonometric). Loturile se iau pe rând, pornind de la marginea poligonului
    /// cu proiecția cea mai mică pe normala `normal` (vector unitar).
    Parallel { angle: f64, normal: (f64, f64) },
    /// Linii prin punctul fix `pivot`; prima linie are direcția `angle0`, iar
    /// celelalte se rotesc în sensul `ccw` (true = trigonometric).
    Pivot { pivot: (f64, f64), angle0: f64, ccw: bool },
}

/// Suprafețele loturilor.
#[derive(Clone, Debug, PartialEq)]
pub enum Sizes {
    /// `n` loturi egale.
    Equal(usize),
    /// Suprafețele date, în ordine; dacă rămâne teren, `rest = true` îl face lot
    /// separat, altfel el intră în ultimul lot.
    Areas { areas: Vec<f64>, rest: bool },
}

/// Un lot rezultat.
#[derive(Clone, Debug)]
pub struct Lot {
    pub geom: MultiPolygon<f64>,
    pub area: f64,
    /// suprafața cerută (None pentru rest)
    pub target: Option<f64>,
}

/// Geometria cadastru → `geo` (doar poligoane), cu originea mutată în `o`.
pub fn to_geo(g: &Geometry, o: (f64, f64)) -> Option<MultiPolygon<f64>> {
    let ring = |r: &Vec<Coord>| -> LineString<f64> {
        LineString::from(r.iter().map(|c| GC { x: c.x - o.0, y: c.y - o.1 }).collect::<Vec<_>>())
    };
    let poly = |rings: &Vec<Vec<Coord>>| -> Option<Polygon<f64>> {
        let ext = rings.first()?;
        (ext.len() >= 3).then(|| Polygon::new(ring(ext), rings[1..].iter().map(ring).collect()))
    };
    let polys: Vec<Polygon<f64>> = g.polygons().into_iter().filter_map(|p| poly(p)).collect();
    (!polys.is_empty()).then(|| MultiPolygon::new(polys))
}

/// `geo` → geometria cadastru, cu originea readusă din `o`.
pub fn from_geo(mp: &MultiPolygon<f64>, o: (f64, f64)) -> Geometry {
    let ring = |l: &LineString<f64>| -> Vec<Coord> {
        let mut v: Vec<Coord> = l.coords().map(|c| Coord::new(c.x + o.0, c.y + o.1)).collect();
        if v.len() > 1 && v.first() != v.last() {
            v.push(v[0]);
        }
        v
    };
    let mut polys: Vec<Vec<Vec<Coord>>> = mp
        .0
        .iter()
        .map(|p| {
            let mut r = vec![ring(p.exterior())];
            r.extend(p.interiors().iter().map(ring));
            r
        })
        .collect();
    if polys.len() == 1 {
        Geometry::Polygon(polys.remove(0))
    } else {
        Geometry::MultiPolygon(polys)
    }
}

pub fn area(mp: &MultiPolygon<f64>) -> f64 {
    mp.unsigned_area()
}

pub fn perimeter(mp: &MultiPolygon<f64>) -> f64 {
    let len = |l: &LineString<f64>| -> f64 {
        l.0.windows(2).map(|w| (w[1].x - w[0].x).hypot(w[1].y - w[0].y)).sum()
    };
    mp.0.iter().map(|p| len(p.exterior()) + p.interiors().iter().map(len).sum::<f64>()).sum()
}

fn poly_from(pts: Vec<(f64, f64)>) -> Polygon<f64> {
    Polygon::new(LineString::from(pts.into_iter().map(|(x, y)| GC { x, y }).collect::<Vec<_>>()), vec![])
}

/// Încadrarea poligonului: (minx, miny, maxx, maxy).
fn bbox(mp: &MultiPolygon<f64>) -> (f64, f64, f64, f64) {
    let mut b = (f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
    for p in &mp.0 {
        for c in p.exterior().coords() {
            b.0 = b.0.min(c.x);
            b.1 = b.1.min(c.y);
            b.2 = b.2.max(c.x);
            b.3 = b.3.max(c.y);
        }
    }
    b
}

/// Banda dintre liniile de cotă `t0` și `t1` pe normala `n` (direcția liniilor `d`).
fn strip(d: (f64, f64), n: (f64, f64), smin: f64, smax: f64, t0: f64, t1: f64) -> Polygon<f64> {
    let p = |s: f64, t: f64| (s * d.0 + t * n.0, s * d.1 + t * n.1);
    poly_from(vec![p(smin, t0), p(smax, t0), p(smax, t1), p(smin, t1)])
}

/// Sectorul de cerc cu centrul `c`, rază `r`, între unghiurile `a0` și `a1` (a1 > a0).
fn sector(c: (f64, f64), r: f64, a0: f64, a1: f64) -> Polygon<f64> {
    let span = (a1 - a0).max(0.0);
    let n = ((span / (PI / 18.0)).ceil() as usize).max(1);
    let mut pts = vec![c];
    for i in 0..=n {
        let a = a0 + span * i as f64 / n as f64;
        pts.push((c.0 + r * a.cos(), c.1 + r * a.sin()));
    }
    poly_from(pts)
}

/// Bisecție: cel mai mic `x` din [lo, hi] cu f(x) ≈ target (f crescătoare).
fn solve(f: &dyn Fn(f64) -> f64, mut lo: f64, mut hi: f64, target: f64) -> f64 {
    for _ in 0..200 {
        let mid = 0.5 * (lo + hi);
        let v = f(mid);
        if (v - target).abs() < AREA_TOL * 0.1 {
            return mid;
        }
        if v < target {
            lo = mid;
        } else {
            hi = mid;
        }
        if (hi - lo).abs() < 1e-12 * (1.0 + hi.abs()) {
            break;
        }
    }
    0.5 * (lo + hi)
}

/// Suprafețele cumulate ale tăieturilor (fără ultima, care e marginea poligonului).
fn cut_targets(total: f64, sizes: &Sizes) -> Result<(Vec<f64>, Vec<Option<f64>>)> {
    match sizes {
        Sizes::Equal(n) => {
            if *n < 2 {
                bail!("{}", tr!("Numărul de loturi trebuie să fie cel puțin 2."));
            }
            let a = total / *n as f64;
            Ok(((1..*n).map(|i| a * i as f64).collect(), vec![Some(a); *n]))
        }
        Sizes::Areas { areas, rest } => {
            if areas.is_empty() {
                bail!("{}", tr!("Scrieți cel puțin o suprafață."));
            }
            if areas.iter().any(|a| !(a.is_finite() && *a > 0.0)) {
                bail!("{}", tr!("Suprafețele trebuie să fie numere pozitive."));
            }
            let sum: f64 = areas.iter().sum();
            if sum > total + AREA_TOL {
                bail!(
                    "{}",
                    tr!("Suprafețele cerute ({:.2}) depășesc suprafața terenului ({:.2}).", sum, total)
                );
            }
            let left = total - sum;
            let mut cum = 0.0;
            let mut cuts = vec![];
            let mut targets: Vec<Option<f64>> = areas.iter().map(|a| Some(*a)).collect();
            let keep_rest = *rest && left > AREA_TOL * 10.0;
            let n_cuts = if keep_rest { areas.len() } else { areas.len() - 1 };
            for a in &areas[..n_cuts] {
                cum += a;
                cuts.push(cum);
            }
            if keep_rest {
                targets.push(None);
            } else if let Some(last) = targets.last_mut() {
                // restul intră în ultimul lot
                *last = Some(areas[areas.len() - 1] + left);
            }
            Ok((cuts, targets))
        }
    }
}

/// Împarte poligonul (coordonate locale) în loturi.
pub fn divide(poly: &MultiPolygon<f64>, method: Method, sizes: &Sizes) -> Result<Vec<Lot>> {
    let total = area(poly);
    if total <= AREA_TOL {
        bail!("{}", tr!("Poligonul nu are suprafață."));
    }
    let (cuts, targets) = cut_targets(total, sizes)?;
    let (minx, miny, maxx, maxy) = bbox(poly);
    let diag = ((maxx - minx).powi(2) + (maxy - miny).powi(2)).sqrt().max(1e-6);
    // părțile cumulate: poligonul ∩ regiunea „până la tăietura k”
    let parts: Vec<MultiPolygon<f64>> = match method {
        Method::Parallel { angle, normal } => {
            let d = (angle.cos(), angle.sin());
            let nl = (normal.0.hypot(normal.1)).max(1e-300);
            let n = (normal.0 / nl, normal.1 / nl);
            let (mut tmin, mut tmax, mut smin, mut smax) =
                (f64::INFINITY, f64::NEG_INFINITY, f64::INFINITY, f64::NEG_INFINITY);
            for p in &poly.0 {
                for c in p.exterior().coords() {
                    let t = c.x * n.0 + c.y * n.1;
                    let s = c.x * d.0 + c.y * d.1;
                    tmin = tmin.min(t);
                    tmax = tmax.max(t);
                    smin = smin.min(s);
                    smax = smax.max(s);
                }
            }
            let (smin, smax) = (smin - diag, smax + diag);
            let lo = tmin - diag * 0.01;
            let region = |t: f64| poly.intersection(&strip(d, n, smin, smax, lo, t));
            let f = |t: f64| area(&region(t));
            let mut prev = lo;
            let mut ts = vec![];
            for c in &cuts {
                let t = solve(&f, prev, tmax + diag * 0.01, *c);
                ts.push(t);
                prev = t;
            }
            ts.push(tmax + diag * 0.01);
            let mut out = vec![];
            let mut t0 = lo;
            for t in ts {
                out.push(poly.intersection(&strip(d, n, smin, smax, t0, t)));
                t0 = t;
            }
            out
        }
        Method::Pivot { pivot, angle0, ccw } => {
            let r = diag * 4.0 + (pivot.0 - minx).abs() + (pivot.1 - miny).abs();
            // unghiul de rotire φ ∈ [0, 2π) → sectorul dintre direcția de pornire și φ
            let sec = |a: f64, b: f64| {
                if ccw {
                    sector(pivot, r, angle0 + a, angle0 + b)
                } else {
                    sector(pivot, r, angle0 - b, angle0 - a)
                }
            };
            let f = |phi: f64| area(&poly.intersection(&sec(0.0, phi)));
            let mut prev = 0.0;
            let mut phis = vec![];
            for c in &cuts {
                let p = solve(&f, prev, TAU, *c);
                phis.push(p);
                prev = p;
            }
            phis.push(TAU);
            let mut out = vec![];
            let mut a0 = 0.0;
            for p in phis {
                out.push(poly.intersection(&sec(a0, p)));
                a0 = p;
            }
            out
        }
    };
    let lots: Vec<Lot> = parts
        .into_iter()
        .zip(targets)
        .map(|(g, target)| {
            let a = area(&g);
            Lot { geom: g, area: a, target }
        })
        .collect();
    let got: f64 = lots.iter().map(|l| l.area).sum();
    if (got - total).abs() > (total * 1e-6).max(0.01) {
        bail!(
            "{}",
            tr!("Loturile nu acoperă tot terenul ({:.2} din {:.2}); verificați poligonul (auto-intersecții?).", got, total)
        );
    }
    if let Some(i) = lots.iter().position(|l| l.area <= AREA_TOL) {
        bail!("{}", tr!("Lotul {} ar ieși gol: direcția sau punctul ales nu permit această împărțire.", i + 1));
    }
    Ok(lots)
}

/// Laturile inelului exterior (al celei mai mari părți): (început, sfârșit).
pub fn sides(poly: &MultiPolygon<f64>) -> Vec<((f64, f64), (f64, f64))> {
    let Some(p) = poly.0.iter().max_by(|a, b| a.unsigned_area().total_cmp(&b.unsigned_area())) else {
        return vec![];
    };
    let mut pts: Vec<(f64, f64)> = p.exterior().coords().map(|c| (c.x, c.y)).collect();
    if pts.len() > 1 && pts.first() == pts.last() {
        pts.pop();
    }
    // vârfuri duplicate consecutive
    pts.dedup_by(|a, b| (a.0 - b.0).abs() < 1e-9 && (a.1 - b.1).abs() < 1e-9);
    let n = pts.len();
    (0..n).map(|i| (pts[i], pts[(i + 1) % n])).collect()
}

/// Inelul exterior al celei mai mari părți e orientat trigonometric.
pub fn is_ccw(poly: &MultiPolygon<f64>) -> bool {
    let s = sides(poly);
    s.iter().map(|(a, b)| a.0 * b.1 - b.0 * a.1).sum::<f64>() > 0.0
}

/// Orientarea (azimutul topografic, față de Nord, în sens orar) a direcției (dx, dy), în grade.
pub fn azimuth_deg(dx: f64, dy: f64) -> f64 {
    let a = dx.atan2(dy).to_degrees();
    if a < 0.0 { a + 360.0 } else { a }
}

/// Unghiul matematic (față de axa x, trigonometric) al unui azimut topografic în grade.
pub fn angle_from_azimuth(az_deg: f64) -> f64 {
    (90.0 - az_deg).to_radians()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(w: f64, h: f64) -> MultiPolygon<f64> {
        MultiPolygon::new(vec![poly_from(vec![(0.0, 0.0), (w, 0.0), (w, h), (0.0, h)])])
    }

    #[test]
    fn equal_lots_parallel_to_side() {
        // 100 × 40 m, loturi egale paralele cu latura de jos
        let p = rect(100.0, 40.0);
        let lots = divide(&p, Method::Parallel { angle: 0.0, normal: (0.0, 1.0) }, &Sizes::Equal(4)).unwrap();
        assert_eq!(lots.len(), 4);
        for l in &lots {
            assert!((l.area - 1000.0).abs() < 1e-3, "{}", l.area);
        }
        // primul lot e lângă latura de jos
        let (_, miny, _, maxy) = bbox(&lots[0].geom);
        assert!(miny.abs() < 1e-6 && (maxy - 10.0).abs() < 1e-3);
    }

    #[test]
    fn detach_area_with_rest_and_without() {
        let p = rect(50.0, 30.0); // 1500
        let s = Sizes::Areas { areas: vec![500.0, 400.0], rest: true };
        let lots = divide(&p, Method::Parallel { angle: PI / 2.0, normal: (1.0, 0.0) }, &s).unwrap();
        assert_eq!(lots.len(), 3);
        assert!((lots[0].area - 500.0).abs() < 1e-3);
        assert!((lots[1].area - 400.0).abs() < 1e-3);
        assert!((lots[2].area - 600.0).abs() < 1e-3);
        assert!(lots[2].target.is_none());
        let s = Sizes::Areas { areas: vec![500.0, 400.0], rest: false };
        let lots = divide(&p, Method::Parallel { angle: PI / 2.0, normal: (1.0, 0.0) }, &s).unwrap();
        assert_eq!(lots.len(), 2);
        assert!((lots[1].area - 1000.0).abs() < 1e-3);
        assert!(divide(&p, Method::Parallel { angle: 0.0, normal: (0.0, 1.0) }, &Sizes::Areas { areas: vec![2000.0], rest: true }).is_err());
    }

    #[test]
    fn pivot_fan_from_vertex() {
        // triunghi dreptunghic, loturi egale în evantai din vârful (0,0)
        let p = MultiPolygon::new(vec![poly_from(vec![(0.0, 0.0), (60.0, 0.0), (0.0, 60.0)])]);
        let lots = divide(&p, Method::Pivot { pivot: (0.0, 0.0), angle0: 0.0, ccw: true }, &Sizes::Equal(3)).unwrap();
        assert_eq!(lots.len(), 3);
        for l in &lots {
            assert!((l.area - 600.0).abs() < 1e-3, "{}", l.area);
        }
    }

    #[test]
    fn concave_polygon_with_hole_and_big_coordinates() {
        // poligon în L, cu un gol, la coordonate Stereo 70; calcul în coordonate locale
        let o = (480_000.0, 380_000.0);
        let g = Geometry::Polygon(vec![
            vec![
                Coord::new(o.0, o.1),
                Coord::new(o.0 + 80.0, o.1),
                Coord::new(o.0 + 80.0, o.1 + 30.0),
                Coord::new(o.0 + 30.0, o.1 + 30.0),
                Coord::new(o.0 + 30.0, o.1 + 70.0),
                Coord::new(o.0, o.1 + 70.0),
                Coord::new(o.0, o.1),
            ],
            vec![
                Coord::new(o.0 + 5.0, o.1 + 5.0),
                Coord::new(o.0 + 5.0, o.1 + 15.0),
                Coord::new(o.0 + 15.0, o.1 + 15.0),
                Coord::new(o.0 + 15.0, o.1 + 5.0),
                Coord::new(o.0 + 5.0, o.1 + 5.0),
            ],
        ]);
        let p = to_geo(&g, o).unwrap();
        let total = area(&p);
        assert!((total - (80.0 * 30.0 + 30.0 * 40.0 - 100.0)).abs() < 1e-6);
        let lots = divide(&p, Method::Parallel { angle: 0.0, normal: (0.0, 1.0) }, &Sizes::Equal(5)).unwrap();
        for l in &lots {
            assert!((l.area - total / 5.0).abs() < 1e-3);
        }
        let back = from_geo(&lots[0].geom, o);
        assert!(back.polygons().iter().all(|r| r[0][0].x > 400_000.0));
    }

    #[test]
    fn azimuth_conventions() {
        assert!((azimuth_deg(0.0, 1.0) - 0.0).abs() < 1e-9);
        assert!((azimuth_deg(1.0, 0.0) - 90.0).abs() < 1e-9);
        assert!((angle_from_azimuth(90.0)).abs() < 1e-12);
    }
}
