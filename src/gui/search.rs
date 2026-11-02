//! Instrumentul Căutare: locuri și adrese (OpenStreetMap Nominatim și / sau Google,
//! linkuri Google Maps, coduri Plus, coordonate),
//! căutare rapidă în straturile încărcate și interogări pe atribute cu condiții ȘI / SAU.

use super::App;
use super::layers::LayerKind;
use super::select::SelMode;
use cadastru::crs::{Crs, Transformer};
use cadastru::i18n::tr;
use cadastru::model::*;
use cadastru::query::{self, Cond, Join, Op};
use super::google;
use super::theme::Geocoder;
use eframe::egui::{self, Color32, FontId, RichText, Stroke, Vec2};
use std::sync::mpsc::{Receiver, channel};

/// Un loc găsit prin geocodare.
#[derive(Clone, Debug)]
pub struct Place {
    pub name: String,
    pub kind: String,
    pub lon: f64,
    pub lat: f64,
    /// încadrarea în WGS84: [vest, sud, est, nord]
    pub bbox: Option<[f64; 4]>,
    pub src: PlaceSrc,
}

/// De unde provine un loc găsit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlaceSrc {
    Osm,
    Google,
    Coords,
}

/// Rezultatul unei căutări online (rulată în fundal).
pub struct GeoResult {
    pub places: Vec<Place>,
    /// avertismente: de ex. una dintre surse nu a răspuns
    pub notes: Vec<String>,
    /// un singur loc precis (link, cod Plus): harta merge direct la el
    pub auto: bool,
}

/// Marcajul locului ales, afișat pe hartă.
pub struct Marker {
    pub lon: f64,
    pub lat: f64,
    pub label: String,
    /// poziția în sistemul hărții și sistemul pentru care a fost calculată
    map_xy: Option<(f64, f64)>,
    for_crs: Option<Option<Crs>>,
}

/// O entitate găsită: (strat, indicele entității).
pub type Hit = (u64, usize);

pub struct SearchDlg {
    pub open: bool,
    /// cere focalizarea câmpului de căutare la următoarea afișare
    pub focus: bool,
    /// 0 = loc / adresă, 1 = atribute
    pub tab: u8,
    pub place_q: String,
    pub place_rx: Option<Receiver<Result<GeoResult, String>>>,
    pub places: Vec<Place>,
    pub place_msg: String,
    /// rezultatele căutării rapide în straturi
    pub quick: Vec<Hit>,
    /// stratul interogat (None = toate straturile vectoriale)
    pub layer: Option<u64>,
    pub conds: Vec<Cond>,
    pub results: Vec<Hit>,
    pub result_msg: String,
}

impl Default for SearchDlg {
    fn default() -> Self {
        SearchDlg {
            open: false,
            focus: false,
            tab: 0,
            place_q: String::new(),
            place_rx: None,
            places: vec![],
            place_msg: String::new(),
            quick: vec![],
            layer: None,
            conds: vec![Cond::new(None, Op::Contains, "")],
            results: vec![],
            result_msg: String::new(),
        }
    }
}

const NOMINATIM: &str = "https://nominatim.openstreetmap.org/search";
const MAX_LIST: usize = 500;

/// Codificare procentuală pentru parametrii URL.
pub fn url_encode(s: &str) -> String {
    let mut o = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => o.push(b as char),
            b' ' => o.push('+'),
            _ => o.push_str(&format!("%{b:02X}")),
        }
    }
    o
}

/// Interpretează răspunsul JSON Nominatim (format jsonv2).
pub fn parse_nominatim(body: &str) -> Result<Vec<Place>, String> {
    let v: serde_json::Value = serde_json::from_str(body).map_err(|e| e.to_string())?;
    let arr = v.as_array().ok_or_else(|| tr!("răspuns neașteptat").to_string())?;
    let num = |x: &serde_json::Value| -> Option<f64> {
        x.as_str().and_then(|s| s.parse().ok()).or_else(|| x.as_f64())
    };
    Ok(arr
        .iter()
        .filter_map(|r| {
            let lat = num(r.get("lat")?)?;
            let lon = num(r.get("lon")?)?;
            let bbox = r.get("boundingbox").and_then(|b| b.as_array()).and_then(|b| {
                // [sud, nord, vest, est]
                let f: Vec<f64> = b.iter().filter_map(num).collect();
                (f.len() == 4).then(|| [f[2], f[0], f[3], f[1]])
            });
            let kind = [r.get("category"), r.get("type")]
                .iter()
                .filter_map(|x| x.and_then(|x| x.as_str()))
                .filter(|s| *s != "yes")
                .collect::<Vec<_>>()
                .join(" / ");
            Some(Place {
                name: r.get("display_name")?.as_str()?.to_string(),
                kind,
                lon,
                lat,
                bbox,
                src: PlaceSrc::Osm,
            })
        })
        .collect())
}

fn geocode_osm(q: &str, ro_only: bool, lang: &str) -> Result<Vec<Place>, String> {
    let mut url = format!(
        "{NOMINATIM}?format=jsonv2&limit=10&q={}&accept-language={lang}",
        url_encode(q)
    );
    if ro_only {
        url.push_str("&countrycodes=ro");
    }
    let agent = super::basemap::make_agent();
    let mut resp = agent.get(&url).call().map_err(|e| e.to_string())?;
    let body = resp
        .body_mut()
        .with_config()
        .limit(4 * 1024 * 1024)
        .read_to_string()
        .map_err(|e| e.to_string())?;
    parse_nominatim(&body)
}

