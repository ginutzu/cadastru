//! TransDatRO — transformarea oficială ANCPI între ETRS89 și sistemele naționale
//! Stereografic 1970 (Krasovski 1942), Stereografic 1930 București (Hayford 1910)
//! și sistemul de altitudini normale Marea Neagră 1975.
//!
//! Port exact al algoritmului publicat de ANCPI în `TransDatRO_code_source` (v1.06):
//!
//! 1. (B, L) ETRS89 → proiecție stereografică oblică pe elipsoidul GRS80 (dublă, sferă Gauss)
//! 2. transformare Helmert 2D cu 4 parametri → (X', Y') Stereo 70 / Stereo 30
//! 3. + corecții din gridul de distorsiuni, interpolate bicubic spline (16 noduri)
//! 4. H(MN75) = h(ETRS89) − ζ, cu anomalia altitudinii ζ din gridul de cvasigeoid
//!
//! Inversa parcurge pașii în ordine opusă (fără iterație la pasul 3, ca în programul oficial).
//! Gridurile oficiale sunt incluse în executabil; versiuni mai noi (de ex. EGG97_QGRJ.GRD,
//! actualizat periodic de ANCPI) pot fi puse în dosarul `grile` de lângă executabil.

use std::fmt;
use std::path::PathBuf;
use std::sync::OnceLock;
use crate::i18n::tr;

/// Sistemul național de destinație/sursă.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stereo {
    /// Stereografic 1970, sistem Krasovski 1942 (x = Nord, y = Est)
    S70,
    /// Stereografic 1930, plan secant București, sistem Hayford 1910 (x = Est, y = Nord)
    S30,
}

