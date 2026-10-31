//! Hărți de fundal (basemap) din tile-uri XYZ / TMS (Web Mercator, EPSG:3857).
//!
//! Tile-urile se descarcă în fundal (mai multe fire), se păstrează într-un cache pe disc
//! și se desenează ca plase de triunghiuri reproiectate în sistemul hărții (de ex. Stereo 70),
//! deci se suprapun corect cu datele indiferent de proiecție.

use cadastru::crs::{Crs, Transformer};
use eframe::egui::{self, Color32, Mesh, Pos2, Rect, Shape, TextureHandle, Vec2};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;
use cadastru::i18n::tr;

/// Un furnizor de hărți de fundal.
pub struct Provider {
    pub id: &'static str,
    pub group: &'static str,
    pub name: &'static str,
    /// Șabloane URL, desenate în ordine (al doilea strat = etichete transparente, pentru hărți mixte).
    /// {z} {x} {y}, {s} = subdomeniu, {q} = quadkey (Bing)
    pub layers: &'static [&'static str],
    pub subdomains: &'static [&'static str],
    pub max_zoom: u8,
    pub attribution: &'static str,
}

const ABC: &[&str] = &["a", "b", "c"];
const ABCD: &[&str] = &["a", "b", "c", "d"];
const G0123: &[&str] = &["0", "1", "2", "3"];

macro_rules! esri {
    ($s:literal) => {
        concat!(
            "https://server.arcgisonline.com/ArcGIS/rest/services/",
            $s,
            "/MapServer/tile/{z}/{y}/{x}"
        )
    };
}