impl Place {
    /// Rezultat obținut local, fără niciun serviciu online: link Google Maps citit
    /// din adresă, cod Plus decodat sau coordonate scrise.
    pub fn is_local(&self) -> bool {
        self.src == PlaceSrc::Coords || self.kind == "Google Maps" || self.kind == "Plus Code"
    }
}

/// Titlul listei de rezultate: după proveniența reală a rezultatelor afișate,
/// nu după sursa aleasă la „Sursă:” (un link Google Maps sau un cod Plus se
/// citesc local, oricare ar fi sursa). Fără rezultate (doar un mesaj), titlul
/// spune ce sursă s-a interogat.
pub fn places_heading(places: &[Place], geocoder: Geocoder) -> &'static str {
    if places.is_empty() {
        return match geocoder {
            Geocoder::Osm => tr!("Locuri (OpenStreetMap)"),
            Geocoder::Google => tr!("Locuri (Google)"),
            Geocoder::Both => tr!("Locuri (Google și OpenStreetMap)"),
        };
    }
    if places.iter().all(|p| p.kind == "Google Maps") {
        return tr!("Loc din linkul Google Maps");
    }
    if places.iter().all(|p| p.kind == "Plus Code") {
        return tr!("Loc din codul Plus");
    }
    if places.iter().all(|p| p.src == PlaceSrc::Coords) {
        return tr!("Coordonate");
    }
    let osm = places.iter().any(|p| p.src == PlaceSrc::Osm);
    let google = places.iter().any(|p| p.src == PlaceSrc::Google && !p.is_local());
    match (osm, google) {
        (true, true) => tr!("Locuri (Google și OpenStreetMap)"),
        (true, false) => tr!("Locuri (OpenStreetMap)"),
        (false, true) => tr!("Locuri (Google)"),
        (false, false) => tr!("Locuri"),
    }
}

/// Locul unui cod Plus decodat.
fn plus_place(code: &str, locality: &str, lat: f64, lon: f64, size: f64) -> Place {
    let h = size.max(0.0005) / 2.0;
    Place {
        name: if locality.is_empty() { code.to_string() } else { format!("{code} {locality}") },
        kind: "Plus Code".into(),
        lon,
        lat,
        bbox: Some([lon - h * 8.0, lat - h * 8.0, lon + h * 8.0, lat + h * 8.0]),
        src: PlaceSrc::Google,
    }
}

/// Două numere: „lat, lon” (ca în Google Maps) sau „x y” în sistemul hărții.
pub fn parse_coords(s: &str) -> Option<(f64, f64)> {
    let t = s.trim();
    // „44,85 24,87” → virgula zecimală doar dacă separatorul este spațiul sau „;”
    let parts: Vec<&str> = if t.contains(';') {
        t.split(';').collect()
    } else if t.matches(',').count() == 1 && !t.contains(' ') || t.contains(", ") {
        t.split(',').collect()
    } else {
        t.split_whitespace().collect()
    };
    let nums: Vec<f64> = parts
        .iter()
        .map(|p| p.trim())
        .filter(|p| !p.is_empty())
        .map(|p| p.replace(',', ".").parse::<f64>())
        .collect::<Result<_, _>>()
        .ok()?;
    (nums.len() == 2 && nums.iter().all(|v| v.is_finite())).then(|| (nums[0], nums[1]))
}

impl App {
    fn layer_name(&self, id: u64) -> String {
        self.layer(id).map(|l| l.name.clone()).unwrap_or_default()
    }

    /// Textul scurt al unei entități pentru liste: câmpul etichetă sau primele valori.
    fn feature_caption(&self, (lid, fi): Hit) -> String {
        let Some(l) = self.layer(lid) else { return String::new() };
        let LayerKind::Vector(v) = &l.kind else { return String::new() };
        let Some(f) = v.data.features.get(fi) else { return String::new() };
        if let Some(i) = v.style.label_field.as_ref().and_then(|n| v.data.field_index(n))
            && let Some(a) = f.attrs.get(i)
            && !a.is_null()
        {
            return a.to_text();
        }
        f.attrs
            .iter()
            .filter(|a| !a.is_null())
            .take(3)
            .map(|a| a.to_text())
            .collect::<Vec<_>>()
            .join(" · ")
    }

    /// Straturile vectoriale vizate de căutare.
    fn search_layers(&self, only: Option<u64>) -> Vec<u64> {
        self.layers
            .iter()
            .filter(|l| l.is_vector() && only.is_none_or(|o| o == l.id))
            .map(|l| l.id)
            .collect()
    }

    /// Rulează condițiile pe straturi.
    pub fn run_query(&self, only: Option<u64>, conds: &[Cond]) -> Vec<Hit> {
        let mut out = vec![];
        for id in self.search_layers(only) {
            if let Some(LayerKind::Vector(v)) = self.layer(id).map(|l| &l.kind) {
                out.extend(query::run(&v.data, conds).into_iter().map(|fi| (id, fi)));
            }
        }
        out
    }

    fn hits_bounds(&self, hits: &[Hit]) -> Option<Bounds> {
        let mut b = Bounds::empty();
        for &(lid, fi) in hits {
            if let Some(LayerKind::Vector(v)) = self.layer(lid).map(|l| &l.kind) {
                for d in v.disp.iter().filter(|d| d.feat == fi) {
                    b.merge(&d.bbox);
                }
            }
        }
        b.is_valid().then_some(b)
    }