/// Modelul de cvasigeoid pentru altitudini.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum HeightModel {
    /// Ca în TransDatRO 4.x: cvasigeoidul București-Ilfov (zitaBucx) în zona lui, EGG97 în rest
    #[default]
    Auto,
    /// Doar cvasigeoidul național EGG97_QGRJ (ca în codul sursă 1.06)
    Egg97,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TdrError {
    /// Punctul e în afara dreptunghiului acoperit de grid
    OutsideGrid(&'static str),
    /// Punctul e în afara graniței (noduri marcate cu 999)
    OutsideBorder(&'static str),
    /// Grid lipsă sau invalid
    Grid(String),
}

impl fmt::Display for TdrError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TdrError::OutsideGrid(g) => write!(f, "{}", tr!("punct în afara gridului {g}")),
            TdrError::OutsideBorder(g) => {
                write!(f, "{}", tr!("punct în afara zonei de transformare (grid {g})"))
            }
            TdrError::Grid(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for TdrError {}

pub type TdrResult<T> = Result<T, TdrError>;

// ---------------------------------------------------------------------------
// Constante (clasa dV din programul oficial)

const A_GRS80: f64 = 6378137.0;
const INV_F_GRS80: f64 = 298.257222101;

/// Parametri Helmert 2D: (tE [m], tN [m], dm [ppm], Rz [″])
type Helmert = (f64, f64, f64, f64);
const OS_ST70: Helmert = (119.7358, 31.8051, 0.11559991, -0.22739706);
const ST70_OS: Helmert = (-119.7358, -31.8051, -0.11559991, 0.22739706);
const OS_ST30: Helmert = (-32701.3610, 13962.1632, 13.97707176, -1006.26886396);
const ST30_OS: Helmert = (32768.6284, -13802.2702, -13.97689927, 1006.26886393);

/// Limitele zonei în care TransDatRO folosește cvasigeoidul București-Ilfov.
const BUCX_ZONE: (f64, f64, f64, f64) = (25.98161988, 44.34914006, 26.22410817, 44.51947112);

// ---------------------------------------------------------------------------
// Proiecția stereografică oblică (dublă) pe GRS80, centrată în φ0 = 46°, λ0 = 25°

struct Sphere {
    ep: f64,
    raza: f64,
    n: f64,
    c: f64,
    hi0: f64,
}

const K0: f64 = 0.99975;
const FN: f64 = 500000.0;
const FE: f64 = 500000.0;

fn sphere() -> &'static Sphere {
    static S: OnceLock<Sphere> = OnceLock::new();
    S.get_or_init(|| {
        let a = A_GRS80;
        let br1 = 46.0 * std::f64::consts::PI / 180.0;
        let f = 1.0 / INV_F_GRS80;
        let b = a * (1.0 - f);
        let ep = ((a * a - b * b) / (a * a)).sqrt();
        let w = (1.0 - ep * ep * br1.sin() * br1.sin()).sqrt();
        let mut raza = (a * (1.0 - ep * ep)) / (w * w * w);
        raza = (raza * a / w).sqrt();
        let cb = br1.cos();
        let n = (1.0 + (ep * ep * cb * cb * cb * cb) / (1.0 - ep * ep)).sqrt();
        let s1 = (1.0 + br1.sin()) / (1.0 - br1.sin());
        let s2 = (1.0 - ep * br1.sin()) / (1.0 + ep * br1.sin());
        let w1 = (n * (s1 * (ep * s2.ln()).exp()).ln()).exp();
        let c = ((n + br1.sin()) * (1.0 - (w1 - 1.0) / (w1 + 1.0)))
            / ((n - br1.sin()) * (1.0 + (w1 - 1.0) / (w1 + 1.0)));
        let w2 = c * w1;
        let mut hi0 = (w2 - 1.0) / (w2 + 1.0);
        hi0 = (hi0 / (1.0 - hi0 * hi0).sqrt()).atan();
        Sphere {
            ep,
            raza,
            n,
            c,
            hi0,
        }
    })
}

/// (φ, λ) în radiani → (Nord, Est) în proiecția stereografică oblică GRS80.
fn stereo_fila_ne(fi: f64, la: f64) -> (f64, f64) {
    let s = sphere();
    let la0 = 25.0 * std::f64::consts::PI / 180.0;
    let (ep, n, c, hi0, raza) = (s.ep, s.n, s.c, s.hi0, s.raza);
    let sa = (1.0 + fi.sin()) / (1.0 - fi.sin());
    let sb = (1.0 - ep * fi.sin()) / (1.0 + ep * fi.sin());
    let w = c * (n * (sa * (ep * sb.ln()).exp()).ln()).exp();
    let mut hi = (w - 1.0) / (w + 1.0);
    hi = (hi / (1.0 - hi * hi).sqrt()).atan();
    let lam = n * (la - la0) + la0;
    let beta = 1.0 + hi.sin() * hi0.sin() + hi.cos() * hi0.cos() * (lam - la0).cos();
    let east = 2.0 * raza * K0 * hi.cos() * (lam - la0).sin() / beta;
    let north =
        2.0 * raza * K0 * (hi0.cos() * hi.sin() - hi0.sin() * hi.cos() * (lam - la0).cos()) / beta;
    (north + FN, east + FE)
}

/// (Nord, Est) în proiecția stereografică oblică GRS80 → (φ, λ) în radiani.
fn stereo_ne_fila(north: f64, east: f64) -> (f64, f64) {
    use std::f64::consts::PI;
    let s = sphere();
    let la0 = 25.0 * PI / 180.0;
    let (ep, n, c, hi0, raza) = (s.ep, s.n, s.c, s.hi0, s.raza);
    let g = 2.0 * raza * K0 * (PI / 4.0 - hi0 / 2.0).tan();
    let h = 4.0 * raza * K0 * hi0.tan() + g;
    let ii = ((east - FE) / (h + (north - FN))).atan();
    let j = ((east - FE) / (g - (north - FN))).atan() - ii;
    let lam = j + 2.0 * ii + la0;
    let la = la0 + (lam - la0) / n;
    let hi = hi0 + 2.0 * ((north - FN - (east - FE) * (j / 2.0).tan()) / (2.0 * raza * K0)).atan();
    let csi = (0.5 * ((1.0 + hi.sin()) / (c * (1.0 - hi.sin()))).ln()) / n;
    let mut fi = 2.0 * csi.exp().atan() - PI / 2.0;
    let tol = 0.000001;
    let mut dif = 100.0;
    let mut i = 0;
    while dif > tol && i < 50 {
        i += 1;
        let fic = fi;
        let csii = ((fi / 2.0 + PI / 4.0).tan()
            * ((ep / 2.0) * ((1.0 - ep * fi.sin()) / (1.0 + ep * fi.sin())).ln()).exp())
        .ln();
        fi -= (csii - csi) * fi.cos() * (1.0 - ep * ep * fi.sin() * fi.sin()) / (1.0 - ep * ep);
        dif = (fi * 180.0 * 60.0 * 60.0 / PI - fic * 180.0 * 60.0 * 60.0 / PI).abs();
    }
    (fi, la)
}

/// Helmert 2D: (Est, Nord) → (Est', Nord').
fn helmert(p: Helmert, e: f64, n: f64) -> (f64, f64) {
    let (te, tn, dm, rz) = p;
    let m = 1.0 + dm * 1e-6;
    let rz = rz * std::f64::consts::PI / (180.0 * 3600.0);
    (
        e * m * rz.cos() - n * m * rz.sin() + te,
        n * m * rz.cos() + e * m * rz.sin() + tn,
    )
}

// ---------------------------------------------------------------------------
// Griduri

/// Grid binar TransDatRO: antet (minE, maxE, minN, maxN, stepE, stepN) urmat de valori
/// double little-endian pe rânduri, din colțul stânga-jos (dE, dN pentru gridurile 2D).
pub struct Grid {
    pub name: &'static str,
    pub min_e: f64,
    pub max_e: f64,
    pub min_n: f64,
    pub max_n: f64,
    pub step_e: f64,
    pub step_n: f64,
    ncols: i64,
    dims: usize,
    vals: Vec<f64>,
    /// De unde a fost încărcat (fișier extern sau inclus în program)
    pub source: String,
}

impl Grid {
    pub fn from_bytes(name: &'static str, dims: usize, b: &[u8]) -> TdrResult<Grid> {
        if b.len() < 48 + 8 * dims || !(b.len() - 48).is_multiple_of(8 * dims) {
            return Err(TdrError::Grid(tr!("grid {name} invalid")));
        }
        let rd = |o: usize| f64::from_le_bytes(b[o..o + 8].try_into().unwrap());
        let h: Vec<f64> = (0..6).map(|i| rd(i * 8)).collect();
        let vals: Vec<f64> = (48..b.len()).step_by(8).map(rd).collect();
        let ncols = ((h[1] - h[0]) / h[4] + 1.0).round() as i64;
        let nrows = ((h[3] - h[2]) / h[5] + 1.0).round() as i64;
        if !(h[4] > 0.0 && h[5] > 0.0) || ncols < 4 || nrows < 4 {
            return Err(TdrError::Grid(tr!("antet invalid în gridul {name}")));
        }
        if (ncols * nrows) as usize * dims > vals.len() {
            return Err(TdrError::Grid(tr!("grid {name} incomplet")));
        }
        Ok(Grid {
            name,
            min_e: h[0],
            max_e: h[1],
            min_n: h[2],
            max_n: h[3],
            step_e: h[4],
            step_n: h[5],
            ncols,
            dims,
            vals,
            source: tr!("inclus în program").into(),
        })
    }

    /// Interpolare bicubică spline (clasele Interpolation_1D/2D + BSplineInterpolation).
    /// `e` = Est (sau λ în grade), `n` = Nord (sau φ în grade).
    pub fn interp(&self, e: f64, n: f64) -> TdrResult<[f64; 2]> {
        if !(e.is_finite() && n.is_finite())
            || e <= self.min_e + self.step_e
            || e >= self.max_e - self.step_e
            || n <= self.min_n + self.step_n
            || n >= self.max_n - self.step_n
        {
            return Err(TdrError::OutsideGrid(self.name));
        }
        let col = ((e - self.min_e) / self.step_e) as i64;
        let row = ((n - self.min_n) / self.step_n) as i64;
        let xk = (e - (self.min_e + col as f64 * self.step_e)) / self.step_e;
        let yk = (n - (self.min_n + row as f64 * self.step_n)) / self.step_n;
        let ff = [
            0.0,
            1.0,
            xk,
            xk * xk,
            xk * xk * xk,
            yk,
            xk * yk,
            xk * xk * yk,
            xk * xk * xk * yk,
            yk * yk,
            xk * yk * yk,
            xk * xk * yk * yk,
            xk * xk * xk * yk * yk,
            yk * yk * yk,
            xk * yk * yk * yk,
            xk * xk * yk * yk * yk,
            xk * xk * xk * yk * yk * yk,
        ];
        let mut az = [[0.0f64; 17]; 2];
        let mut outside = false;
        for k in 0..16 {
            let r = row - 1 + (k / 4) as i64;
            let c = col - 1 + (k % 4) as i64;
            let idx = r * self.ncols + c;
            if idx < 0 || (idx as usize + 1) * self.dims > self.vals.len() {
                return Err(TdrError::OutsideGrid(self.name));
            }
            for (d, a) in az.iter_mut().enumerate().take(self.dims) {
                let v = self.vals[idx as usize * self.dims + d];
                if v as i64 == 999 {
                    outside = true;
                }
                a[k + 1] = v;
            }
        }
        if outside {
            return Err(TdrError::OutsideBorder(self.name));
        }
        let mut out = [0.0; 2];
        for d in 0..self.dims {
            out[d] = bspline(&ff, &mut az[d]);
        }
        Ok(out)
    }

    /// Valoarea dintr-un nod (pentru diagnostic).
    pub fn contains(&self, e: f64, n: f64) -> bool {
        self.interp(e, n).is_ok()
    }
}

/// BSplineInterpolation.doBSInterpolation — suprafață bicubică pe pătratul unitar 6-7-11-10.
fn bspline(ff: &[f64; 17], az: &mut [f64; 17]) -> f64 {
    let mut cf = [0.0f64; 17];
    cf[1] = az[6];
    cf[2] = az[7];
    cf[3] = az[10];
    cf[4] = az[11];
    cf[5] = (-az[8] + 4.0 * az[7] - 3.0 * az[6]) / 2.0;
    cf[6] = (3.0 * az[7] - 4.0 * az[6] + az[5]) / 2.0;
    cf[7] = (-az[12] + 4.0 * az[11] - 3.0 * az[10]) / 2.0;
    cf[8] = (3.0 * az[11] - 4.0 * az[10] + az[9]) / 2.0;
    cf[9] = (-az[14] + 4.0 * az[10] - 3.0 * az[6]) / 2.0;
    cf[10] = (-az[15] + 4.0 * az[11] - 3.0 * az[7]) / 2.0;
    cf[11] = (3.0 * az[10] - 4.0 * az[6] + az[2]) / 2.0;
    cf[12] = (3.0 * az[11] - 4.0 * az[7] + az[3]) / 2.0;
    cf[13] = ((az[1] + az[11]) - (az[3] + az[9])) / 4.0;
    cf[14] = ((az[2] + az[12]) - (az[4] + az[10])) / 4.0;
    cf[15] = ((az[5] + az[15]) - (az[7] + az[13])) / 4.0;
    cf[16] = ((az[6] + az[16]) - (az[8] + az[14])) / 4.0;

    az[1] = cf[1];
    az[2] = cf[5];
    az[3] = -3.0 * cf[1] + 3.0 * cf[2] - 2.0 * cf[5] - cf[6];
    az[4] = 2.0 * cf[1] - 2.0 * cf[2] + cf[5] + cf[6];
    az[5] = cf[9];
    az[6] = cf[13];
    az[7] = -3.0 * cf[9] + 3.0 * cf[10] - 2.0 * cf[13] - cf[14];
    az[8] = 2.0 * cf[9] - 2.0 * cf[10] + cf[13] + cf[14];
    az[9] = -3.0 * cf[1] + 3.0 * cf[3] - 2.0 * cf[9] - cf[11];
    az[10] = -3.0 * cf[5] + 3.0 * cf[7] - 2.0 * cf[13] - cf[15];
    az[11] = 9.0 * cf[1] - 9.0 * cf[2] - 9.0 * cf[3] + 9.0 * cf[4] + 6.0 * cf[5] + 3.0 * cf[6]
        - 6.0 * cf[7]
        - 3.0 * cf[8]
        + 6.0 * cf[9]
        - 6.0 * cf[10]
        + 3.0 * cf[11]
        - 3.0 * cf[12]
        + 4.0 * cf[13]
        + 2.0 * cf[14]
        + 2.0 * cf[15]
        + cf[16];
    az[12] = -6.0 * cf[1] + 6.0 * cf[2] + 6.0 * cf[3] - 6.0 * cf[4] - 3.0 * cf[5] - 3.0 * cf[6]
        + 3.0 * cf[7]
        + 3.0 * cf[8]
        - 4.0 * cf[9]
        + 4.0 * cf[10]
        - 2.0 * cf[11]
        + 2.0 * cf[12]
        - 2.0 * cf[13]
        - 2.0 * cf[14]
        - cf[15]
        - cf[16];
    az[13] = 2.0 * cf[1] - 2.0 * cf[3] + cf[9] + cf[11];
    az[14] = 2.0 * cf[5] - 2.0 * cf[7] + cf[13] + cf[15];
    az[15] = -6.0 * cf[1] + 6.0 * cf[2] + 6.0 * cf[3] - 6.0 * cf[4] - 4.0 * cf[5] - 2.0 * cf[6]
        + 4.0 * cf[7]
        + 2.0 * cf[8]
        - 3.0 * cf[9]
        + 3.0 * cf[10]
        - 3.0 * cf[11]
        + 3.0 * cf[12]
        - 2.0 * cf[13]
        - cf[14]
        - 2.0 * cf[15]
        - cf[16];
    az[16] = 4.0 * cf[1] - 4.0 * cf[2] - 4.0 * cf[3] + 4.0 * cf[4] + 2.0 * cf[5] + 2.0 * cf[6]
        - 2.0 * cf[7]
        - 2.0 * cf[8]
        + 2.0 * cf[9]
        - 2.0 * cf[10]
        + 2.0 * cf[11]
        - 2.0 * cf[12]
        + cf[13]
        + cf[14]
        + cf[15]
        + cf[16];
    let mut s = 0.0;
    for i in 1..17 {
        s += az[i] * ff[i];
    }
    s
}

pub struct Grids {
    pub st70: Grid,
    pub st30: Grid,
    pub egg97: Grid,
    pub bucx: Grid,
}

const EMB_ST70: &[u8] = include_bytes!("../assets/transdatro/ETRS89_KRASOVSCHI42_2DJ.GRD");
const EMB_ST30: &[u8] = include_bytes!("../assets/transdatro/ETRS89_Stereo30_2DJ.GRD");
const EMB_EGG97: &[u8] = include_bytes!("../assets/transdatro/EGG97_QGRJ.GRD");
const EMB_BUCX: &[u8] = include_bytes!("../assets/transdatro/zitaBucx.GRD");

/// Dosarele în care se caută griduri mai noi decât cele incluse.
pub fn grid_dirs() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Ok(exe) = std::env::current_exe()
        && let Some(d) = exe.parent()
    {
        v.push(d.join("grile"));
        v.push(d.join("TransDatRO"));
    }
    v
}

fn find_external(file: &str) -> Option<PathBuf> {
    let low = file.to_lowercase();
    for d in grid_dirs() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in rd.flatten() {
            if e.file_name().to_string_lossy().to_lowercase() == low {
                return Some(e.path());
            }
        }
    }
    None
}