pub const PROVIDERS: &[Provider] = &[
    // ---- OpenStreetMap
    Provider {
        id: "osm",
        group: "OpenStreetMap",
        name: "OpenStreetMap Standard",
        layers: &["https://tile.openstreetmap.org/{z}/{x}/{y}.png"],
        subdomains: &[],
        max_zoom: 19,
        attribution: "© OpenStreetMap contributors",
    },
    Provider {
        id: "osm-hot",
        group: "OpenStreetMap",
        name: "OpenStreetMap Humanitarian",
        layers: &["https://{s}.tile.openstreetmap.fr/hot/{z}/{x}/{y}.png"],
        subdomains: ABC,
        max_zoom: 19,
        attribution: "© OpenStreetMap contributors, Humanitarian OpenStreetMap Team",
    },
    Provider {
        id: "osm-fr",
        group: "OpenStreetMap",
        name: "OpenStreetMap France",
        layers: &["https://{s}.tile.openstreetmap.fr/osmfr/{z}/{x}/{y}.png"],
        subdomains: ABC,
        max_zoom: 20,
        attribution: "© OpenStreetMap France | © OpenStreetMap contributors",
    },
    Provider {
        id: "osm-de",
        group: "OpenStreetMap",
        name: "OpenStreetMap DE",
        layers: &["https://tile.openstreetmap.de/{z}/{x}/{y}.png"],
        subdomains: &[],
        max_zoom: 18,
        attribution: "© OpenStreetMap contributors",
    },
    Provider {
        id: "cyclosm",
        group: "OpenStreetMap",
        name: "CyclOSM (biciclete, relief)",
        layers: &["https://{s}.tile-cyclosm.openstreetmap.fr/cyclosm/{z}/{x}/{y}.png"],
        subdomains: ABC,
        max_zoom: 20,
        attribution: "CyclOSM | © OpenStreetMap contributors",
    },
    Provider {
        id: "opentopomap",
        group: "OpenStreetMap",
        name: "OpenTopoMap (topografică)",
        layers: &["https://{s}.tile.opentopomap.org/{z}/{x}/{y}.png"],
        subdomains: ABC,
        max_zoom: 17,
        attribution: "© OpenStreetMap contributors, SRTM | © OpenTopoMap (CC-BY-SA)",
    },
    // ---- Esri
    Provider {
        id: "esri-imagery",
        group: "Esri",
        name: "Esri World Imagery (satelit)",
        layers: &[esri!("World_Imagery")],
        subdomains: &[],
        max_zoom: 19,
        attribution: "© Esri — Esri, Maxar, Earthstar Geographics, GIS User Community",
    },
    Provider {
        id: "esri-hybrid",
        group: "Esri",
        name: "Esri Imagery Hybrid (satelit + etichete)",
        layers: &[
            esri!("World_Imagery"),
            esri!("Reference/World_Transportation"),
            esri!("Reference/World_Boundaries_and_Places"),
        ],
        subdomains: &[],
        max_zoom: 19,
        attribution: "© Esri — Esri, Maxar, Earthstar Geographics, HERE, Garmin, GIS User Community",
    },
    Provider {
        id: "esri-street",
        group: "Esri",
        name: "Esri World Street Map",
        layers: &[esri!("World_Street_Map")],
        subdomains: &[],
        max_zoom: 19,
        attribution: "© Esri — Esri, HERE, Garmin, USGS, © OpenStreetMap contributors",
    },
    Provider {
        id: "esri-topo",
        group: "Esri",
        name: "Esri World Topo Map (topografică)",
        layers: &[esri!("World_Topo_Map")],
        subdomains: &[],
        max_zoom: 19,
        attribution: "© Esri — Esri, HERE, Garmin, FAO, NOAA, USGS, © OpenStreetMap contributors",
    },
    Provider {
        id: "esri-natgeo",
        group: "Esri",
        name: "Esri National Geographic",
        layers: &[esri!("NatGeo_World_Map")],
        subdomains: &[],
        max_zoom: 16,
        attribution: "© Esri — National Geographic, Esri, Garmin, HERE, UNEP-WCMC, USGS, NASA",
    },
    Provider {
        id: "esri-terrain",
        group: "Esri",
        name: "Esri World Terrain (relief)",
        layers: &[esri!("World_Terrain_Base")],
        subdomains: &[],
        max_zoom: 13,
        attribution: "© Esri — Esri, USGS, NOAA",
    },
    Provider {
        id: "esri-shaded",
        group: "Esri",
        name: "Esri World Shaded Relief",
        layers: &[esri!("World_Shaded_Relief")],
        subdomains: &[],
        max_zoom: 13,
        attribution: "© Esri — Esri",
    },
    Provider {
        id: "esri-physical",
        group: "Esri",
        name: "Esri World Physical Map (fizică)",
        layers: &[esri!("World_Physical_Map")],
        subdomains: &[],
        max_zoom: 8,
        attribution: "© Esri — US National Park Service",
    },
    Provider {
        id: "esri-gray",
        group: "Esri",
        name: "Esri Light Gray Canvas",
        layers: &[
            esri!("Canvas/World_Light_Gray_Base"),
            esri!("Canvas/World_Light_Gray_Reference"),
        ],
        subdomains: &[],
        max_zoom: 16,
        attribution: "© Esri — Esri, HERE, Garmin, © OpenStreetMap contributors",
    },
    Provider {
        id: "esri-darkgray",
        group: "Esri",
        name: "Esri Dark Gray Canvas",
        layers: &[
            esri!("Canvas/World_Dark_Gray_Base"),
            esri!("Canvas/World_Dark_Gray_Reference"),
        ],
        subdomains: &[],
        max_zoom: 16,
        attribution: "© Esri — Esri, HERE, Garmin, © OpenStreetMap contributors",
    },
    // ---- Google
    Provider {
        id: "google-roads",
        group: "Google",
        name: "Google Hărți",
        layers: &["https://mt{s}.google.com/vt/lyrs=m&x={x}&y={y}&z={z}"],
        subdomains: G0123,
        max_zoom: 20,
        attribution: "© Google",
    },
    Provider {
        id: "google-satellite",
        group: "Google",
        name: "Google Satelit",
        layers: &["https://mt{s}.google.com/vt/lyrs=s&x={x}&y={y}&z={z}"],
        subdomains: G0123,
        max_zoom: 20,
        attribution: "© Google",
    },
    Provider {
        id: "google-hybrid",
        group: "Google",
        name: "Google Hibrid (satelit + etichete)",
        layers: &["https://mt{s}.google.com/vt/lyrs=y&x={x}&y={y}&z={z}"],
        subdomains: G0123,
        max_zoom: 20,
        attribution: "© Google",
    },
    Provider {
        id: "google-terrain",
        group: "Google",
        name: "Google Teren (relief)",
        layers: &["https://mt{s}.google.com/vt/lyrs=p&x={x}&y={y}&z={z}"],
        subdomains: G0123,
        max_zoom: 20,
        attribution: "© Google",
    },
    // ---- Bing
    Provider {
        id: "bing-aerial",
        group: "Bing",
        name: "Bing Aerial (satelit)",
        layers: &["https://ecn.t{s}.tiles.virtualearth.net/tiles/a{q}.jpeg?g=1"],
        subdomains: G0123,
        max_zoom: 19,
        attribution: "© Microsoft",
    },
    Provider {
        id: "bing-hybrid",
        group: "Bing",
        name: "Bing Hibrid (satelit + etichete)",
        layers: &["https://ecn.t{s}.tiles.virtualearth.net/tiles/h{q}.jpeg?g=1"],
        subdomains: G0123,
        max_zoom: 19,
        attribution: "© Microsoft",
    },
    // ---- CARTO
    Provider {
        id: "carto-positron",
        group: "CARTO",
        name: "CARTO Positron (deschisă)",
        layers: &["https://{s}.basemaps.cartocdn.com/light_all/{z}/{x}/{y}.png"],
        subdomains: ABCD,
        max_zoom: 20,
        attribution: "© OpenStreetMap contributors © CARTO",
    },
    Provider {
        id: "carto-dark",
        group: "CARTO",
        name: "CARTO Dark Matter (închisă)",
        layers: &["https://{s}.basemaps.cartocdn.com/dark_all/{z}/{x}/{y}.png"],
        subdomains: ABCD,
        max_zoom: 20,
        attribution: "© OpenStreetMap contributors © CARTO",
    },
    Provider {
        id: "carto-voyager",
        group: "CARTO",
        name: "CARTO Voyager",
        layers: &["https://{s}.basemaps.cartocdn.com/rastertiles/voyager/{z}/{x}/{y}.png"],
        subdomains: ABCD,
        max_zoom: 20,
        attribution: "© OpenStreetMap contributors © CARTO",
    },
    // ---- Altele
    Provider {
        id: "topplus",
        group: "Altele",
        name: "TopPlusOpen (BKG, topografică)",
        layers: &[
            "https://sgx.geodatenzentrum.de/wmts_topplus_open/tile/1.0.0/web/default/WEBMERCATOR/{z}/{y}/{x}.png",
        ],
        subdomains: &[],
        max_zoom: 18,
        attribution: "© BKG (dl-de/by-2-0)",
    },
    Provider {
        id: "s2cloudless",
        group: "Altele",
        name: "Sentinel-2 cloudless (EOX, satelit 10 m)",
        layers: &["https://tiles.maps.eox.at/wmts/1.0.0/s2cloudless-2020_3857/default/g/{z}/{y}/{x}.jpg"],
        subdomains: &[],
        max_zoom: 15,
        attribution: "Sentinel-2 cloudless — https://s2maps.eu by EOX IT Services GmbH (Copernicus Sentinel data 2020)",
    },
];

/// Sursa activă (din catalog sau definită de utilizator).
#[derive(Clone, Debug, PartialEq)]
pub struct Source {
    pub id: String,
    pub name: String,
    pub layers: Vec<String>,
    pub subdomains: Vec<String>,
    pub max_zoom: u8,
    pub attribution: String,
}

impl Source {
    pub fn from_id(id: &str) -> Option<Source> {
        PROVIDERS.iter().find(|p| p.id == id).map(|p| Source {
            id: p.id.into(),
            name: p.name.into(),
            layers: p.layers.iter().map(|s| s.to_string()).collect(),
            subdomains: p.subdomains.iter().map(|s| s.to_string()).collect(),
            max_zoom: p.max_zoom,
            attribution: p.attribution.into(),
        })
    }