    /// Selectează entitățile găsite (înlocuiește sau adaugă la selecție).
    pub fn select_hits(&mut self, hits: &[Hit], mode: SelMode) {
        if mode == SelMode::Replace {
            for l in &mut self.layers {
                if let LayerKind::Vector(v) = &mut l.kind {
                    v.clear_sel();
                }
            }
        }
        let mut ids: Vec<u64> = hits.iter().map(|h| h.0).collect();
        ids.dedup();
        for id in ids {
            let feats: Vec<usize> = hits.iter().filter(|h| h.0 == id).map(|h| h.1).collect();
            if let Some(LayerKind::Vector(v)) = self.layer_mut(id).map(|l| &mut l.kind) {
                v.apply_sel(feats, if mode == SelMode::Replace { SelMode::Add } else { mode });
            }
        }
        self.status = self.selection_status();
    }

    /// Pune marcajul pe un loc (WGS84) și centrează harta pe el.
    pub fn go_to_place(&mut self, p: &Place) {
        let wgs = Crs::wgs84();
        let tr_ = match &self.map_crs {
            Some(m) => Transformer::new(&wgs, m).ok(),
            None => Some(Transformer::identity()),
        };
        let Some(t) = tr_ else { return };
        let target = p.bbox.and_then(|[w, s, e, n]| {
            let mut b = Bounds::empty();
            // marginile și mijloacele laturilor, ca reproiecția să cuprindă tot locul
            for (x, y) in [
                (w, s),
                (e, s),
                (e, n),
                (w, n),
                ((w + e) / 2.0, s),
                ((w + e) / 2.0, n),
                (w, (s + n) / 2.0),
                (e, (s + n) / 2.0),
            ] {
                if let Some((mx, my)) = t.xy(x, y) {
                    b.extend(mx, my);
                }
            }
            b.is_valid().then_some(b)
        });
        if let Some(b) = target {
            self.zoom_to_padded(b);
        } else if let Some((x, y)) = t.xy(p.lon, p.lat) {
            self.zoom_point(x, y);
        }
        let label = if p.is_local() {
            p.name.clone()
        } else {
            p.name.split(',').next().unwrap_or(&p.name).trim().to_string()
        };
        self.search_marker = Some(Marker {
            lon: p.lon,
            lat: p.lat,
            label,
            map_xy: None,
            for_crs: None,
        });
        self.status = tr!("Loc: {}", p.name);
    }

    /// Centrează pe un punct, la o scară de cartier (circa 1,5 km în lățime).
    fn zoom_point(&mut self, x: f64, y: f64) {
        let geo = self.map_crs.as_ref().is_none_or(|c| c.is_geographic());
        let h = if geo { 0.008 } else { 750.0 };
        self.zoom_to(Bounds::new(x - h, y - h, x + h, y + h));
    }

    /// Coordonate scrise în căutare: lat, lon (WGS84) sau x y în sistemul hărții.
    fn go_to_coords(&mut self, a: f64, b: f64) -> bool {
        let map_geo = self.map_crs.as_ref().is_none_or(|c| c.is_geographic());
        let looks_latlon = a.abs() <= 90.0 && b.abs() <= 180.0;
        if looks_latlon {
            let p = Place {
                name: format!("{a:.6}, {b:.6}"),
                kind: "WGS84".into(),
                lon: b,
                lat: a,
                bbox: None,
                src: PlaceSrc::Coords,
            };
            self.go_to_place(&p);
            return true;
        }
        if !map_geo {
            // x y în sistemul hărții
            let lonlat = self.to_wgs.as_ref().and_then(|t| t.xy(a, b));
            self.zoom_point(a, b);
            if let Some((lon, lat)) = lonlat {
                self.search_marker = Some(Marker {
                    lon,
                    lat,
                    label: format!("{a:.2} {b:.2}"),
                    map_xy: None,
                    for_crs: None,
                });
            }
            self.status = tr!("Coordonate: {a} {b}");
            return true;
        }
        false
    }

