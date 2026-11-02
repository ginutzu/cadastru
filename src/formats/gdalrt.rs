//! Citirea rasterelor ECW, MrSID și JPEG 2000 prin GDAL, încărcat dinamic.
//!
//! Formatele ECW și MrSID sunt proprietare: decodarea lor are nevoie de
//! bibliotecile producătorilor (ERDAS ECW/JP2 SDK, MrSID Decode SDK), care nu
//! pot fi incluse într-o aplicație cu licență MIT. Cadastru folosește, dacă
//! există pe calculator, GDAL-ul unei instalări QGIS / OSGeo4W, care are deja
//! driverele ECW și MrSID (citire gratuită). Biblioteca se încarcă la prima
//! nevoie; fără ea, restul aplicației funcționează normal.
//!
//! Doar citire: Cadastru nu scrie (nu convertește) în ECW sau MrSID.
//!
//! Rasterele acestea au adesea zeci de mii de pixeli pe latură, așa că în
//! memorie se ține doar o imagine de ansamblu; detaliul se citește din fișier
//! pentru zona afișată, la rezoluția ecranului (vezi [`RasterSource`]).

use crate::crs::Crs;
use crate::i18n::tr;
use crate::model::{BandData, FullSource, Raster, RasterSource};
use anyhow::{Result, anyhow, bail};
use libloading::Library;
use std::ffi::{CStr, CString, c_char, c_double, c_int, c_void};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

type H = *mut c_void;

/// Latura maximă a imaginii de ansamblu ținute în memorie.
pub const OVERVIEW_MAX: usize = 4096;

/// Extensiile citite prin GDAL și driverele potrivite.
pub const EXTS: &[(&str, &[&str])] = &[
    ("ecw", &["ECW"]),
    ("sid", &["MrSID"]),
    ("jp2", &["JP2ECW", "JP2OpenJPEG", "JP2KAK", "JP2MrSID", "JPEG2000"]),
];

/// Driverele afișate în fereastra de stare.
pub const DRIVERS: &[(&str, &str)] = &[
    ("ECW", "ECW"),
    ("MrSID", "MrSID"),
    ("JP2ECW", "JPEG 2000 (ERDAS)"),
    ("JP2OpenJPEG", "JPEG 2000 (OpenJPEG)"),
];

struct Api {
    all_register: unsafe extern "C" fn(),
    version_info: unsafe extern "C" fn(*const c_char) -> *const c_char,
    set_config: unsafe extern "C" fn(*const c_char, *const c_char),
    open_ex: unsafe extern "C" fn(
        *const c_char,
        u32,
        *const *const c_char,
        *const *const c_char,
        *const *const c_char,
    ) -> H,
    close: unsafe extern "C" fn(H),
    x_size: unsafe extern "C" fn(H) -> c_int,
    y_size: unsafe extern "C" fn(H) -> c_int,
    count: unsafe extern "C" fn(H) -> c_int,
    get_gt: unsafe extern "C" fn(H, *mut c_double) -> c_int,
    proj_ref: unsafe extern "C" fn(H) -> *const c_char,
    get_band: unsafe extern "C" fn(H, c_int) -> H,
    band_type: unsafe extern "C" fn(H) -> c_int,
    nodata: unsafe extern "C" fn(H, *mut c_int) -> c_double,
    color_interp: unsafe extern "C" fn(H) -> c_int,
    #[allow(clippy::type_complexity)]
    ds_io: unsafe extern "C" fn(
        H,
        c_int,
        c_int,
        c_int,
        c_int,
        c_int,
        *mut c_void,
        c_int,
        c_int,
        c_int,
        c_int,
        *const c_int,
        c_int,
        c_int,
        c_int,
    ) -> c_int,
    driver_by_name: unsafe extern "C" fn(*const c_char) -> H,
    ds_driver: unsafe extern "C" fn(H) -> H,
    driver_short: unsafe extern "C" fn(H) -> *const c_char,
    last_err: unsafe extern "C" fn() -> *const c_char,
    err_reset: unsafe extern "C" fn(),
    osr_new: unsafe extern "C" fn(*const c_char) -> H,
    osr_auto_epsg: unsafe extern "C" fn(H) -> c_int,
    osr_auth_name: unsafe extern "C" fn(H, *const c_char) -> *const c_char,
    osr_auth_code: unsafe extern "C" fn(H, *const c_char) -> *const c_char,
    osr_destroy: unsafe extern "C" fn(H),
}