    /// Șablon XYZ introdus de utilizator (de ex. https://server/{z}/{x}/{y}.png).
    pub fn custom(template: &str, max_zoom: u8) -> Option<Source> {
        let t = template.trim();
        if !(t.starts_with("http://") || t.starts_with("https://")) {
            return None;
        }
        let has_xyz = t.contains("{z}") && t.contains("{x}") && t.contains("{y}");
        if !has_xyz && !t.contains("{q}") {
            return None;
        }
        // id stabil pentru cache, derivat din șablon
        let mut h: u64 = 1469598103934665603;
        for b in t.bytes() {
            h = (h ^ b as u64).wrapping_mul(1099511628211);
        }
        Some(Source {
            id: format!("custom-{h:016x}"),
            name: tr!("Personalizat").into(),
            layers: vec![t.to_string()],
            subdomains: if t.contains("{s}") {
                ABC.iter().map(|s| s.to_string()).collect()
            } else {
                vec![]
            },
            max_zoom: max_zoom.clamp(1, 22),
            attribution: String::new(),
        })
    }

    fn url(&self, layer: usize, z: u8, x: u32, y: u32) -> String {
        let t = &self.layers[layer];
        let mut u = t
            .replace("{z}", &z.to_string())
            .replace("{x}", &x.to_string())
            .replace("{y}", &y.to_string())
            .replace("{-y}", &((1u32 << z) - 1 - y).to_string());
        if u.contains("{s}") && !self.subdomains.is_empty() {
            let s = &self.subdomains[(x as usize + y as usize) % self.subdomains.len()];
            u = u.replace("{s}", s);
        }
        if u.contains("{q}") {
            u = u.replace("{q}", &quadkey(z, x, y));
        }
        u
    }
}

fn quadkey(z: u8, x: u32, y: u32) -> String {
    let mut q = String::with_capacity(z as usize);
    for i in (1..=z).rev() {
        let mask = 1u32 << (i - 1);
        let mut d = b'0';
        if x & mask != 0 {
            d += 1;
        }
        if y & mask != 0 {
            d += 2;
        }
        q.push(d as char);
    }
    q
}

// ---------------------------------------------------------------------------
// Descărcare și cache

const MERC: f64 = 20037508.342789244;
const MAX_TILES: usize = 300;
const WORKERS: usize = 6;
const CACHE_DAYS: u64 = 30;

/// (strat, z, x, y)
type Key = (u8, u8, u32, u32);
/// vârfurile unui tile în sistemul hărții
type TileGeo = Arc<Vec<Option<[f64; 2]>>>;

struct Job {
    epoch: u64,
    key: Key,
    url: String,
    path: Option<PathBuf>,
}

struct Done {
    epoch: u64,
    key: Key,
    result: Result<egui::ColorImage, String>,
}

#[derive(Default)]
struct Queue {
    items: VecDeque<Job>,
}

struct Shared {
    q: Mutex<Queue>,
    cv: Condvar,
}

enum TileState {
    Loading,
    Ready(TextureHandle),
    Failed(f64),
}

pub struct Basemap {
    pub source: Option<Source>,
    pub opacity: f32,
    epoch: u64,
    tiles: HashMap<Key, (TileState, u64)>,
    meshes: HashMap<(u8, u32, u32), TileGeo>,
    crs_key: String,
    to_merc: Option<Transformer>,
    from_merc: Option<Transformer>,
    shared: Arc<Shared>,
    tx: Sender<Done>,
    rx: Receiver<Done>,
    started: bool,
    frame: u64,
    last_needed: Vec<Key>,
    pub loading: usize,
    pub ok: usize,
    pub errors: usize,
    pub last_error: Option<String>,
}

pub fn cache_dir() -> Option<PathBuf> {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from))
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))?;
    Some(base.join("Cadastru").join("tile-cache"))
}

/// Proxy: variabilele de mediu HTTPS_PROXY / HTTP_PROXY sau setările Windows (Internet Options).
fn proxy() -> Option<ureq::Proxy> {
    if let Some(p) = ureq::Proxy::try_from_env() {
        return Some(p);
    }
    system_proxy().and_then(|s| ureq::Proxy::new(&s).ok())
}

#[cfg(windows)]
fn system_proxy() -> Option<String> {
    use winreg::RegKey;
    use winreg::enums::HKEY_CURRENT_USER;
    let k = RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey("Software\\Microsoft\\Windows\\CurrentVersion\\Internet Settings")
        .ok()?;
    let enabled: u32 = k.get_value("ProxyEnable").ok()?;
    if enabled == 0 {
        return None;
    }
    let s: String = k.get_value("ProxyServer").ok()?;
    let pick = if s.contains('=') {
        let parts: Vec<&str> = s.split(';').map(|p| p.trim()).collect();
        parts
            .iter()
            .find_map(|p| p.strip_prefix("https="))
            .or_else(|| parts.iter().find_map(|p| p.strip_prefix("http=")))?
            .to_string()
    } else {
        s.trim().to_string()
    };
    if pick.is_empty() {
        return None;
    }
    Some(if pick.contains("://") {
        pick
    } else {
        format!("http://{pick}")
    })
}

#[cfg(not(windows))]
fn system_proxy() -> Option<String> {
    None
}

pub(crate) fn make_agent() -> ureq::Agent {
    use ureq::tls::{RootCerts, TlsConfig};
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(25)))
        .user_agent(concat!(
            "Cadastru/",
            env!("CARGO_PKG_VERSION"),
            " (aplicatie GIS desktop)"
        ))
        .proxy(proxy())
        .tls_config(
            TlsConfig::builder()
                .root_certs(RootCerts::PlatformVerifier)
                .build(),
        )
        .build()
        .into()
}

fn decode(bytes: &[u8]) -> Result<egui::ColorImage, String> {
    let img = image::load_from_memory(bytes).map_err(|e| tr!("imagine invalidă: {e}"))?;
    let rgba = img.to_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    Ok(egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw()))
}

fn fetch(agent: &ureq::Agent, job: &Job) -> Result<egui::ColorImage, String> {
    let fresh = |p: &PathBuf| {
        std::fs::metadata(p)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age < Duration::from_secs(CACHE_DAYS * 86400))
    };
    if let Some(p) = &job.path
        && fresh(p)
        && let Ok(b) = std::fs::read(p)
        && let Ok(img) = decode(&b)
    {
        return Ok(img);
    }
    let downloaded = (|| -> Result<Vec<u8>, String> {
        let mut resp = agent.get(&job.url).call().map_err(|e| e.to_string())?;
        resp.body_mut()
            .with_config()
            .limit(16 * 1024 * 1024)
            .read_to_vec()
            .map_err(|e| e.to_string())
    })();
    match downloaded {
        Ok(b) => {
            let img = decode(&b)?;
            if let Some(p) = &job.path
                && let Some(d) = p.parent()
                && std::fs::create_dir_all(d).is_ok()
            {
                let tmp = p.with_extension("tmp");
                if std::fs::write(&tmp, &b).is_ok() {
                    let _ = std::fs::rename(&tmp, p);
                }
            }
            Ok(img)
        }
        Err(e) => {
            // fără conexiune: folosește și o copie veche din cache
            if let Some(p) = &job.path
                && let Ok(b) = std::fs::read(p)
                && let Ok(img) = decode(&b)
            {
                return Ok(img);
            }
            Err(e)
        }
    }
}