fn load(name: &'static str, file: &str, dims: usize, emb: &[u8]) -> Grid {
    if let Some(p) = find_external(file)
        && let Ok(b) = std::fs::read(&p)
        && let Ok(mut g) = Grid::from_bytes(name, dims, &b)
    {
        g.source = p.display().to_string();
        return g;
    }
    Grid::from_bytes(name, dims, emb).expect("grid TransDatRO inclus")
}

/// Gridurile (încărcate o singură dată).
pub fn grids() -> &'static Grids {
    static G: OnceLock<Grids> = OnceLock::new();
    G.get_or_init(|| Grids {
        st70: load(
            "ETRS89_KRASOVSCHI42_2DJ.GRD",
            "ETRS89_KRASOVSCHI42_2DJ.GRD",
            2,
            EMB_ST70,
        ),
        st30: load(
            "ETRS89_Stereo30_2DJ.GRD",
            "ETRS89_Stereo30_2DJ.GRD",
            2,
            EMB_ST30,
        ),
        egg97: load("EGG97_QGRJ.GRD", "EGG97_QGRJ.GRD", 1, EMB_EGG97),
        bucx: load("zitaBucx.grd", "zitaBucx.grd", 1, EMB_BUCX),
    })
}

// ---------------------------------------------------------------------------
// Transformări publice. Unghiurile sunt în grade zecimale; coordonatele plane sunt
// întoarse ca (Est, Nord) indiferent de convenția x/y a sistemului.