/// GDAL încărcat în proces.
pub struct Gdal {
    api: Api,
    _lib: Library,
    /// fișierul bibliotecii (gdal*.dll / libgdal.so)
    pub path: PathBuf,
    /// versiunea GDAL (ex. „3.10.2”)
    pub version: String,
    /// GDAL nu e sigur pentru apeluri simultane pe același set de date;
    /// operațiile de deschidere se serializează
    lock: Mutex<()>,
}

// Pointerii de funcții din bibliotecă pot fi folosiți din orice fir.
unsafe impl Send for Gdal {}
unsafe impl Sync for Gdal {}

fn cstr(p: *const c_char) -> String {
    if p.is_null() {
        String::new()
    } else {
        unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
    }
}

impl Gdal {
    fn load(path: &Path) -> Result<Gdal> {
        let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
        #[cfg(windows)]
        let lib = {
            // dependențele (proj, geos, NCSEcw, lti_dsdk…) stau lângă gdal*.dll
            prepend_path(&dir);
            prepend_path(&dir.join("gdalplugins"));
            const LOAD_WITH_ALTERED_SEARCH_PATH: u32 = 0x8;
            unsafe { libloading::os::windows::Library::load_with_flags(path, LOAD_WITH_ALTERED_SEARCH_PATH) }
                .map(Library::from)?
        };
        #[cfg(not(windows))]
        let lib = unsafe { Library::new(path) }?;
        macro_rules! sym {
            ($n:literal) => {
                *unsafe { lib.get(concat!($n, "\0").as_bytes()) }
                    .map_err(|e| anyhow!("{}: {e}", $n))?
            };
        }
        let api = Api {
            all_register: sym!("GDALAllRegister"),
            version_info: sym!("GDALVersionInfo"),
            set_config: sym!("CPLSetConfigOption"),
            open_ex: sym!("GDALOpenEx"),
            close: sym!("GDALClose"),
            x_size: sym!("GDALGetRasterXSize"),
            y_size: sym!("GDALGetRasterYSize"),
            count: sym!("GDALGetRasterCount"),
            get_gt: sym!("GDALGetGeoTransform"),
            proj_ref: sym!("GDALGetProjectionRef"),
            get_band: sym!("GDALGetRasterBand"),
            band_type: sym!("GDALGetRasterDataType"),
            nodata: sym!("GDALGetRasterNoDataValue"),
            color_interp: sym!("GDALGetRasterColorInterpretation"),
            ds_io: sym!("GDALDatasetRasterIO"),
            driver_by_name: sym!("GDALGetDriverByName"),
            ds_driver: sym!("GDALGetDatasetDriver"),
            driver_short: sym!("GDALGetDriverShortName"),
            last_err: sym!("CPLGetLastErrorMsg"),
            err_reset: sym!("CPLErrorReset"),
            osr_new: sym!("OSRNewSpatialReference"),
            osr_auto_epsg: sym!("OSRAutoIdentifyEPSG"),
            osr_auth_name: sym!("OSRGetAuthorityName"),
            osr_auth_code: sym!("OSRGetAuthorityCode"),
            osr_destroy: sym!("OSRDestroySpatialReference"),
        };
        // pluginurile (ECW, MrSID) și datele GDAL / PROJ ale instalării găsite
        let root = if dir.file_name().is_some_and(|n| n.eq_ignore_ascii_case("bin")) {
            dir.parent().map(Path::to_path_buf).unwrap_or_else(|| dir.clone())
        } else {
            dir.clone()
        };
        let set = |k: &str, v: &Path| {
            if let (Ok(k), Ok(v)) = (CString::new(k), CString::new(v.to_string_lossy().as_bytes())) {
                unsafe { (api.set_config)(k.as_ptr(), v.as_ptr()) };
            }
        };
        if std::env::var_os("GDAL_DRIVER_PATH").is_none() {
            let plug = dir.join("gdalplugins");
            if plug.is_dir() {
                set("GDAL_DRIVER_PATH", &plug);
            }
        }
        if std::env::var_os("GDAL_DATA").is_none()
            && let Some(d) = [root.join("apps/gdal/share/gdal"), root.join("share/gdal")]
                .into_iter()
                .find(|d| d.join("gdalvrt.xsd").is_file() || d.is_dir())
        {
            set("GDAL_DATA", &d);
        }
        if std::env::var_os("PROJ_DATA").is_none() && std::env::var_os("PROJ_LIB").is_none() {
            let cands = [root.join("share/proj"), root.join("apps/proj/share/proj")];
            if let Some(d) = cands.iter().find(|d| d.join("proj.db").is_file()) {
                // OSRSetPROJSearchPaths există din GDAL 3.0
                if let Ok(f) = unsafe { lib.get::<unsafe extern "C" fn(*const *const c_char)>(b"OSRSetPROJSearchPaths\0") } {
                    if let Ok(c) = CString::new(d.to_string_lossy().as_bytes()) {
                        let arr = [c.as_ptr(), std::ptr::null()];
                        unsafe { f(arr.as_ptr()) };
                    }
                }
            }
        }
        // fără mesaje GDAL în consolă: erorile se citesc cu CPLGetLastErrorMsg
        if let (Ok(push), Ok(quiet)) = (
            unsafe { lib.get::<unsafe extern "C" fn(*const c_void)>(b"CPLPushErrorHandler\0") },
            unsafe { lib.get::<*const c_void>(b"CPLQuietErrorHandler\0") },
        ) {
            unsafe { push(*quiet) };
        }
        unsafe { (api.all_register)() };
        let version = cstr(unsafe { (api.version_info)(c"RELEASE_NAME".as_ptr()) });
        Ok(Gdal {
            api,
            _lib: lib,
            path: path.to_path_buf(),
            version,
            lock: Mutex::new(()),
        })
    }