/// Golește tile-urile mai vechi decât CACHE_DAYS*3 și ține cache-ul sub ~1 GB.
pub fn prune_cache() {
    std::thread::spawn(|| {
        let Some(root) = cache_dir() else {
            return;
        };
        let mut files: Vec<(std::time::SystemTime, u64, PathBuf)> = vec![];
        let mut stack = vec![root];
        while let Some(d) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&d) else {
                continue;
            };
            for e in rd.flatten() {
                let p = e.path();
                match e.metadata() {
                    Ok(m) if m.is_dir() => stack.push(p),
                    Ok(m) => files.push((
                        m.modified().unwrap_or(std::time::UNIX_EPOCH),
                        m.len(),
                        p,
                    )),
                    Err(_) => {}
                }
            }
        }
        files.sort();
        let mut total: u64 = files.iter().map(|f| f.1).sum();
        let limit = 1024 * 1024 * 1024;
        let old = Duration::from_secs(CACHE_DAYS * 3 * 86400);
        for (t, len, p) in files {
            let too_old = t.elapsed().is_ok_and(|a| a > old);
            if total > limit * 8 / 10 || too_old {
                if std::fs::remove_file(&p).is_ok() {
                    total = total.saturating_sub(len);
                }
            } else {
                break;
            }
        }
    });
}

/// Șterge tot cache-ul de tile-uri.
pub fn clear_cache() -> std::io::Result<()> {
    if let Some(d) = cache_dir()
        && d.exists()
    {
        std::fs::remove_dir_all(d)?;
    }
    Ok(())
}

impl Basemap {
    pub fn new(source: Option<Source>, opacity: f32) -> Basemap {
        let (tx, rx) = channel();
        Basemap {
            source,
            opacity,
            epoch: 1,
            tiles: HashMap::new(),
            meshes: HashMap::new(),
            crs_key: String::new(),
            to_merc: None,
            from_merc: None,
            shared: Arc::new(Shared {
                q: Mutex::new(Queue::default()),
                cv: Condvar::new(),
            }),
            tx,
            rx,
            started: false,
            frame: 0,
            last_needed: vec![],
            loading: 0,
            ok: 0,
            errors: 0,
            last_error: None,
        }
    }

    pub fn set_source(&mut self, s: Option<Source>) {
        if self.source == s {
            return;
        }
        self.source = s;
        self.epoch += 1;
        self.tiles.clear();
        self.last_needed.clear();
        self.ok = 0;
        self.errors = 0;
        self.last_error = None;
        if let Ok(mut q) = self.shared.q.lock() {
            q.items.clear();
        }
    }

    fn start(&mut self, ctx: &egui::Context) {
        if self.started {
            return;
        }
        self.started = true;
        prune_cache();
        let agent = make_agent();
        for _ in 0..WORKERS {
            let shared = self.shared.clone();
            let tx = self.tx.clone();
            let ctx = ctx.clone();
            let agent = agent.clone();
            std::thread::spawn(move || {
                loop {
                    let job = {
                        let Ok(mut q) = shared.q.lock() else {
                            return;
                        };
                        loop {
                            if let Some(j) = q.items.pop_front() {
                                break j;
                            }
                            q = match shared.cv.wait(q) {
                                Ok(g) => g,
                                Err(_) => return,
                            };
                        }
                    };
                    let result = fetch(&agent, &job);
                    if tx
                        .send(Done {
                            epoch: job.epoch,
                            key: job.key,
                            result,
                        })
                        .is_err()
                    {
                        return;
                    }
                    ctx.request_repaint();
                }
            });
        }
    }

    fn receive(&mut self, ctx: &egui::Context, now: f64) {
        while let Ok(d) = self.rx.try_recv() {
            if d.epoch != self.epoch {
                continue;
            }
            match d.result {
                Ok(img) => {
                    let name = format!("bm{}-{}-{}-{}-{}", self.epoch, d.key.0, d.key.1, d.key.2, d.key.3);
                    let tex = ctx.load_texture(name, img, egui::TextureOptions::LINEAR);
                    self.tiles
                        .insert(d.key, (TileState::Ready(tex), self.frame));
                    self.ok += 1;
                }
                Err(e) => {
                    self.tiles.insert(d.key, (TileState::Failed(now), self.frame));
                    self.errors += 1;
                    self.last_error = Some(e);
                }
            }
        }
    }

    fn ensure_transformers(&mut self, map_crs: &Crs) {
        let key = format!(
            "{}|{}",
            map_crs.to_user_string(),
            cadastru::crs::transdatro_enabled()
        );
        if key == self.crs_key {
            return;
        }
        self.crs_key = key;
        self.meshes.clear();
        let merc = Crs::from_epsg(3857).expect("EPSG:3857");
        self.to_merc = Transformer::new(map_crs, &merc).ok();
        self.from_merc = Transformer::new(&merc, map_crs).ok();
    }

    /// Geometria tile-ului în sistemul hărții: grilă (n+1)x(n+1) de vârfuri.
    fn mesh_coords(&mut self, z: u8, x: u32, y: u32) -> Option<TileGeo> {
        if let Some(m) = self.meshes.get(&(z, x, y)) {
            return Some(m.clone());
        }
        let t = self.from_merc.as_ref()?;
        let n = grid_n(z);
        let size = 2.0 * MERC / (1u64 << z) as f64;
        let x0 = -MERC + x as f64 * size;
        let y0 = MERC - y as f64 * size;
        let mut v = Vec::with_capacity((n + 1) * (n + 1));
        for j in 0..=n {
            for i in 0..=n {
                let mx = x0 + size * i as f64 / n as f64;
                let my = y0 - size * j as f64 / n as f64;
                v.push(t.xy(mx, my).map(|(a, b)| [a, b]));
            }
        }
        let a = Arc::new(v);
        if self.meshes.len() > 4000 {
            self.meshes.clear();
        }
        self.meshes.insert((z, x, y), a.clone());
        Some(a)
    }
}