/// ETRS89 (φ, λ) → (Est, Nord) în Stereo 70 / Stereo 30 (clasa ETRS89Stereo70or30).
pub fn etrs89_to_stereo(lat: f64, lon: f64, sys: Stereo) -> TdrResult<(f64, f64)> {
    let (ns, es) = stereo_fila_ne(lat.to_radians(), lon.to_radians());
    let g = grids();
    let (par, grid) = match sys {
        Stereo::S70 => (OS_ST70, &g.st70),
        Stereo::S30 => (OS_ST30, &g.st30),
    };
    let (e, n) = helmert(par, es, ns);
    let [de, dn] = grid.interp(e, n)?;
    Ok((e + de, n + dn))
}

/// (Est, Nord) în Stereo 70 / Stereo 30 → ETRS89 (φ, λ) în grade (clasa Stereo70or30ETRS89).
pub fn stereo_to_etrs89(east: f64, north: f64, sys: Stereo) -> TdrResult<(f64, f64)> {
    let g = grids();
    let (par, grid) = match sys {
        Stereo::S70 => (ST70_OS, &g.st70),
        Stereo::S30 => (ST30_OS, &g.st30),
    };
    let [de, dn] = grid.interp(east, north)?;
    let (es, ns) = helmert(par, east - de, north - dn);
    let (fi, la) = stereo_ne_fila(ns, es);
    Ok((fi.to_degrees(), la.to_degrees()))
}