    /// Driverul cu numele dat este disponibil (inclusiv ca plugin).
    pub fn has_driver(&self, name: &str) -> bool {
        CString::new(name).is_ok_and(|c| !unsafe { (self.api.driver_by_name)(c.as_ptr()) }.is_null())
    }

    /// Starea driverelor relevante: (nume afișat, disponibil).
    pub fn driver_status(&self) -> Vec<(&'static str, bool)> {
        DRIVERS.iter().map(|(d, label)| (*label, self.has_driver(d))).collect()
    }

    fn last_error(&self) -> String {
        cstr(unsafe { (self.api.last_err)() })
    }

    /// Sistemul de coordonate din WKT-ul dat de GDAL.
    fn crs_from_wkt(&self, wkt: &str) -> Option<Crs> {
        if wkt.trim().is_empty() {
            return None;
        }
        let c = CString::new(wkt).ok()?;
        let srs = unsafe { (self.api.osr_new)(c.as_ptr()) };
        let mut epsg = None;
        if !srs.is_null() {
            unsafe { (self.api.osr_auto_epsg)(srs) };
            let name = cstr(unsafe { (self.api.osr_auth_name)(srs, std::ptr::null()) });
            let code = cstr(unsafe { (self.api.osr_auth_code)(srs, std::ptr::null()) });
            if name.eq_ignore_ascii_case("EPSG") {
                epsg = code.parse::<u32>().ok();
            }
            unsafe { (self.api.osr_destroy)(srs) };
        }
        if let Some(code) = epsg
            && let Ok(mut c) = Crs::from_epsg(code)
        {
            c.wkt = Some(wkt.to_string());
            return Some(c);
        }
        Crs::from_wkt(wkt).ok()
    }
}