fn grid_n(z: u8) -> usize {
    match z {
        0..=4 => 24,
        5..=7 => 12,
        _ => 8,
    }
}

/// Parametrii vederii necesari desenării.
pub struct ViewParams {
    pub rect: Rect,
    pub cx: f64,
    pub cy: f64,
    pub scale: f64,
    pub ppp: f32,
    pub now: f64,
}

impl ViewParams {
    #[inline]
    fn w2s(&self, x: f64, y: f64) -> Pos2 {
        let c = self.rect.center();
        Pos2::new(
            c.x + ((x - self.cx) / self.scale) as f32,
            c.y - ((y - self.cy) / self.scale) as f32,
        )
    }
    fn s2w(&self, p: Pos2) -> (f64, f64) {
        let c = self.rect.center();
        (
            self.cx + (p.x - c.x) as f64 * self.scale,
            self.cy - (p.y - c.y) as f64 * self.scale,
        )
    }
}

impl Basemap {
    /// Desenează harta de fundal. Întoarce true dacă mai sunt tile-uri în curs de încărcare.
    pub fn draw(
        &mut self,
        ctx: &egui::Context,
        painter: &egui::Painter,
        map_crs: Option<&Crs>,
        v: &ViewParams,
    ) -> bool {
        let Some(src) = self.source.clone() else {
            return false;
        };
        let Some(map_crs) = map_crs else {
            return false;
        };
        self.start(ctx);
        self.frame += 1;
        self.receive(ctx, v.now);
        self.ensure_transformers(map_crs);
        let Some(to_merc) = self.to_merc.as_ref() else {
            return false;
        };

        // 1. nivelul de zoom: rezoluția Web Mercator (unități/pixel fizic) în centrul vederii
        let c = v.rect.center();
        let probe = |dx: f32, dy: f32| {
            let (x, y) = v.s2w(c + Vec2::new(dx, dy));
            to_merc.xy(x, y)
        };
        let res = match (probe(0.0, 0.0), probe(64.0, 0.0), probe(0.0, 64.0)) {
            (Some(a), Some(b), Some(d)) => {
                let rx = ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt();
                let ry = ((d.0 - a.0).powi(2) + (d.1 - a.1).powi(2)).sqrt();
                (rx + ry) / 2.0 / 64.0 / v.ppp as f64
            }
            _ => return false,
        };
        if !(res > 0.0 && res.is_finite()) {
            return false;
        }
        let zf = (2.0 * MERC / 256.0 / res).log2();
        let mut z = (zf.round() as i32).clamp(0, src.max_zoom as i32) as u8;

        // 2. extinderea vizibilă în Web Mercator
        let mut mb = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
        let k = 10;
        for i in 0..=k {
            for j in 0..=k {
                if i != 0 && i != k && j != 0 && j != k && (i + j) % 3 != 0 {
                    continue;
                }
                let p = Pos2::new(
                    v.rect.left() + v.rect.width() * i as f32 / k as f32,
                    v.rect.top() + v.rect.height() * j as f32 / k as f32,
                );
                let (x, y) = v.s2w(p);
                if let Some((mx, my)) = to_merc.xy(x, y)
                    && mx.is_finite()
                    && my.is_finite()
                {
                    mb[0] = mb[0].min(mx);
                    mb[1] = mb[1].min(my);
                    mb[2] = mb[2].max(mx);
                    mb[3] = mb[3].max(my);
                }
            }
        }
        if mb[0] > mb[2] {
            return false;
        }
        let clampm = |v: f64| v.clamp(-MERC, MERC - 1e-6);
        let (mut tx0, mut ty0, mut tx1, mut ty1);
        loop {
            let size = 2.0 * MERC / (1u64 << z) as f64;
            tx0 = ((clampm(mb[0]) + MERC) / size).floor() as u32;
            tx1 = ((clampm(mb[2]) + MERC) / size).floor() as u32;
            ty0 = ((MERC - clampm(mb[3])) / size).floor() as u32;
            ty1 = ((MERC - clampm(mb[1])) / size).floor() as u32;
            let count = (tx1 - tx0 + 1) as usize * (ty1 - ty0 + 1) as usize;
            if count <= MAX_TILES || z == 0 {
                break;
            }
            z -= 1;
        }

        // 3. lista tile-urilor necesare, de la centru spre margini
        let ctile = {
            let size = 2.0 * MERC / (1u64 << z) as f64;
            match probe(0.0, 0.0) {
                Some((mx, my)) => (
                    (clampm(mx) + MERC) / size - 0.5,
                    (MERC - clampm(my)) / size - 0.5,
                ),
                None => (tx0 as f64, ty0 as f64),
            }
        };
        let mut coords: Vec<(u32, u32)> = vec![];
        for ty in ty0..=ty1 {
            for tx in tx0..=tx1 {
                coords.push((tx, ty));
            }
        }
        coords.sort_by(|a, b| {
            let da = (a.0 as f64 - ctile.0).powi(2) + (a.1 as f64 - ctile.1).powi(2);
            let db = (b.0 as f64 - ctile.0).powi(2) + (b.1 as f64 - ctile.1).powi(2);
            da.total_cmp(&db)
        });
        let nl = src.layers.len().min(8) as u8;
        let mut needed: Vec<Key> = Vec::with_capacity(coords.len() * nl as usize);
        for &(x, y) in &coords {
            for l in 0..nl {
                needed.push((l, z, x, y));
            }
        }
        self.request(&src, &needed, v.now);

        // 4. desenare (strat cu strat; tile lipsă -> părinte deja încărcat, decupat)
        let alpha = (self.opacity.clamp(0.0, 1.0) * 255.0).round() as u8;
        let tint = Color32::from_rgba_premultiplied(alpha, alpha, alpha, alpha);
        let limit = v.rect.width().max(v.rect.height()) * 6.0;
        let frame = self.frame;
        for l in 0..nl {
            for &(x, y) in &coords {
                let Some(geo) = self.mesh_coords(z, x, y) else {
                    continue;
                };
                // textura: tile-ul sau cel mai apropiat strămoș încărcat
                let mut found: Option<(egui::TextureId, [f32; 3])> = None;
                for dz in 0..=8u8 {
                    if dz > z {
                        break;
                    }
                    let k = (l, z - dz, x >> dz, y >> dz);
                    if let Some((TileState::Ready(t), used)) = self.tiles.get_mut(&k) {
                        *used = frame;
                        let f = 1.0 / (1u32 << dz) as f32;
                        let mask = (1u32 << dz) - 1;
                        found = Some((t.id(), [(x & mask) as f32 * f, (y & mask) as f32 * f, f]));
                        break;
                    }
                }
                let Some((tex, [u0, v0, f])) = found else {
                    continue;
                };
                let n = grid_n(z);
                let mut mesh = Mesh::with_texture(tex);
                let mut ok = Vec::with_capacity(geo.len());
                let mut any_in = false;
                for j in 0..=n {
                    for i in 0..=n {
                        let p = geo[j * (n + 1) + i];
                        let (pos, good) = match p {
                            Some([a, b]) => {
                                let s = v.w2s(a, b);
                                let good = s.x.is_finite()
                                    && s.y.is_finite()
                                    && (s.x - c.x).abs() < limit
                                    && (s.y - c.y).abs() < limit;
                                (s, good)
                            }
                            None => (Pos2::ZERO, false),
                        };
                        if good && v.rect.expand(64.0).contains(pos) {
                            any_in = true;
                        }
                        ok.push(good);
                        mesh.vertices.push(egui::epaint::Vertex {
                            pos,
                            uv: Pos2::new(
                                u0 + f * i as f32 / n as f32,
                                v0 + f * j as f32 / n as f32,
                            ),
                            color: tint,
                        });
                    }
                }
                if !any_in {
                    // tile mare care acoperă tot ecranul fără vârfuri în interior
                    let r = geo
                        .iter()
                        .flatten()
                        .fold(Rect::NOTHING, |r, p| r.union(Rect::from_pos(v.w2s(p[0], p[1]))));
                    if !r.intersects(v.rect) {
                        continue;
                    }
                }
                for j in 0..n {
                    for i in 0..n {
                        let a = (j * (n + 1) + i) as u32;
                        let b = a + 1;
                        let cc = a + (n + 1) as u32;
                        let d = cc + 1;
                        if ok[a as usize] && ok[b as usize] && ok[cc as usize] && ok[d as usize] {
                            mesh.add_triangle(a, b, d);
                            mesh.add_triangle(a, d, cc);
                        }
                    }
                }
                if !mesh.indices.is_empty() {
                    painter.add(Shape::mesh(mesh));
                }
            }
        }
        self.evict();
        self.loading = self
            .tiles
            .values()
            .filter(|(s, _)| matches!(s, TileState::Loading))
            .count();
        self.loading > 0
    }