    fn start_place_search(&mut self) {
        let q = self.dlg.search.place_q.trim().to_string();
        self.dlg.search.places.clear();
        self.dlg.search.quick.clear();
        if q.is_empty() {
            self.dlg.search.place_msg.clear();
            return;
        }
        if let Some((a, b)) = parse_coords(&q)
            && self.go_to_coords(a, b)
        {
            self.dlg.search.place_msg.clear();
            return;
        }
        // link Google Maps complet: locul se citește direct din adresă
        if google::is_maps_link(&q) && !google::is_short_link(&q) {
            match google::parse_maps_url(&q) {
                Some(p) => {
                    self.dlg.search.place_msg.clear();
                    self.dlg.search.places = vec![p.clone()];
                    self.go_to_place(&p);
                }
                None => {
                    self.dlg.search.place_msg =
                        tr!("Linkul Google Maps nu conține coordonate. Deschideți locul în Google Maps și copiați linkul din bara de adrese sau din Distribuie.").into()
                }
            }
            return;
        }
        let ro = self.settings.geocode_ro_only;
        let lang = cadastru::i18n::lang().code().to_lowercase();
        let source = self.settings.geocoder;
        let key = google::effective_key(&self.settings.google_api_key);
        let (tx, rx) = channel();
        if google::is_short_link(&q) {
            // link scurt (maps.app.goo.gl): urmăm redirecționarea în fundal
            std::thread::spawn(move || {
                let r = google::resolve_short_link(&q).map(|p| GeoResult { places: vec![p], notes: vec![], auto: true });
                let _ = tx.send(r);
            });
        } else if let Some((code, locality)) = google::find_plus_code(&q) {
            if google::plus_is_full(&code) || locality.is_empty() {
                // cod complet, sau cod scurt raportat la centrul hărții
                let center = self.view_center_wgs();
                let r = if google::plus_is_full(&code) {
                    google::plus_decode(&code)
                } else {
                    center.and_then(|(lon, lat)| google::plus_recover(&code, lat, lon))
                };
                match r {
                    Some((lat, lon, size)) => {
                        let p = plus_place(&code, &locality, lat, lon, size);
                        self.dlg.search.places = vec![p.clone()];
                        self.dlg.search.place_msg.clear();
                        self.go_to_place(&p);
                    }
                    None => self.dlg.search.place_msg = tr!("Codul Plus nu a putut fi decodat.").into(),
                }
                return;
            }
            // cod scurt + localitate: localitatea dă punctul de referință
            std::thread::spawn(move || {
                let refs = match source {
                    Geocoder::Osm => geocode_osm(&locality, ro, &lang),
                    _ if key.is_empty() => geocode_osm(&locality, ro, &lang),
                    _ => google::search(&locality, ro, &lang, &key).or_else(|_| geocode_osm(&locality, ro, &lang)),
                };
                let r = refs.and_then(|v| {
                    let p0 = v.first().ok_or_else(|| tr!("localitatea „{locality}” nu a fost găsită"))?;
                    let (lat, lon, size) = google::plus_recover(&code, p0.lat, p0.lon)
                        .ok_or_else(|| tr!("Codul Plus nu a putut fi decodat.").to_string())?;
                    Ok(GeoResult { places: vec![plus_place(&code, &locality, lat, lon, size)], notes: vec![], auto: true })
                });
                let _ = tx.send(r);
            });
        } else {
            // în straturile încărcate: orice câmp conține textul
            self.dlg.search.quick = self.run_query(None, &[Cond::new(None, Op::Contains, &q)]);
            if source != Geocoder::Osm && key.is_empty() {
                self.dlg.search.place_msg = tr!("Pentru căutarea Google este nevoie de o cheie API (mai jos). Fără cheie: butonul „Deschide în Google Maps” trimite căutarea în browser, iar linkul locului copiat de acolo se poate lipi aici.").into();
                if source == Geocoder::Google {
                    return;
                }
            }
            std::thread::spawn(move || {
                let use_osm = source != Geocoder::Google || key.is_empty();
                let use_google = source != Geocoder::Osm && !key.is_empty();
                let mut places = vec![];
                let mut notes = vec![];
                let mut errs = vec![];
                if use_google {
                    match google::search(&q, ro, &lang, &key) {
                        Ok(v) => places.extend(v),
                        Err(e) => errs.push(format!("Google: {e}")),
                    }
                }
                if use_osm {
                    match geocode_osm(&q, ro, &lang) {
                        Ok(v) => places.extend(v),
                        Err(e) => errs.push(format!("OpenStreetMap: {e}")),
                    }
                }
                let r = if places.is_empty() && !errs.is_empty() && errs.len() == use_google as usize + use_osm as usize {
                    Err(errs.join("\n"))
                } else {
                    notes.extend(errs);
                    Ok(GeoResult { places, notes, auto: false })
                };
                let _ = tx.send(r);
            });
        }
        self.dlg.search.place_rx = Some(rx);
        self.dlg.search.place_msg = tr!("Se caută…").into();
    }

    /// Centrul vederii, în WGS84 (lon, lat).
    pub fn view_center_wgs(&self) -> Option<(f64, f64)> {
        let (x, y) = (self.view.cx, self.view.cy);
        match &self.map_crs {
            Some(c) if !c.is_geographic() => self.to_wgs.as_ref().and_then(|t| t.xy(x, y)),
            _ => (x.abs() <= 180.0 && y.abs() <= 90.0).then_some((x, y)),
        }
    }

    /// Nivelul de zoom Google Maps echivalent vederii curente.
    fn view_zoom_web(&self) -> u8 {
        let Some((lon, lat)) = self.view_center_wgs() else { return 7 };
        let s = self.view.scale;
        let (x, y) = (self.view.cx, self.view.cy);
        let other = match &self.map_crs {
            Some(c) if !c.is_geographic() => self.to_wgs.as_ref().and_then(|t| t.xy(x + s * 100.0, y)),
            _ => Some((x + s * 100.0, y)),
        };
        let Some((lon2, _)) = other else { return 7 };
        let mpp = ((lon2 - lon).abs() * 111_320.0 * lat.to_radians().cos() / 100.0).max(1e-6);
        let z = (156_543.03 * lat.to_radians().cos() / mpp).log2();
        z.round().clamp(2.0, 21.0) as u8
    }

    /// „Deschide în Google Maps”: textul căutat, sau vederea curentă dacă nu e scris nimic.
    fn open_google_maps(&mut self) {
        let q = self.dlg.search.place_q.trim().to_string();
        let url = if !q.is_empty() && !google::is_maps_link(&q) {
            google::maps_search_url(&q)
        } else if let Some(m) = &self.search_marker {
            google::maps_view_url(m.lat, m.lon, self.view_zoom_web().max(15))
        } else if let Some((lon, lat)) = self.view_center_wgs() {
            google::maps_view_url(lat, lon, self.view_zoom_web())
        } else {
            "https://www.google.com/maps".to_string()
        };
        match super::manual::open_external(std::ffi::OsStr::new(&url)) {
            Ok(()) => self.status = tr!("Google Maps deschis în browser: {url}"),
            Err(e) => self.errors.push(tr!("Browserul nu s-a putut deschide: {e:#}")),
        }
    }