#[cfg(windows)]
fn prepend_path(dir: &Path) {
    if !dir.is_dir() {
        return;
    }
    let cur = std::env::var_os("PATH").unwrap_or_default();
    if std::env::split_paths(&cur).any(|p| p == dir) {
        return;
    }
    let mut v = vec![dir.to_path_buf()];
    v.extend(std::env::split_paths(&cur));
    if let Ok(p) = std::env::join_paths(v) {
        // SetEnvironmentVariableW: încărcătorul de DLL-uri citește PATH-ul procesului
        unsafe { std::env::set_var("PATH", p) };
    }
}

static GDAL: Mutex<Option<Arc<Gdal>>> = Mutex::new(None);
static HINT: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Dosarul (sau fișierul) GDAL ales de utilizator; se încearcă primul.
pub fn set_hint(p: Option<PathBuf>) {
    *HINT.lock().unwrap() = p.filter(|p| !p.as_os_str().is_empty());
}

/// GDAL deja încărcat, fără să-l caute.
pub fn loaded() -> Option<Arc<Gdal>> {
    GDAL.lock().unwrap().clone()
}

fn is_gdal_lib(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    if cfg!(windows) {
        // gdal.dll, gdal310.dll, gdal-36.dll (nu pluginurile gdal_*.dll)
        n.ends_with(".dll")
            && n.starts_with("gdal")
            && n[4..n.len() - 4].chars().all(|c| c.is_ascii_digit() || c == '-')
    } else {
        n.starts_with("libgdal") && (n.contains(".so") || n.ends_with(".dylib"))
    }
}

fn libs_in(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file() && p.file_name().is_some_and(|n| is_gdal_lib(&n.to_string_lossy())))
        .collect();
    // cel mai nou întâi (gdal311.dll înaintea gdal310.dll)
    v.sort();
    v.reverse();
    v
}

/// Locurile în care se caută GDAL, în ordine.
pub fn candidates() -> Vec<PathBuf> {
    let mut out = vec![];
    let add_dir = |d: &Path, out: &mut Vec<PathBuf>| {
        out.extend(libs_in(d));
        out.extend(libs_in(&d.join("bin")));
    };
    let from = |p: PathBuf, out: &mut Vec<PathBuf>| {
        if p.is_file() {
            out.push(p);
        } else if p.is_dir() {
            add_dir(&p, out);
        }
    };
    if let Some(h) = HINT.lock().unwrap().clone() {
        from(h, &mut out);
    }
    if let Some(e) = std::env::var_os("CADASTRU_GDAL") {
        from(PathBuf::from(e), &mut out);
    }
    #[cfg(windows)]
    {
        // QGIS (cel mai nou întâi), OSGeo4W, GISInternals
        let mut roots = vec![];
        for pf in ["ProgramFiles", "ProgramW6432", "ProgramFiles(x86)"] {
            if let Some(p) = std::env::var_os(pf) {
                let p = PathBuf::from(p);
                let mut q: Vec<PathBuf> = std::fs::read_dir(&p)
                    .into_iter()
                    .flatten()
                    .flatten()
                    .map(|e| e.path())
                    .filter(|d| {
                        d.file_name()
                            .is_some_and(|n| n.to_string_lossy().to_ascii_lowercase().starts_with("qgis"))
                    })
                    .collect();
                q.sort_by_key(|d| std::cmp::Reverse(version_key(d)));
                roots.extend(q);
                roots.push(p.join("GDAL"));
            }
        }
        for d in ["C:\\OSGeo4W", "C:\\OSGeo4W64"] {
            roots.push(PathBuf::from(d));
        }
        for r in roots {
            from(r, &mut out);
        }
        if let Some(path) = std::env::var_os("PATH") {
            for d in std::env::split_paths(&path) {
                out.extend(libs_in(&d));
            }
        }
    }
    #[cfg(not(windows))]
    {
        for n in ["libgdal.so", "libgdal.dylib", "/Applications/QGIS.app/Contents/Frameworks/libgdal.dylib"] {
            out.push(PathBuf::from(n));
        }
    }
    let mut seen = std::collections::HashSet::new();
    out.retain(|p| seen.insert(p.clone()));
    out
}