    /// Pune în coadă tile-urile lipsă (în ordinea priorității) și scoate cererile care nu mai sunt necesare.
    fn request(&mut self, src: &Source, needed: &[Key], now: f64) {
        if needed == self.last_needed.as_slice() {
            // reîncearcă tile-urile eșuate după 20 s
            let retry = needed.iter().any(|k| {
                matches!(self.tiles.get(k), Some((TileState::Failed(t), _)) if now - t > 20.0)
            });
            if !retry {
                return;
            }
        }
        self.last_needed = needed.to_vec();
        let Ok(mut q) = self.shared.q.lock() else {
            return;
        };
        let queued: HashSet<Key> = q.items.drain(..).map(|j| j.key).collect();
        let needed_set: HashSet<Key> = needed.iter().copied().collect();
        // cererile abandonate revin la „neîncărcat”
        for k in &queued {
            if !needed_set.contains(k)
                && matches!(self.tiles.get(k), Some((TileState::Loading, _)))
            {
                self.tiles.remove(k);
            }
        }
        let cache = cache_dir();
        for &k in needed {
            let state = self.tiles.get(&k).map(|(s, _)| s);
            let push = match state {
                None => true,
                Some(TileState::Loading) => queued.contains(&k), // altfel e deja în lucru
                Some(TileState::Failed(t)) => now - *t > 20.0,
                Some(TileState::Ready(_)) => false,
            };
            if !push {
                continue;
            }
            let (l, z, x, y) = k;
            q.items.push_back(Job {
                epoch: self.epoch,
                key: k,
                url: src.url(l as usize, z, x, y),
                path: cache.as_ref().map(|d| {
                    d.join(&src.id)
                        .join(l.to_string())
                        .join(z.to_string())
                        .join(x.to_string())
                        .join(format!("{y}.tile"))
                }),
            });
            self.tiles.insert(k, (TileState::Loading, self.frame));
        }
        drop(q);
        self.shared.cv.notify_all();
    }

    /// Eliberează texturile nefolosite recent.
    fn evict(&mut self) {
        if self.tiles.len() <= 600 {
            return;
        }
        let mut old: Vec<(u64, Key)> = self
            .tiles
            .iter()
            .filter(|(_, (s, used))| !matches!(s, TileState::Loading) && *used + 2 < self.frame)
            .map(|(k, (_, used))| (*used, *k))
            .collect();
        old.sort();
        let excess = self.tiles.len().saturating_sub(450);
        for (_, k) in old.into_iter().take(excess) {
            self.tiles.remove(&k);
        }
    }