fn in_bucx_zone(lat: f64, lon: f64) -> bool {
    let (l0, p0, l1, p1) = BUCX_ZONE;
    lon >= l0 && lon <= l1 && lat >= p0 && lat <= p1
}

/// Anomalia altitudinii ζ (m) în punctul ETRS89 (φ, λ), plus numele gridului folosit.
pub fn zeta(lat: f64, lon: f64, model: HeightModel) -> TdrResult<(f64, &'static str)> {
    let g = grids();
    if model == HeightModel::Auto
        && in_bucx_zone(lat, lon)
        && let Ok([z, _]) = g.bucx.interp(lon, lat)
    {
        return Ok((z, g.bucx.name));
    }
    let [z, _] = g.egg97.interp(lon, lat)?;
    Ok((z, g.egg97.name))
}

/// Altitudine elipsoidală ETRS89 → altitudine normală Marea Neagră 1975.
pub fn h_to_hmn(lat: f64, lon: f64, h: f64, model: HeightModel) -> TdrResult<f64> {
    Ok(h - zeta(lat, lon, model)?.0)
}

/// Altitudine normală Marea Neagră 1975 → altitudine elipsoidală ETRS89.
pub fn hmn_to_h(lat: f64, lon: f64, hmn: f64, model: HeightModel) -> TdrResult<f64> {
    Ok(hmn + zeta(lat, lon, model)?.0)
}

/// Transformare compusă 2D+1D (ETRS89Stereo7030HMN): (φ, λ, h) → (Est, Nord, H_MN75).
pub fn etrs89_to_stereo_h(
    lat: f64,
    lon: f64,
    h: f64,
    sys: Stereo,
    model: HeightModel,
) -> TdrResult<(f64, f64, f64)> {
    let hmn = h_to_hmn(lat, lon, h, model)?;
    let (e, n) = etrs89_to_stereo(lat, lon, sys)?;
    Ok((e, n, hmn))
}

/// Transformare compusă 2D+1D (Stereo7030HMNETRS89): (Est, Nord, H_MN75) → (φ, λ, h).
pub fn stereo_to_etrs89_h(
    east: f64,
    north: f64,
    hmn: f64,
    sys: Stereo,
    model: HeightModel,
) -> TdrResult<(f64, f64, f64)> {
    let (lat, lon) = stereo_to_etrs89(east, north, sys)?;
    let h = hmn_to_h(lat, lon, hmn, model)?;
    Ok((lat, lon, h))
}

/// Stereo 30 ↔ Stereo 70 prin ETRS89 (transformare compusă).
pub fn stereo_to_stereo(east: f64, north: f64, from: Stereo, to: Stereo) -> TdrResult<(f64, f64)> {
    if from == to {
        return Ok((east, north));
    }
    let (lat, lon) = stereo_to_etrs89(east, north, from)?;
    etrs89_to_stereo(lat, lon, to)
}

// ---------------------------------------------------------------------------
// Fișiere text în formatul programului TransDatRO

/// Direcția de transformare (opțiunile programului TransDatRO).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    EtrsToSt(Stereo),
    StToEtrs(Stereo),
    StToSt(Stereo, Stereo),
}

impl Direction {
    pub const ALL: [Direction; 6] = [
        Direction::EtrsToSt(Stereo::S70),
        Direction::StToEtrs(Stereo::S70),
        Direction::EtrsToSt(Stereo::S30),
        Direction::StToEtrs(Stereo::S30),
        Direction::StToSt(Stereo::S30, Stereo::S70),
        Direction::StToSt(Stereo::S70, Stereo::S30),
    ];