    /// Recalculează poziția marcajului dacă s-a schimbat sistemul hărții și îl desenează.
    pub fn draw_search_marker(&mut self, painter: &egui::Painter) {
        let map_crs = self.map_crs.clone();
        let Some(m) = self.search_marker.as_mut() else { return };
        if m.for_crs.as_ref() != Some(&map_crs) {
            let t = match &map_crs {
                Some(c) => Transformer::new(&Crs::wgs84(), c).ok(),
                None => Some(Transformer::identity()),
            };
            m.map_xy = t.and_then(|t| t.xy(m.lon, m.lat));
            m.for_crs = Some(map_crs);
        }
        let Some((x, y)) = m.map_xy else { return };
        let label = m.label.clone();
        let p = self.w2s(x, y);
        let red = Color32::from_rgb(220, 40, 40);
        // ac de hartă: cerc deasupra unui vârf
        let head = p - Vec2::new(0.0, 18.0);
        painter.add(egui::Shape::convex_polygon(
            vec![p, head + Vec2::new(-6.5, 3.5), head + Vec2::new(6.5, 3.5)],
            red,
            Stroke::NONE,
        ));
        painter.circle(head, 8.0, red, Stroke::new(1.5, Color32::WHITE));
        painter.circle_filled(head, 3.0, Color32::WHITE);
        if !label.is_empty() {
            let galley = painter.layout_no_wrap(label, FontId::proportional(12.0), Color32::BLACK);
            let r = egui::Rect::from_min_size(
                head + Vec2::new(12.0, -galley.size().y / 2.0 - 2.0),
                galley.size() + Vec2::new(8.0, 4.0),
            );
            painter.rect_filled(r, 3.0, Color32::from_white_alpha(230));
            painter.rect_stroke(r, 3.0, Stroke::new(1.0, red), egui::StrokeKind::Inside);
            painter.galley(r.min + Vec2::new(4.0, 2.0), galley, Color32::BLACK);
        }
    }