    /// Textul de atribuire și starea (afișate în colțul hărții).
    pub fn status_text(&self) -> Option<String> {
        let s = self.source.as_ref()?;
        let mut t = if s.attribution.is_empty() {
            cadastru::i18n::trs(&s.name).into_owned()
        } else {
            s.attribution.clone()
        };
        if self.loading > 0 {
            t.push_str(&tr!("   ·   se încarcă {} tile-uri", self.loading));
        }
        if self.ok == 0
            && self.errors > 0
            && let Some(e) = &self.last_error
        {
            let mut e = e.clone();
            if e.chars().count() > 90 {
                e = e.chars().take(90).collect::<String>() + "…";
            }
            t = tr!("Basemap indisponibil: {e}");
        }
        Some(t)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quadkeys() {
        // exemplu din documentația Bing Maps Tile System
        assert_eq!(quadkey(3, 3, 5), "213");
        let s = Source::from_id("bing-aerial").unwrap();
        assert!(s.url(0, 3, 3, 5).contains("/a213.jpeg"));
        let o = Source::from_id("osm").unwrap();
        assert_eq!(o.url(0, 6, 36, 22), "https://tile.openstreetmap.org/6/36/22.png");
        let e = Source::from_id("esri-imagery").unwrap();
        assert!(e.url(0, 6, 36, 22).ends_with("/World_Imagery/MapServer/tile/6/22/36"));
        assert!(Source::custom("https://x/{z}/{x}/{y}.png", 18).is_some());
        assert!(Source::custom("ftp://x", 18).is_none());
    }

    /// Eroarea introdusă de desenarea tile-ului ca plasă de triunghiuri (față de reproiecția
    /// exactă a fiecărui punct), în pixeli ai tile-ului, pe toată România, la toate nivelurile.
    #[test]
    fn mesh_error_below_half_pixel() {
        let s70 = Crs::from_epsg(3844).unwrap();
        let merc = Crs::from_epsg(3857).unwrap();
        let wgs = Crs::wgs84();
        let to_m = Transformer::new(&wgs, &merc).unwrap();
        let from_m = Transformer::new(&merc, &s70).unwrap();
        let mut worst: f64 = 0.0;
        for z in 5..=19u8 {
            let n = grid_n(z) as f64;
            let size = 2.0 * MERC / (1u64 << z) as f64;
            let px = size / 256.0; // m Mercator pe pixel de tile
            for k in 0..60 {
                let lon = 20.5 + 9.0 * ((k * 37 % 60) as f64 / 60.0);
                let lat = 43.8 + 4.3 * ((k * 11 % 60) as f64 / 60.0);
                let (mx, my) = to_m.xy(lon, lat).unwrap();
                // celula plasei care conține punctul
                let tx = ((mx + MERC) / size).floor();
                let ty = ((MERC - my) / size).floor();
                let fx = ((mx + MERC) / size - tx) * n;
                let fy = ((MERC - my) / size - ty) * n;
                let (i, j) = (fx.floor(), fy.floor());
                let (u, v) = (fx - i, fy - j);
                let corner = |di: f64, dj: f64| {
                    let cx = -MERC + tx * size + (i + di) / n * size;
                    let cy = MERC - ty * size - (j + dj) / n * size;
                    from_m.xy(cx, cy).unwrap()
                };
                let (a, b, c, d) = (corner(0.0, 0.0), corner(1.0, 0.0), corner(0.0, 1.0), corner(1.0, 1.0));
                // triunghiurile a-b-d și a-d-c (ca la desenare)
                let (x, y) = if u >= v {
                    (a.0 + u * (b.0 - a.0) + v * (d.0 - b.0), a.1 + u * (b.1 - a.1) + v * (d.1 - b.1))
                } else {
                    (a.0 + v * (c.0 - a.0) + u * (d.0 - c.0), a.1 + v * (c.1 - a.1) + u * (d.1 - c.1))
                };
                let exact = from_m.xy(mx, my).unwrap();
                let err_m = ((x - exact.0).powi(2) + (y - exact.1).powi(2)).sqrt();
                // pixeli de ecran ≈ m / (dimensiunea pixelului de tile pe teren, ~px * cos(lat))
                let err_px = err_m / (px * lat.to_radians().cos());
                worst = worst.max(err_px);
            }
        }
        eprintln!("eroare maximă plasă: {worst:.4} px");
        assert!(worst < 0.5, "{worst}");
    }
}

// ---------------------------------------------------------------------------
// Interfață: butonul „Basemap” din bara de instrumente

impl super::App {
    pub fn basemap_menu(&mut self, ui: &mut egui::Ui) {
        let current = self
            .basemap
            .source
            .as_ref()
            .map(|s| cadastru::i18n::trs(&s.name).into_owned())
            .unwrap_or_else(|| tr!("fără").into());
        let mut choice: Option<Option<Source>> = None;
        let mut open_custom = false;
        let mut clear = false;
        let mut opacity_changed = false;
        let r = ui.menu_button("Basemap", |ui| {
            ui.set_min_width(240.0);
            let cur_id = self.basemap.source.as_ref().map(|s| s.id.clone());
            if ui
                .radio(cur_id.is_none(), tr!("Fără basemap"))
                .clicked()
            {
                choice = Some(None);
                ui.close();
            }
            ui.separator();
            let mut groups: Vec<&str> = vec![];
            for p in PROVIDERS {
                if !groups.contains(&p.group) {
                    groups.push(p.group);
                }
            }
            for g in groups {
                let active = PROVIDERS
                    .iter()
                    .any(|p| p.group == g && cur_id.as_deref() == Some(p.id));
                let title = if active {
                    egui::RichText::new(format!("{}  •", cadastru::i18n::trs_static(g))).strong()
                } else {
                    egui::RichText::new(cadastru::i18n::trs_static(g))
                };
                ui.menu_button(title, |ui| {
                    ui.set_min_width(280.0);
                    for p in PROVIDERS.iter().filter(|p| p.group == g) {
                        if ui
                            .radio(cur_id.as_deref() == Some(p.id), cadastru::i18n::trs_static(p.name))
                            .on_hover_text(p.attribution)
                            .clicked()
                        {
                            choice = Some(Source::from_id(p.id));
                            ui.close();
                        }
                    }
                });
            }
            ui.separator();
            let is_custom = cur_id.as_deref().is_some_and(|i| i.starts_with("custom-"));
            if ui
                .radio(is_custom, tr!("Șablon XYZ personalizat…"))
                .on_hover_text(cadastru::i18n::trs_static("Orice serviciu de tile-uri: https://server/{z}/{x}/{y}.png"))
                .clicked()
            {
                open_custom = true;
                ui.close();
            }
            ui.separator();
            ui.horizontal(|ui| {
                ui.label(tr!("Opacitate:"));
                if ui
                    .add(egui::Slider::new(&mut self.basemap.opacity, 0.0..=1.0).show_value(true))
                    .changed()
                {
                    opacity_changed = true;
                }
            });
            if ui
                .button(tr!("Golește cache-ul de tile-uri"))
                .on_hover_text(
                    cache_dir()
                        .map(|d| d.display().to_string())
                        .unwrap_or_default(),
                )
                .clicked()
            {
                clear = true;
                ui.close();
            }
        });
        r.response
            .on_hover_text(tr!("Harta de fundal: {current}"));
        if let Some(s) = choice {
            self.basemap.set_source(s);
            self.status = match &self.basemap.source {
                Some(s) => format!("Basemap: {}", cadastru::i18n::trs(&s.name)),
                None => tr!("Fără basemap").into(),
            };
        }
        if opacity_changed {
            self.settings.basemap_opacity = self.basemap.opacity;
            self.settings.save();
        }
        if open_custom {
            self.show_bm_custom = true;
        }
        if clear {
            let src = self.basemap.source.clone();
            self.basemap.set_source(None);
            self.status = match clear_cache() {
                Ok(_) => tr!("Cache-ul de tile-uri a fost golit").into(),
                Err(e) => tr!("Cache-ul nu a putut fi golit complet: {e}"),
            };
            self.basemap.set_source(src);
        }
    }