/// „QGIS 3.40.5” → [3, 40, 5] pentru ordonare.
#[allow(dead_code)]
fn version_key(p: &Path) -> Vec<u32> {
    p.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default()
        .split(|c: char| !c.is_ascii_digit())
        .filter_map(|s| s.parse().ok())
        .collect()
}

/// Încarcă GDAL (o singură dată). Întoarce o eroare care spune ce s-a încercat.
pub fn get() -> Result<Arc<Gdal>> {
    let mut g = GDAL.lock().unwrap();
    if let Some(x) = &*g {
        return Ok(x.clone());
    }
    let mut errs = vec![];
    for c in candidates() {
        match Gdal::load(&c) {
            Ok(x) => {
                let x = Arc::new(x);
                *g = Some(x.clone());
                return Ok(x);
            }
            Err(e) => {
                if c.is_absolute() {
                    errs.push(format!("{}: {e}", c.display()));
                }
            }
        }
    }
    let mut msg = tr!(
        "Nu am găsit GDAL pe acest calculator. Pentru ECW și MrSID instalați QGIS (gratuit, qgis.org) sau alegeți dosarul „bin” al unei instalări GDAL / OSGeo4W din Instrumente » Formate ECW / MrSID (GDAL)."
    )
    .to_string();
    if !errs.is_empty() {
        msg.push_str("\n");
        msg.push_str(&errs.join("\n"));
    }
    bail!(msg)
}

/// Extensia este citită prin GDAL.
pub fn handles_ext(ext: &str) -> bool {
    EXTS.iter().any(|(e, _)| *e == ext)
}

/// Un raster deschis prin GDAL, citit pe ferestre.
pub struct GdalRaster {
    gdal: Arc<Gdal>,
    ds: Mutex<H>,
    width: usize,
    height: usize,
    /// benzile citite (1-based), fără banda alfa
    bands: Vec<c_int>,
    dtype: c_int,
    gt: [f64; 6],
    nodata: Option<f64>,
    crs: Option<Crs>,
    driver: String,
}

unsafe impl Send for GdalRaster {}
unsafe impl Sync for GdalRaster {}

impl Drop for GdalRaster {
    fn drop(&mut self) {
        let h = *self.ds.lock().unwrap();
        if !h.is_null() {
            unsafe { (self.gdal.api.close)(h) };
        }
    }
}

// tipurile de date GDAL
const GDT_BYTE: c_int = 1;
const GDT_UINT16: c_int = 2;
const GDT_INT16: c_int = 3;
const GDT_UINT32: c_int = 4;
const GDT_INT32: c_int = 5;
const GDT_FLOAT32: c_int = 6;
const GDT_FLOAT64: c_int = 7;
const GDT_INT8: c_int = 14;
const GCI_ALPHA: c_int = 6;