    pub fn search_window(&mut self, ctx: &egui::Context) {
        if !self.dlg.search.open {
            return;
        }
        // rezultatul geocodării
        if let Some(rx) = &self.dlg.search.place_rx {
            match rx.try_recv() {
                Ok(r) => {
                    self.dlg.search.place_rx = None;
                    match r {
                        Ok(g) => {
                            let mut msg = vec![];
                            if g.places.is_empty() {
                                msg.push(match self.settings.geocoder {
                                    Geocoder::Osm => tr!("Niciun loc găsit în OpenStreetMap.").to_string(),
                                    _ => tr!("Niciun loc găsit.").to_string(),
                                });
                            }
                            msg.extend(g.notes.iter().map(|n| tr!("Atenție – {n}")));
                            self.dlg.search.place_msg = msg.join("\n");
                            if g.auto && g.places.len() == 1 {
                                let p = g.places[0].clone();
                                self.go_to_place(&p);
                            }
                            self.dlg.search.places = g.places;
                        }
                        Err(e) => {
                            self.dlg.search.place_msg =
                                tr!("Căutarea online nu a reușit: {e}");
                        }
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    ctx.request_repaint_after(std::time::Duration::from_millis(100));
                }
                Err(_) => self.dlg.search.place_rx = None,
            }
        }
        let mut open = true;
        let mut act = Act::None;
        egui::Window::new(tr!("Căutare"))
            .id(egui::Id::unique("win_search"))
            .open(&mut open)
            .default_width(520.0)
            .default_height(460.0)
            .resizable(true)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let s = &mut self.dlg.search;
                    ui.selectable_value(&mut s.tab, 0, tr!("Loc / adresă"));
                    ui.selectable_value(&mut s.tab, 1, tr!("După câmpuri"));
                });
                ui.separator();
                if self.dlg.search.tab == 0 {
                    self.place_tab(ui, &mut act);
                } else {
                    self.query_tab(ui, &mut act);
                }
            });
        if !open {
            self.dlg.search.open = false;
        }
        self.apply_act(act);
    }

    fn hit_list(&self, ui: &mut egui::Ui, hits: &[Hit], act: &mut Act, id: &str) {
        egui::ScrollArea::vertical()
            .id_salt(id)
            .max_height(ui.available_height().max(120.0))
            .auto_shrink([false, true])
            .show(ui, |ui| {
                for &h in hits.iter().take(MAX_LIST) {
                    let txt = format!("{}  ·  #{}  {}", self.layer_name(h.0), h.1 + 1, self.feature_caption(h));
                    let r = ui.selectable_label(self.selection == Some(h), txt);
                    if r.clicked() {
                        *act = Act::ZoomHit(h);
                    }
                }
                if hits.len() > MAX_LIST {
                    ui.label(
                        RichText::new(tr!("… încă {} (folosiți Selectează pentru toate)", hits.len() - MAX_LIST))
                            .weak(),
                    );
                }
            });
    }

    fn hits_buttons(&self, ui: &mut egui::Ui, hits: &[Hit], act: &mut Act) {
        ui.horizontal(|ui| {
            let any = !hits.is_empty();
            if ui.add_enabled(any, egui::Button::new(tr!("Selectează"))).clicked() {
                *act = Act::Select(hits.to_vec(), SelMode::Replace);
            }
            if ui.add_enabled(any, egui::Button::new(tr!("Adaugă la selecție"))).clicked() {
                *act = Act::Select(hits.to_vec(), SelMode::Add);
            }
            if ui.add_enabled(any, egui::Button::new(tr!("Zoom la rezultate"))).clicked() {
                *act = Act::ZoomAll(hits.to_vec());
            }
        });
    }

    fn place_tab(&mut self, ui: &mut egui::Ui, act: &mut Act) {
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let go = ui.button(tr!("Caută")).clicked();
                let s = &mut self.dlg.search;
                let te = ui.add(
                    egui::TextEdit::singleline(&mut s.place_q)
                        .hint_text(tr!("ex. Pitești, Str. Victoriei 10 Brașov, 44.85 24.87, link Google Maps"))
                        .desired_width(ui.available_width()),
                );
                if s.focus {
                    te.request_focus();
                    s.focus = false;
                }
                let enter = te.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if go || enter {
                    *act = Act::PlaceSearch;
                }
            });
        });
        ui.horizontal_wrapped(|ui| {
            ui.label(tr!("Sursă:"));
            let mut changed = false;
            for (g, name) in [
                (Geocoder::Osm, "OpenStreetMap"),
                (Geocoder::Google, "Google"),
                (Geocoder::Both, tr!("Ambele")),
            ] {
                changed |= ui.radio_value(&mut self.settings.geocoder, g, name).changed();
            }
            if changed {
                self.settings.save();
            }
            ui.separator();
            if ui
                .checkbox(&mut self.settings.geocode_ro_only, tr!("Doar în România"))
                .changed()
            {
                self.settings.save();
            }
            if ui
                .button(tr!("Deschide în Google Maps"))
                .on_hover_text(tr!("Deschide în browser căutarea în Google Maps (sau vederea curentă, dacă nu e scris nimic). Linkul unui loc copiat din Google Maps se poate lipi apoi în căutare."))
                .clicked()
            {
                *act = Act::GoogleMaps;
            }
            if self.search_marker.is_some() && ui.small_button(tr!("Șterge marcajul")).clicked() {
                *act = Act::ClearMarker;
            }
        });
        if self.settings.geocoder != Geocoder::Osm {
            ui.horizontal(|ui| {
                ui.label(tr!("Cheie API Google:"));
                // butonul întâi (la dreapta), câmpul ocupă restul: fereastra nu se lățește singură
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .small_button(tr!("Obține o cheie…"))
                        .on_hover_text(tr!("Google Cloud Console: creați o cheie și activați Places API (New) sau Geocoding API"))
                        .clicked()
                    {
                        *act = Act::OpenUrl("https://console.cloud.google.com/google/maps-apis/credentials".into());
                    }
                    let env = self.settings.google_api_key.trim().is_empty()
                        && !google::effective_key("").is_empty();
                    let r = ui.add(
                        egui::TextEdit::singleline(&mut self.settings.google_api_key)
                            .password(true)
                            .hint_text(if env { tr!("din GOOGLE_MAPS_API_KEY") } else { "AIza…" })
                            .desired_width(ui.available_width()),
                    );
                    if r.lost_focus() || (r.changed() && self.settings.google_api_key.len() > 30) {
                        self.settings.save();
                    }
                });
            });
        }
        ui.label(
            RichText::new(tr!(
                "Caută în straturile încărcate (orice câmp) și online. Se pot scrie și: coordonate „lat, lon” (WGS84) sau „X Y” în sistemul hărții, un link Google Maps (inclusiv maps.app.goo.gl) ori un cod Plus (ex. 8GP8PV9C+2W sau PV9C+2W Pitești)."
            ))
            .weak()
            .small(),
        );
        ui.separator();
        let quick = self.dlg.search.quick.clone();
        if !quick.is_empty() {
            ui.strong(tr!("În straturile încărcate ({})", quick.len()));
            self.hits_buttons(ui, &quick, act);
            ui.push_id("quick", |ui| {
                ui.set_max_height(150.0);
                self.hit_list(ui, &quick, act, "quick_list");
            });
            ui.separator();
        }
        let msg = self.dlg.search.place_msg.clone();
        let places = self.dlg.search.places.clone();
        if !places.is_empty() || !msg.is_empty() {
            ui.strong(places_heading(&places, self.settings.geocoder));
        }
        if !msg.is_empty() {
            ui.label(RichText::new(msg).weak());
        }
        egui::ScrollArea::vertical()
            .id_salt("places")
            .auto_shrink([false, true])
            .show(ui, |ui| {
                let mixed = places.iter().any(|p| p.src == PlaceSrc::Osm)
                    && places.iter().any(|p| p.src == PlaceSrc::Google);
                for p in &places {
                    let text = if mixed {
                        let tag = if p.src == PlaceSrc::Google { "Google" } else { "OSM" };
                        format!("[{tag}]  {}", p.name)
                    } else {
                        p.name.clone()
                    };
                    let r = ui.add(
                        egui::Button::selectable(
                            false,
                            RichText::new(text),
                        )
                        .wrap(),
                    );
                    let r = if p.kind.is_empty() { r } else { r.on_hover_text(&p.kind) };
                    if r.clicked() {
                        *act = Act::Place(p.clone());
                    }
                }
            });
        let mut attr = vec![];
        if places.iter().any(|p| p.src == PlaceSrc::Osm) {
            attr.push("© OpenStreetMap contributors · Nominatim");
        }
        if places.iter().any(|p| p.src == PlaceSrc::Google && !p.is_local()) {
            attr.push("Google Maps Platform");
        }
        if !attr.is_empty() {
            ui.label(RichText::new(attr.join("   ·   ")).weak().small());
        }
    }

    fn query_tab(&mut self, ui: &mut egui::Ui, act: &mut Act) {
        // straturi și câmpuri
        let vec_layers: Vec<(u64, String)> = self
            .layers
            .iter()
            .filter(|l| l.is_vector())
            .map(|l| (l.id, l.name.clone()))
            .collect();
        if vec_layers.is_empty() {
            ui.label(RichText::new(tr!("Niciun strat vectorial încărcat.")).weak());
            return;
        }
        if self.dlg.search.layer.is_some_and(|id| !vec_layers.iter().any(|(i, _)| *i == id)) {
            self.dlg.search.layer = None;
        }
        let mut fields: Vec<String> = vec![];
        for id in self.search_layers(self.dlg.search.layer) {
            if let Some(LayerKind::Vector(v)) = self.layer(id).map(|l| &l.kind) {
                for f in &v.data.fields {
                    if !fields.contains(&f.name) {
                        fields.push(f.name.clone());
                    }
                }
            }
        }
        // valorile distincte ale câmpurilor (pentru lista de sugestii)
        let distinct = |app: &App, field: &str| -> Vec<String> {
            let mut vals = std::collections::BTreeSet::new();
            for id in app.search_layers(app.dlg.search.layer) {
                if let Some(LayerKind::Vector(v)) = app.layer(id).map(|l| &l.kind)
                    && let Some(i) = v.data.field_index(field)
                {
                    for f in &v.data.features {
                        if let Some(a) = f.attrs.get(i)
                            && !a.is_null()
                        {
                            vals.insert(a.to_text());
                            if vals.len() >= 300 {
                                break;
                            }
                        }
                    }
                }
            }
            vals.into_iter().collect()
        };
        ui.horizontal(|ui| {
            ui.label(tr!("Strat:"));
            let s = &mut self.dlg.search;
            let cur = s
                .layer
                .and_then(|id| vec_layers.iter().find(|(i, _)| *i == id))
                .map(|(_, n)| n.clone())
                .unwrap_or_else(|| tr!("Toate straturile vectoriale").into());
            egui::ComboBox::from_id_salt("q_layer").selected_text(cur).width(260.0).show_ui(ui, |ui| {
                ui.selectable_value(&mut s.layer, None, tr!("Toate straturile vectoriale"));
                for (id, n) in &vec_layers {
                    ui.selectable_value(&mut s.layer, Some(*id), n);
                }
            });
        });
        ui.add_space(4.0);
        let mut remove: Option<usize> = None;
        let mut suggestions: Vec<(usize, Vec<String>)> = vec![];
        for (i, c) in self.dlg.search.conds.iter().enumerate() {
            if let Some(f) = &c.field
                && c.op.needs_value()
            {
                suggestions.push((i, distinct(self, f)));
            }
        }
        let mut run = false;
        egui::Grid::new("q_conds").num_columns(5).spacing([6.0, 4.0]).show(ui, |ui| {
            let n = self.dlg.search.conds.len();
            for (i, c) in self.dlg.search.conds.iter_mut().enumerate() {
                if i == 0 {
                    ui.label(tr!("Unde"));
                } else {
                    egui::ComboBox::from_id_salt(("q_join", i))
                        .selected_text(c.join.label())
                        .width(56.0)
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut c.join, Join::And, Join::And.label());
                            ui.selectable_value(&mut c.join, Join::Or, Join::Or.label());
                        });
                }
                egui::ComboBox::from_id_salt(("q_field", i))
                    .selected_text(c.field.clone().unwrap_or_else(|| tr!("(orice câmp)").into()))
                    .width(150.0)
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut c.field, None, tr!("(orice câmp)"));
                        for f in &fields {
                            ui.selectable_value(&mut c.field, Some(f.clone()), f);
                        }
                    });
                egui::ComboBox::from_id_salt(("q_op", i))
                    .selected_text(c.op.label())
                    .width(110.0)
                    .show_ui(ui, |ui| {
                        for op in Op::ALL {
                            ui.selectable_value(&mut c.op, op, op.label());
                        }
                    });
                ui.horizontal(|ui| {
                    let te = ui
                        .add_enabled_ui(c.op.needs_value(), |ui| {
                            ui.add_sized(
                                [150.0, 20.0],
                                egui::TextEdit::singleline(&mut c.value),
                            )
                        })
                        .inner;
                    if te.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        run = true;
                    }
                    if let Some((_, vals)) = suggestions.iter().find(|(k, _)| *k == i)
                        && !vals.is_empty()
                    {
                        ui.menu_button("…", |ui| {
                            ui.set_max_height(300.0);
                            egui::ScrollArea::vertical().show(ui, |ui| {
                                for v in vals {
                                    if ui.button(v).clicked() {
                                        c.value = v.clone();
                                        ui.close();
                                    }
                                }
                            });
                        })
                        .response
                        .on_hover_text(tr!("Valorile din câmp"));
                    }
                });
                if n > 1 && ui.small_button("×").on_hover_text(tr!("Elimină condiția")).clicked() {
                    remove = Some(i);
                }
                ui.end_row();
            }
        });
        if let Some(i) = remove {
            self.dlg.search.conds.remove(i);
        }
        ui.horizontal(|ui| {
            if ui.button(tr!("+ Condiție")).clicked() {
                let last = self.dlg.search.conds.last().and_then(|c| c.field.clone());
                let mut c = Cond::new(last.as_deref(), Op::Eq, "");
                c.join = Join::And;
                self.dlg.search.conds.push(c);
            }
            if ui.button(tr!("Golește")).clicked() {
                self.dlg.search.conds = vec![Cond::new(None, Op::Contains, "")];
                self.dlg.search.results.clear();
                self.dlg.search.result_msg.clear();
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button(RichText::new(tr!("Caută")).strong()).clicked() {
                    run = true;
                }
            });
        });
        ui.label(
            RichText::new(tr!("Interogare: {}", query::describe(&self.dlg.search.conds)))
                .monospace()
                .small(),
        );
        ui.label(
            RichText::new(tr!(
                "ȘI are prioritate față de SAU. Textele se compară fără majuscule și fără diacritice; numerele, ca numere."
            ))
            .weak()
            .small(),
        );
        if run {
            *act = Act::Query;
        }
        ui.separator();
        let results = self.dlg.search.results.clone();
        if !self.dlg.search.result_msg.is_empty() {
            ui.strong(&self.dlg.search.result_msg);
        }
        if !results.is_empty() {
            self.hits_buttons(ui, &results, act);
            self.hit_list(ui, &results, act, "q_results");
        }
    }

    fn apply_act(&mut self, act: Act) {
        match act {
            Act::None => {}
            Act::PlaceSearch => self.start_place_search(),
            Act::Place(p) => self.go_to_place(&p),
            Act::ClearMarker => self.search_marker = None,
            Act::GoogleMaps => self.open_google_maps(),
            Act::OpenUrl(u) => {
                if let Err(e) = super::manual::open_external(std::ffi::OsStr::new(&u)) {
                    self.errors.push(tr!("Browserul nu s-a putut deschide: {e:#}"));
                }
            }
            Act::ZoomHit((lid, fi)) => {
                self.zoom_to_feature(lid, fi);
                self.dlg.table.layer_id = Some(lid);
                self.dlg.table.scroll_to = Some(fi);
            }
            Act::ZoomAll(h) => {
                if let Some(b) = self.hits_bounds(&h) {
                    self.zoom_to_padded(b);
                }
            }
            Act::Select(h, mode) => self.select_hits(&h, mode),
            Act::Query => {
                let conds = self.dlg.search.conds.clone();
                let hits = self.run_query(self.dlg.search.layer, &conds);
                let mut ids: Vec<u64> = hits.iter().map(|h| h.0).collect();
                ids.dedup();
                self.dlg.search.result_msg = match (hits.len(), ids.len()) {
                    (0, _) => tr!("Nicio entitate nu îndeplinește condițiile.").into(),
                    (1, _) => tr!("O entitate găsită").into(),
                    (n, 1) => {
                        let de = cadastru::i18n::de(n);
                        tr!("{n} {de}entități găsite")
                    }
                    (n, k) => {
                        let de = cadastru::i18n::de(n);
                        tr!("{n} {de}entități găsite în {k} straturi")
                    }
                };
                self.status = self.dlg.search.result_msg.clone();
                self.dlg.search.results = hits;
            }
        }
    }
}