    pub fn basemap_custom_window(&mut self, ctx: &egui::Context) {
        if !self.show_bm_custom {
            return;
        }
        let mut open = true;
        let mut apply = false;
        egui::Window::new(tr!("Basemap personalizat")).id(egui::Id::unique("win_basemap_0"))
            .open(&mut open)
            .collapsible(false)
            .default_width(560.0)
            .show(ctx, |ui| {
                ui.label(tr!("Șablon URL pentru tile-uri XYZ în Web Mercator (EPSG:3857):"));
                ui.add(
                    egui::TextEdit::singleline(&mut self.bm_custom_text)
                        .desired_width(f32::INFINITY)
                        .hint_text("https://{s}.server.ro/tiles/{z}/{x}/{y}.png"),
                );
                ui.label(
                    egui::RichText::new(
                        cadastru::i18n::trs_static("{z} {x} {y} = nivel și coloană/rând; {-y} = rând TMS; {s} = subdomeniu a/b/c; {q} = quadkey Bing"),
                    )
                    .small()
                    .weak(),
                );
                ui.horizontal(|ui| {
                    ui.label(tr!("Zoom maxim:"));
                    ui.add(egui::DragValue::new(&mut self.settings.basemap_custom_zoom).range(1..=22));
                });
                let valid = Source::custom(&self.bm_custom_text, self.settings.basemap_custom_zoom);
                if valid.is_none() && !self.bm_custom_text.trim().is_empty() {
                    ui.colored_label(
                        ui.visuals().error_fg_color,
                        cadastru::i18n::trs_static("Șablonul trebuie să înceapă cu http(s):// și să conțină {z}, {x}, {y} (sau {q})."),
                    );
                }
                ui.horizontal(|ui| {
                    if ui.add_enabled(valid.is_some(), egui::Button::new(tr!("Folosește"))).clicked() {
                        apply = true;
                    }
                    if ui.button(tr!("Anulează")).clicked() {
                        self.show_bm_custom = false;
                    }
                });
            });
        if apply
            && let Some(s) =
                Source::custom(&self.bm_custom_text, self.settings.basemap_custom_zoom)
        {
            self.settings.basemap_custom = self.bm_custom_text.trim().to_string();
            self.settings.save();
            self.basemap.set_source(Some(s));
            self.show_bm_custom = false;
            self.status = tr!("Basemap personalizat activ").into();
        }
        if !open {
            self.show_bm_custom = false;
        }
    }

    /// Identificator pentru salvarea în spațiul de lucru.
    pub fn basemap_ws_id(&self) -> Option<String> {
        let s = self.basemap.source.as_ref()?;
        Some(if s.id.starts_with("custom-") {
            format!("custom:{}|{}", s.max_zoom, s.layers[0])
        } else {
            s.id.clone()
        })
    }

    pub fn basemap_from_ws_id(&mut self, id: Option<&str>) {
        let src = id.and_then(|id| match id.strip_prefix("custom:") {
            Some(rest) => {
                let (z, t) = rest.split_once('|')?;
                Source::custom(t, z.parse().unwrap_or(19))
            }
            None => Source::from_id(id),
        });
        self.basemap.set_source(src);
    }
}

// ---------------------------------------------------------------------------
// Descărcare sincronă (pentru tipărire)

/// Tile decodat: lățime, înălțime și pixeli RGBA premultiplicați.
pub struct TileImage {
    pub w: usize,
    pub h: usize,
    pub px: Vec<[u8; 4]>,
}

/// Descarcă (sau ia din cache) tile-urile cerute, în paralel. `progress(n)` primește numărul
/// de tile-uri terminate.
pub fn fetch_tiles_blocking(
    src: &Source,
    keys: &[(u8, u8, u32, u32)],
    progress: &(dyn Fn(usize) + Sync),
) -> HashMap<(u8, u8, u32, u32), TileImage> {
    let agent = make_agent();
    let cache = cache_dir();
    let jobs: Vec<Job> = keys
        .iter()
        .map(|&k| {
            let (l, z, x, y) = k;
            Job {
                epoch: 0,
                key: k,
                url: src.url(l as usize, z, x, y),
                path: cache.as_ref().map(|d| {
                    d.join(&src.id)
                        .join(l.to_string())
                        .join(z.to_string())
                        .join(x.to_string())
                        .join(format!("{y}.tile"))
                }),
            }
        })
        .collect();
    let next = std::sync::atomic::AtomicUsize::new(0);
    let done = std::sync::atomic::AtomicUsize::new(0);
    let out = Mutex::new(HashMap::new());
    std::thread::scope(|s| {
        for _ in 0..8 {
            s.spawn(|| {
                loop {
                    let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let Some(j) = jobs.get(i) else { break };
                    if let Ok(img) = fetch(&agent, j) {
                        let t = TileImage {
                            w: img.size[0],
                            h: img.size[1],
                            px: img.pixels.iter().map(|c| c.to_array()).collect(),
                        };
                        if let Ok(mut m) = out.lock() {
                            m.insert(j.key, t);
                        }
                    }
                    let n = done.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                    progress(n);
                }
            });
        }
    });
    out.into_inner().unwrap_or_default()
}