impl RasterSource for GdalRaster {
    fn size(&self) -> (usize, usize) {
        (self.width, self.height)
    }
    fn geo_transform(&self) -> [f64; 6] {
        self.gt
    }
    fn driver(&self) -> String {
        self.driver.clone()
    }
    fn read_window(&self, col: usize, row: usize, w: usize, h: usize, bw: usize, bh: usize) -> Result<Raster> {
        let col = col.min(self.width.saturating_sub(1));
        let row = row.min(self.height.saturating_sub(1));
        let w = w.clamp(1, self.width - col);
        let h = h.clamp(1, self.height - row);
        let (bw, bh) = (bw.clamp(1, 16384), bh.clamp(1, 16384));
        let n = bw * bh;
        let nb = self.bands.len();
        macro_rules! io {
            ($t:ty, $gdt:expr, $var:ident) => {{
                let mut buf: Vec<$t> = vec![<$t>::default(); n * nb];
                let ds = self.ds.lock().unwrap();
                unsafe { (self.gdal.api.err_reset)() };
                let err = unsafe {
                    (self.gdal.api.ds_io)(
                        *ds,
                        0, // GF_Read
                        col as c_int,
                        row as c_int,
                        w as c_int,
                        h as c_int,
                        buf.as_mut_ptr() as *mut c_void,
                        bw as c_int,
                        bh as c_int,
                        $gdt,
                        nb as c_int,
                        self.bands.as_ptr(),
                        0,
                        0,
                        0,
                    )
                };
                drop(ds);
                if err >= 3 {
                    bail!(tr!("Eroare la citirea rasterului: {}", self.gdal.last_error()));
                }
                buf.chunks(n).map(|c| BandData::$var(c.to_vec())).collect::<Vec<_>>()
            }};
        }
        let bands = match self.dtype {
            GDT_BYTE => io!(u8, GDT_BYTE, U8),
            GDT_INT8 => io!(i8, GDT_INT8, I8),
            GDT_UINT16 => io!(u16, GDT_UINT16, U16),
            GDT_INT16 => io!(i16, GDT_INT16, I16),
            GDT_UINT32 => io!(u32, GDT_UINT32, U32),
            GDT_INT32 => io!(i32, GDT_INT32, I32),
            GDT_FLOAT32 => io!(f32, GDT_FLOAT32, F32),
            _ => io!(f64, GDT_FLOAT64, F64),
        };
        let (sx, sy) = (w as f64 / bw as f64, h as f64 / bh as f64);
        let g = self.gt;
        let (x0, y0) = (
            g[0] + col as f64 * g[1] + row as f64 * g[2],
            g[3] + col as f64 * g[4] + row as f64 * g[5],
        );
        Ok(Raster {
            name: String::new(),
            source: None,
            width: bw,
            height: bh,
            bands,
            gt: [x0, g[1] * sx, g[2] * sy, y0, g[4] * sx, g[5] * sy],
            nodata: self.nodata,
            crs: self.crs.clone(),
            full: None,
        })
    }
}