enum Act {
    None,
    PlaceSearch,
    Place(Place),
    ClearMarker,
    GoogleMaps,
    OpenUrl(String),
    ZoomHit(Hit),
    ZoomAll(Vec<Hit>),
    Select(Vec<Hit>, SelMode),
    Query,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coords() {
        assert_eq!(parse_coords("44.85, 24.87"), Some((44.85, 24.87)));
        assert_eq!(parse_coords("44,85 24,87"), Some((44.85, 24.87)));
        assert_eq!(parse_coords("44.85 24.87"), Some((44.85, 24.87)));
        assert_eq!(parse_coords("466575.5 366107.5"), Some((466575.5, 366107.5)));
        assert_eq!(parse_coords("44.85;24.87"), Some((44.85, 24.87)));
        assert_eq!(parse_coords("Pitești"), None);
        assert_eq!(parse_coords("Str. Victoriei 10"), None);
        assert_eq!(parse_coords("1 2 3"), None);
    }

    #[test]
    fn encode() {
        assert_eq!(url_encode("Pitești, Argeș"), "Pite%C8%99ti%2C+Arge%C8%99");
    }

    #[test]
    fn nominatim_json() {
        let body = r#"[{"place_id":1,"lat":"44.8565","lon":"24.8692","category":"boundary","type":"administrative",
            "display_name":"Pitești, Argeș, România","boundingbox":["44.8","44.9","24.8","24.95"]},
            {"lat":"45","lon":"25","display_name":"X","type":"yes"}]"#;
        let v = parse_nominatim(body).unwrap();
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].name, "Pitești, Argeș, România");
        assert!((v[0].lat - 44.8565).abs() < 1e-9);
        assert_eq!(v[0].bbox, Some([24.8, 44.8, 24.95, 44.9]));
        assert_eq!(v[0].kind, "boundary / administrative");
        assert_eq!(v[1].kind, "");
        assert!(parse_nominatim("{}").is_err());
    }
}