    pub fn label(&self) -> &'static str {
        match self {
            Direction::EtrsToSt(Stereo::S70) => tr!("(B,L,h) ETRS89 » (x,y,H) Stereo 70, Marea Neagră 75"),
            Direction::StToEtrs(Stereo::S70) => tr!("(x,y,H) Stereo 70, Marea Neagră 75 » (B,L,h) ETRS89"),
            Direction::EtrsToSt(Stereo::S30) => {
                tr!("(B,L,h) ETRS89 » (x,y,H) Stereo 30 București, Marea Neagră 75")
            }
            Direction::StToEtrs(Stereo::S30) => {
                tr!("(x,y,H) Stereo 30 București, Marea Neagră 75 » (B,L,h) ETRS89")
            }
            Direction::StToSt(Stereo::S30, _) => tr!("(x,y) Stereo 30 București » (x,y) Stereo 70"),
            Direction::StToSt(..) => tr!("(x,y) Stereo 70 » (x,y) Stereo 30 București"),
        }
    }

    /// Cheie scurtă (linie de comandă).
    pub fn key(&self) -> &'static str {
        match self {
            Direction::EtrsToSt(Stereo::S70) => "etrs89-st70",
            Direction::StToEtrs(Stereo::S70) => "st70-etrs89",
            Direction::EtrsToSt(Stereo::S30) => "etrs89-st30",
            Direction::StToEtrs(Stereo::S30) => "st30-etrs89",
            Direction::StToSt(Stereo::S30, _) => "st30-st70",
            Direction::StToSt(..) => "st70-st30",
        }
    }

    pub fn from_key(k: &str) -> Option<Direction> {
        let k: String = k
            .to_lowercase()
            .chars()
            .filter(|c| c.is_alphanumeric())
            .collect::<String>()
            .replace("stereo", "st")
            .replace("wgs84", "etrs89");
        Direction::ALL
            .into_iter()
            .find(|d| d.key().replace('-', "") == k)
    }

    /// Intrarea este în coordonate geografice.
    pub fn input_geographic(&self) -> bool {
        matches!(self, Direction::EtrsToSt(_))
    }

    pub fn output_geographic(&self) -> bool {
        matches!(self, Direction::StToEtrs(_))
    }

    /// Sistemul plan al intrării (dacă intrarea e plană).
    pub fn input_stereo(&self) -> Option<Stereo> {
        match *self {
            Direction::StToEtrs(s) | Direction::StToSt(s, _) => Some(s),
            _ => None,
        }
    }

    pub fn output_stereo(&self) -> Option<Stereo> {
        match *self {
            Direction::EtrsToSt(s) | Direction::StToSt(_, s) => Some(s),
            _ => None,
        }
    }

    pub fn has_height(&self) -> bool {
        !matches!(self, Direction::StToSt(..))
    }
}

/// Coordonatele „x, y” în convenția sistemului: Stereo 70 x=Nord y=Est; Stereo 30 x=Est y=Nord.
pub fn stereo_xy_to_en(sys: Stereo, x: f64, y: f64) -> (f64, f64) {
    match sys {
        Stereo::S70 => (y, x),
        Stereo::S30 => (x, y),
    }
}

pub fn stereo_en_to_xy(sys: Stereo, e: f64, n: f64) -> (f64, f64) {
    match sys {
        Stereo::S70 => (n, e),
        Stereo::S30 => (e, n),
    }
}

/// Citește un unghi: „44 38 43.52664”, „44°38'43.52664"N”, „44:38:43.5”, „44.6454” sau cu virgulă zecimală.
pub fn parse_angle(s: &str) -> Option<f64> {
    let t = s.trim();
    if t.is_empty() {
        return None;
    }
    let up = t.to_uppercase();
    let neg = up.ends_with('S') || up.ends_with('W') || up.ends_with('V') || t.starts_with('-');
    let cleaned: String = t
        .chars()
        .map(|c| match c {
            '0'..='9' | '.' => c,
            ',' => '.',
            _ => ' ',
        })
        .collect();
    let parts: Vec<f64> = cleaned
        .split_whitespace()
        .map(|p| p.parse::<f64>().ok())
        .collect::<Option<Vec<f64>>>()?;
    let v = match parts.as_slice() {
        [d] => *d,
        [d, m] => d + m / 60.0,
        [d, m, s] => d + m / 60.0 + s / 3600.0,
        _ => return None,
    };
    if !v.is_finite() {
        return None;
    }
    Some(if neg { -v } else { v })
}

fn parse_num(s: &str) -> Option<f64> {
    let t: String = s
        .trim()
        .trim_end_matches(['m', 'M'])
        .trim()
        .replace(',', ".")
        .replace(['\u{a0}', ' '], "");
    t.parse::<f64>().ok().filter(|v| v.is_finite())
}

/// Formatează grade în stilul TransDatRO: 44°38'43.52664"N
pub fn fmt_dms(v: f64, is_lat: bool) -> String {
    let hemi = match (is_lat, v >= 0.0) {
        (true, true) => 'N',
        (true, false) => 'S',
        (false, true) => 'E',
        (false, false) => 'W',
    };
    let a = v.abs();
    let mut d = a.trunc();
    let mut m = ((a - d) * 60.0).trunc();
    let mut s = ((a - d) * 60.0 - m) * 60.0;
    if (s * 100000.0).round() / 100000.0 >= 60.0 {
        s = 0.0;
        m += 1.0;
    }
    if m >= 60.0 {
        m = 0.0;
        d += 1.0;
    }
    format!("{}°{:02}'{:08.5}\"{}", d as i64, m as i64, s, hemi)
}

/// Un punct procesat (coordonatele de ieșire pentru afișarea pe hartă).
#[derive(Clone, Debug)]
pub struct PointOut {
    pub id: String,
    /// Ieșirea: (Est, Nord) pentru sistemele plane, (λ, φ) pentru ETRS89
    pub x: f64,
    pub y: f64,
    pub h: Option<f64>,
}

/// Transformă un singur punct. Intrarea `a`, `b` e în convenția direcției:
/// (φ, λ) pentru ETRS89 și (x, y) ale sistemului stereografic.
pub fn transform_point(
    dir: Direction,
    a: f64,
    b: f64,
    h: Option<f64>,
    model: HeightModel,
) -> TdrResult<PointOut> {
    match dir {
        Direction::EtrsToSt(s) => {
            let (e, n) = etrs89_to_stereo(a, b, s)?;
            let h = match h {
                Some(h) => Some(h_to_hmn(a, b, h, model)?),
                None => None,
            };
            Ok(PointOut {
                id: String::new(),
                x: e,
                y: n,
                h,
            })
        }
        Direction::StToEtrs(s) => {
            let (e, n) = stereo_xy_to_en(s, a, b);
            let (lat, lon) = stereo_to_etrs89(e, n, s)?;
            let h = match h {
                Some(h) => Some(hmn_to_h(lat, lon, h, model)?),
                None => None,
            };
            Ok(PointOut {
                id: String::new(),
                x: lon,
                y: lat,
                h,
            })
        }
        Direction::StToSt(f, t) => {
            let (e, n) = stereo_xy_to_en(f, a, b);
            let (e2, n2) = stereo_to_stereo(e, n, f, t)?;
            Ok(PointOut {
                id: String::new(),
                x: e2,
                y: n2,
                h,
            })
        }
    }
}