/// Deschide un raster prin GDAL: întoarce imaginea de ansamblu (max.
/// [`OVERVIEW_MAX`] px pe latură), legată de fișier pentru detaliu.
pub fn open(path: &Path) -> Result<Raster> {
    let gdal = get()?;
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    if let Some((_, drv)) = EXTS.iter().find(|(e, _)| *e == ext)
        && !drv.iter().any(|d| gdal.has_driver(d))
    {
        bail!(
            "{}",
            match ext.as_str() {
                "ecw" => tr!("GDAL-ul găsit ({}) nu are driverul ECW. Instalați QGIS (instalatorul complet include citirea ECW) sau, în OSGeo4W, pachetul gdal-ecw.", gdal.path.display()),
                "sid" => tr!("GDAL-ul găsit ({}) nu are driverul MrSID. În OSGeo4W instalați pachetul gdal-mrsid (citire gratuită).", gdal.path.display()),
                _ => tr!("GDAL-ul găsit ({}) nu are niciun driver JPEG 2000.", gdal.path.display()),
            }
        );
    }
    let c = CString::new(path.to_string_lossy().as_bytes())?;
    let ds = {
        let _g = gdal.lock.lock().unwrap();
        unsafe { (gdal.api.err_reset)() };
        // GDAL_OF_RASTER | GDAL_OF_READONLY | GDAL_OF_VERBOSE_ERROR
        unsafe { (gdal.api.open_ex)(c.as_ptr(), 0x02 | 0x40, std::ptr::null(), std::ptr::null(), std::ptr::null()) }
    };
    if ds.is_null() {
        let e = gdal.last_error();
        bail!(tr!("GDAL nu a putut deschide fișierul: {}", if e.is_empty() { "?".into() } else { e }));
    }
    let api = &gdal.api;
    let (w, h, nb) = unsafe { ((api.x_size)(ds), (api.y_size)(ds), (api.count)(ds)) };
    let driver = cstr(unsafe { (api.driver_short)((api.ds_driver)(ds)) });
    if w <= 0 || h <= 0 || nb <= 0 {
        unsafe { (api.close)(ds) };
        bail!(tr!("Rasterul nu are benzi sau are dimensiune nulă."));
    }
    let mut gt = [0.0, 1.0, 0.0, 0.0, 0.0, -1.0];
    unsafe { (api.get_gt)(ds, gt.as_mut_ptr()) };
    let wkt = cstr(unsafe { (api.proj_ref)(ds) });
    let mut bands = vec![];
    let mut dtype = 0;
    let mut nodata = None;
    for b in 1..=nb {
        let hb = unsafe { (api.get_band)(ds, b) };
        if hb.is_null() {
            continue;
        }
        // banda alfa nu intră în afișare (benzile RGB rămân 1-3)
        if nb > 1 && unsafe { (api.color_interp)(hb) } == GCI_ALPHA {
            continue;
        }
        let t = unsafe { (api.band_type)(hb) };
        dtype = if bands.is_empty() || dtype == t { t } else { GDT_FLOAT64 };
        if bands.is_empty() {
            let mut ok: c_int = 0;
            let v = unsafe { (api.nodata)(hb, &mut ok) };
            if ok != 0 {
                nodata = Some(v);
            }
        }
        bands.push(b);
    }
    if !matches!(dtype, GDT_BYTE | GDT_INT8 | GDT_UINT16 | GDT_INT16 | GDT_UINT32 | GDT_INT32 | GDT_FLOAT32) {
        dtype = GDT_FLOAT64;
    }
    let crs = gdal.crs_from_wkt(&wkt);
    let src = Arc::new(GdalRaster {
        gdal: gdal.clone(),
        ds: Mutex::new(ds),
        width: w as usize,
        height: h as usize,
        bands,
        dtype,
        gt,
        nodata,
        crs,
        driver,
    });
    let (w, h) = (w as usize, h as usize);
    let k = (w.max(h) as f64 / OVERVIEW_MAX as f64).max(1.0);
    let (bw, bh) = (((w as f64 / k).round() as usize).max(1), ((h as f64 / k).round() as usize).max(1));
    let mut r = src.read_window(0, 0, w, h, bw, bh)?;
    if k > 1.0 {
        r.full = Some(FullSource(src));
    }
    r.source = Some(path.to_path_buf());
    r.name = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    Ok(r)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_gdal_library_names() {
        if cfg!(windows) {
            assert!(is_gdal_lib("gdal310.dll"));
            assert!(is_gdal_lib("gdal.dll"));
            assert!(!is_gdal_lib("gdal_ECW_JP2ECW.dll"));
            assert!(!is_gdal_lib("gdal_MrSID.dll"));
        } else {
            assert!(is_gdal_lib("libgdal.so.36"));
            assert!(!is_gdal_lib("libgeos.so"));
        }
        assert!(handles_ext("ecw") && handles_ext("sid") && handles_ext("jp2"));
        assert!(!handles_ext("tif"));
    }

    /// Rulează doar cu un GDAL disponibil și un fișier de test:
    /// `CADASTRU_GDAL=… CADASTRU_TEST_JP2=… cargo test -- --ignored`
    #[test]
    #[ignore]
    fn reads_overview_and_windows() {
        let Some(p) = std::env::var_os("CADASTRU_TEST_JP2") else { return };
        let r = open(Path::new(&p)).unwrap();
        assert!(r.width.max(r.height) <= OVERVIEW_MAX);
        let f = r.full.as_ref().expect("raster mare: sursă pe ferestre");
        let (w, h) = f.0.size();
        assert!(w > r.width);
        // o fereastră la rezoluție completă are geotransformarea originală
        let win = f.0.read_window(100, 200, 64, 32, 64, 32).unwrap();
        let g = f.0.geo_transform();
        assert_eq!((win.width, win.height), (64, 32));
        assert!((win.gt[1] - g[1]).abs() < 1e-12);
        assert!((win.gt[0] - (g[0] + 100.0 * g[1])).abs() < 1e-9);
        assert!((win.gt[3] - (g[3] + 200.0 * g[5])).abs() < 1e-9);
        // scrierea în ECW este refuzată
        assert!(crate::formats::write_raster(&win, Path::new("/tmp/x.ecw"), &Default::default()).is_err());
        assert!(crate::formats::write_raster(&r, Path::new("/tmp/x.tif"), &Default::default()).is_err());
        let _ = h;
    }
}