#[cfg(test)]
mod heading_tests {
    use super::*;

    fn place(kind: &str, src: PlaceSrc) -> Place {
        Place { name: "x".into(), kind: kind.into(), lon: 24.0, lat: 45.0, bbox: None, src }
    }

    #[test]
    fn heading_follows_results_not_selected_source() {
        cadastru::i18n::set_lang(cadastru::i18n::Lang::Ro);
        // link Google Maps căutat cu sursa OpenStreetMap
        let p = [place("Google Maps", PlaceSrc::Google)];
        assert_eq!(places_heading(&p, Geocoder::Osm), "Loc din linkul Google Maps");
        let p = [place("Plus Code", PlaceSrc::Google)];
        assert_eq!(places_heading(&p, Geocoder::Osm), "Loc din codul Plus");
        let p = [place("city", PlaceSrc::Osm)];
        assert_eq!(places_heading(&p, Geocoder::Google), "Locuri (OpenStreetMap)");
        let p = [place("city", PlaceSrc::Osm), place("locality", PlaceSrc::Google)];
        assert_eq!(places_heading(&p, Geocoder::Osm), "Locuri (Google și OpenStreetMap)");
        // fără rezultate: sursa interogată
        assert_eq!(places_heading(&[], Geocoder::Google), "Locuri (Google)");
    }
}