/// Rezultatul procesării unui fișier text.
pub struct TextResult {
    pub output: String,
    pub points: Vec<PointOut>,
    pub ok: usize,
    pub errors: usize,
}

/// Procesează un text în formatul de intrare TransDatRO (rând de titlu opțional,
/// apoi „cod, coord1, coord2[, h]”, încheiat opțional cu ENDF) și produce ieșirea
/// în același format ca programul oficial.
pub fn process_text(input: &str, dir: Direction, model: HeightModel) -> TextResult {
    let mut out = String::new();
    let mut points = Vec::new();
    let (mut ok, mut errors) = (0, 0);
    let mut first = true;
    for raw in input.lines() {
        let line = raw.trim_end_matches('\r');
        let t = line.trim();
        if t.eq_ignore_ascii_case("ENDF") {
            break;
        }
        // separatorul e virgula; dacă lipsește, acceptăm tab / ; / spații (doar pentru coordonate plane)
        let mut fields: Vec<String> = if t.contains(',') {
            t.split(',').map(|s| s.trim().to_string()).collect()
        } else if t.contains(';') || t.contains('\t') {
            t.split([';', '\t']).map(|s| s.trim().to_string()).collect()
        } else if !dir.input_geographic() {
            t.split_whitespace().map(|s| s.to_string()).collect()
        } else {
            vec![t.to_string()]
        };
        fields.retain(|f| !f.is_empty());
        let parsed = parse_fields(&fields, dir);
        let Some((id, a, b, h)) = parsed else {
            if first || t.is_empty() || t.to_uppercase().starts_with("PROIECT") {
                // rândul de titlu / comentariu se copiază
                out.push_str(line);
                out.push('\n');
            } else {
                out.push_str(&tr!("{t}  ← EROARE: linie nerecunoscută\n"));
                errors += 1;
            }
            first = false;
            continue;
        };
        if first {
            // fișier fără rând de titlu
            out.push('\n');
        }
        first = false;
        let h = if dir.has_height() { h } else { None };
        match transform_point(dir, a, b, h, model) {
            Ok(mut p) => {
                p.id = id.clone();
                let hs = match p.h {
                    Some(v) if dir.has_height() => format!("{v:>11.3}"),
                    _ if dir.has_height() => " fara_cota".to_string(),
                    _ => String::new(),
                };
                let sep = if hs.is_empty() { "" } else { "," };
                if dir.output_geographic() {
                    out.push_str(&format!(
                        "{id}, {}, {}{sep}{hs}\n",
                        fmt_dms(p.y, true),
                        fmt_dms(p.x, false)
                    ));
                } else {
                    let s = dir.output_stereo().unwrap_or(Stereo::S70);
                    let (x, y) = stereo_en_to_xy(s, p.x, p.y);
                    out.push_str(&format!("{id}, {x:.3}, {y:.3}{sep}{hs}\n"));
                }
                points.push(p);
                ok += 1;
            }
            Err(e) => {
                out.push_str(&tr!("{id}, EROARE: {e}\n"));
                errors += 1;
            }
        }
    }
    out.push_str("ENDF\n");
    TextResult {
        output: out,
        points,
        ok,
        errors,
    }
}

fn parse_fields(f: &[String], dir: Direction) -> Option<(String, f64, f64, Option<f64>)> {
    let conv = |s: &str| {
        if dir.input_geographic() {
            parse_angle(s)
        } else {
            parse_num(s)
        }
    };
    // cu cod: cod, a, b[, h]; fără cod: a, b[, h]
    let try_with = |id: &str, rest: &[String]| -> Option<(String, f64, f64, Option<f64>)> {
        if rest.len() < 2 || rest.len() > 3 {
            return None;
        }
        let a = conv(&rest[0])?;
        let b = conv(&rest[1])?;
        let h = match rest.get(2) {
            Some(s) if !s.trim().is_empty() && !s.to_lowercase().contains("fara") => {
                Some(parse_num(s)?)
            }
            _ => None,
        };
        Some((id.to_string(), a, b, h))
    };
    if f.len() >= 3
        && let Some(r) = try_with(&f[0], &f[1..])
    {
        return Some(r);
    }
    try_with("", f)
}

// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn dms(d: f64, m: f64, s: f64) -> f64 {
        d + m / 60.0 + s / 3600.0
    }

    // Seturile oficiale de test din Help_TransDatRO_code_source (toleranță 3 mm / 0.00003″)
    const P70: &[(f64, f64, f64, f64, f64, f64, f64, f64, f64, f64)] = &[
        (47.0, 42.0, 56.4, 22.0, 28.0, 32.0, 162.000, 693771.731, 310723.518, 122.698),
        (47.0, 58.0, 33.2, 26.0, 53.0, 26.7, 251.000, 721361.806, 641283.450, 217.742),
        (46.0, 3.0, 57.4, 20.0, 40.0, 11.6, 129.000, 516470.189, 165265.572, 86.013),
        (45.0, 5.0, 18.2, 27.0, 42.0, 24.0, 55.000, 402327.815, 713143.130, 23.099),
        (44.0, 26.0, 51.3, 22.0, 54.0, 9.3, 302.000, 329703.378, 333185.413, 260.519),
        (43.0, 44.0, 37.2, 25.0, 13.0, 48.1, 129.000, 249343.594, 518651.464, 89.546),
        (46.0, 14.0, 47.6, 23.0, 50.0, 46.1, 536.000, 528076.247, 411159.899, 495.187),
    ];
    const P30: &[(f64, f64, f64, f64, f64, f64, f64, f64, f64, f64)] = &[
        (44.0, 33.0, 7.0, 25.0, 56.0, 17.5, 142.000, 543496.555, 350692.549, 106.873),
        (44.0, 19.0, 32.8, 26.0, 12.0, 38.3, 104.000, 565397.499, 325744.598, 67.849),
        (44.0, 24.0, 21.3, 26.0, 6.0, 38.7, 120.000, 557350.864, 334573.918, 84.185),
        (44.0, 30.0, 11.9, 26.0, 12.0, 41.5, 117.000, 565268.634, 345471.544, 81.924),
        (44.0, 21.0, 47.9, 25.0, 50.0, 37.7, 132.000, 536113.846, 329685.299, 95.560),
    ];

    #[test]
    fn official_stereo70() {
        for &(a, b, c, d, e, f, h, x, y, hmn) in P70 {
            let (lat, lon) = (dms(a, b, c), dms(d, e, f));
            let (ee, nn, hh) =
                etrs89_to_stereo_h(lat, lon, h, Stereo::S70, HeightModel::Egg97).unwrap();
            assert!((nn - x).abs() <= 0.003 && (ee - y).abs() <= 0.003, "{nn} {ee}");
            assert!((hh - hmn).abs() <= 0.003, "{hh}");
            let (la2, lo2, h2) =
                stereo_to_etrs89_h(y, x, hmn, Stereo::S70, HeightModel::Egg97).unwrap();
            assert!(((la2 - lat) * 3600.0).abs() <= 0.0002);
            assert!(((lo2 - lon) * 3600.0).abs() <= 0.0002);
            assert!((h2 - h).abs() <= 0.003);
        }
    }

    #[test]
    fn official_stereo30() {
        // B3 și B4 sunt în zona București-Ilfov: valorile din tabel (marcate cu roșu în
        // ediția nouă) folosesc cvasigeoidul zitaBucx, deci modelul Auto.
        for &(a, b, c, d, e, f, h, x, y, hmn) in P30 {
            let (lat, lon) = (dms(a, b, c), dms(d, e, f));
            let (ee, nn, hh) =
                etrs89_to_stereo_h(lat, lon, h, Stereo::S30, HeightModel::Auto).unwrap();
            assert!((ee - x).abs() <= 0.003 && (nn - y).abs() <= 0.003, "{ee} {nn}");
            assert!((hh - hmn).abs() <= 0.003, "{hh} vs {hmn}");
            let (la2, lo2, h2) =
                stereo_to_etrs89_h(x, y, hmn, Stereo::S30, HeightModel::Auto).unwrap();
            assert!(((la2 - lat) * 3600.0).abs() <= 0.0002, "{}", (la2 - lat) * 3600.0);
            assert!(((lo2 - lon) * 3600.0).abs() <= 0.0002);
            assert!((h2 - h).abs() <= 0.003);
        }
    }

    #[test]
    fn matches_official_java_egg97() {
        // Valori calculate cu TransDatRO_code_source.jar (v1.06, doar EGG97)
        let lat = dms(44.0, 24.0, 21.3);
        let lon = dms(26.0, 6.0, 38.7);
        let (e, n, h) =
            etrs89_to_stereo_h(lat, lon, 120.0, Stereo::S30, HeightModel::Egg97).unwrap();
        assert!((e - 557350.864119).abs() < 1e-6 && (n - 334573.917768).abs() < 1e-6);
        assert!((h - 84.172314).abs() < 1e-6);
        let (lat, lon, h) =
            stereo_to_etrs89_h(310723.518, 693771.731, 122.698, Stereo::S70, HeightModel::Egg97)
                .unwrap();
        assert!((lat - 47.715666665548).abs() < 1e-11 && (lon - 22.475555548782).abs() < 1e-11);
        assert!((h - 161.999863).abs() < 1e-6);
    }

    #[test]
    fn outside() {
        assert!(matches!(
            etrs89_to_stereo(dms(43.0, 11.0, 7.00414), dms(23.0, 8.0, 15.35121), Stereo::S70),
            Err(TdrError::OutsideGrid(_)) | Err(TdrError::OutsideBorder(_))
        ));
        assert!(
            etrs89_to_stereo(dms(47.0, 56.0, 25.22432), dms(20.0, 35.0, 1.23026), Stereo::S70)
                .is_err()
        );
        assert!(stereo_to_etrs89(348668.167, 188993.152, Stereo::S70).is_err());
        assert!(stereo_to_etrs89(170257.544, 725005.421, Stereo::S70).is_err());
        assert!(
            etrs89_to_stereo(dms(44.0, 13.0, 26.43938), dms(26.0, 1.0, 0.7549), Stereo::S30)
                .is_err()
        );
        assert!(stereo_to_etrs89(536869.164, 353703.293, Stereo::S30).is_err());
    }
}
