//! Căutarea locurilor prin Google: Places API (New) / Geocoding API cu cheia utilizatorului,
//! linkuri Google Maps lipite în căutare (inclusiv cele scurte, maps.app.goo.gl),
//! coduri Plus (Open Location Code) și deschiderea căutării în Google Maps din browser.

use super::search::{Place, PlaceSrc, url_encode};
use cadastru::i18n::tr;
use std::sync::atomic::{AtomicU8, Ordering};

const PLACES: &str = "https://places.googleapis.com/v1/places:searchText";
const GEOCODE: &str = "https://maps.googleapis.com/maps/api/geocode/json";

/// Cheia efectivă: cea din setări sau variabila de mediu GOOGLE_MAPS_API_KEY.
pub fn effective_key(setting: &str) -> String {
    let k = setting.trim();
    if !k.is_empty() {
        return k.to_string();
    }
    std::env::var("GOOGLE_MAPS_API_KEY")
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

fn read_body(resp: &mut ureq::http::Response<ureq::Body>) -> Result<String, String> {
    resp.body_mut()
        .with_config()
        .limit(8 * 1024 * 1024)
        .read_to_string()
        .map_err(|e| e.to_string())
}

/// Ce API Google a mers ultima dată: 0 = necunoscut, 1 = Places, 2 = Geocoding.
/// Dacă Places nu e activat pentru cheie, nu îl mai încercăm la fiecare căutare.
static WORKING_API: AtomicU8 = AtomicU8::new(0);

/// Caută ca în caseta Google Maps: Places API (New) Text Search, apoi Geocoding API
/// dacă Places nu e disponibil pentru cheie (sau nu găsește nimic).
pub fn search(q: &str, ro_only: bool, lang: &str, key: &str) -> Result<Vec<Place>, String> {
    if key.is_empty() {
        return Err(tr!("lipsește cheia API Google (Căutare > Loc / adresă > Cheie API)").into());
    }
    let mut errs = vec![];
    if WORKING_API.load(Ordering::Relaxed) != 2 {
        match places_text_search(q, ro_only, lang, key) {
            Ok(v) if !v.is_empty() => {
                WORKING_API.store(1, Ordering::Relaxed);
                return Ok(v);
            }
            Ok(_) => {}
            Err(e) => errs.push(format!("Places API: {e}")),
        }
    }
    match geocode(q, ro_only, lang, key) {
        Ok(v) => {
            if !errs.is_empty() {
                WORKING_API.store(2, Ordering::Relaxed);
            }
            Ok(v)
        }
        Err(e) => {
            errs.push(format!("Geocoding API: {e}"));
            Err(errs.join("; "))
        }
    }
}

fn places_text_search(q: &str, ro_only: bool, lang: &str, key: &str) -> Result<Vec<Place>, String> {
    let mut body = serde_json::json!({
        "textQuery": q,
        "languageCode": lang,
        "pageSize": 10,
    });
    if ro_only {
        body["regionCode"] = "RO".into();
        body["locationRestriction"] = serde_json::json!({
            "rectangle": {
                "low": { "latitude": 43.5, "longitude": 20.2 },
                "high": { "latitude": 48.35, "longitude": 29.8 }
            }
        });
    }
    let agent = super::basemap::make_agent_with(10, false);
    let mut resp = agent
        .post(PLACES)
        .header("Content-Type", "application/json")
        .header("X-Goog-Api-Key", key)
        .header(
            "X-Goog-FieldMask",
            "places.displayName,places.formattedAddress,places.location,places.viewport,places.primaryTypeDisplayName,places.types",
        )
        .send(body.to_string())
        .map_err(|e| e.to_string())?;
    let text = read_body(&mut resp)?;
    parse_places(&text)
}

fn geocode(q: &str, ro_only: bool, lang: &str, key: &str) -> Result<Vec<Place>, String> {
    let mut url = format!(
        "{GEOCODE}?address={}&language={lang}&key={}",
        url_encode(q),
        url_encode(key)
    );
    if ro_only {
        url.push_str("&region=ro&components=country:RO");
    }
    let agent = super::basemap::make_agent_with(10, false);
    let mut resp = agent.get(&url).call().map_err(|e| e.to_string())?;
    let text = read_body(&mut resp)?;
    parse_geocode(&text)
}

fn latlng(v: &serde_json::Value, la: &str, lo: &str) -> Option<(f64, f64)> {
    Some((v.get(la)?.as_f64()?, v.get(lo)?.as_f64()?))
}

/// Răspunsul Places API (New) Text Search.
pub fn parse_places(body: &str) -> Result<Vec<Place>, String> {
    let v: serde_json::Value = serde_json::from_str(body).map_err(|e| e.to_string())?;
    if let Some(e) = v.get("error") {
        let msg = e.get("message").and_then(|m| m.as_str()).unwrap_or("");
        let st = e.get("status").and_then(|m| m.as_str()).unwrap_or("");
        return Err(format!("{st} {msg}").trim().to_string());
    }
    let Some(arr) = v.get("places").and_then(|p| p.as_array()) else {
        // răspuns gol = niciun rezultat
        return Ok(vec![]);
    };
    Ok(arr
        .iter()
        .filter_map(|p| {
            let (lat, lon) = latlng(p.get("location")?, "latitude", "longitude")?;
            let title = p
                .get("displayName")
                .and_then(|d| d.get("text"))
                .and_then(|t| t.as_str())
                .unwrap_or("");
            let addr = p.get("formattedAddress").and_then(|t| t.as_str()).unwrap_or("");
            let name = if title.is_empty() || addr.starts_with(title) {
                addr.to_string()
            } else if addr.is_empty() {
                title.to_string()
            } else {
                format!("{title}, {addr}")
            };
            let bbox = p.get("viewport").and_then(|vp| {
                let (s, w) = latlng(vp.get("low")?, "latitude", "longitude")?;
                let (n, e) = latlng(vp.get("high")?, "latitude", "longitude")?;
                Some([w, s, e, n])
            });
            let kind = p
                .get("primaryTypeDisplayName")
                .and_then(|d| d.get("text"))
                .and_then(|t| t.as_str())
                .map(str::to_string)
                .or_else(|| {
                    p.get("types")
                        .and_then(|t| t.as_array())
                        .map(|t| t.iter().filter_map(|x| x.as_str()).take(2).collect::<Vec<_>>().join(" / "))
                })
                .unwrap_or_default();
            Some(Place { name, kind, lon, lat, bbox, src: PlaceSrc::Google })
        })
        .collect())
}

/// Răspunsul Geocoding API.
pub fn parse_geocode(body: &str) -> Result<Vec<Place>, String> {
    let v: serde_json::Value = serde_json::from_str(body).map_err(|e| e.to_string())?;
    let status = v.get("status").and_then(|s| s.as_str()).unwrap_or("");
    match status {
        "OK" => {}
        "ZERO_RESULTS" => return Ok(vec![]),
        _ => {
            let msg = v.get("error_message").and_then(|m| m.as_str()).unwrap_or("");
            return Err(format!("{status} {msg}").trim().to_string());
        }
    }
    let arr = v.get("results").and_then(|r| r.as_array()).cloned().unwrap_or_default();
    Ok(arr
        .iter()
        .filter_map(|r| {
            let g = r.get("geometry")?;
            let (lat, lon) = latlng(g.get("location")?, "lat", "lng")?;
            let bbox = g.get("bounds").or_else(|| g.get("viewport")).and_then(|vp| {
                let (s, w) = latlng(vp.get("southwest")?, "lat", "lng")?;
                let (n, e) = latlng(vp.get("northeast")?, "lat", "lng")?;
                Some([w, s, e, n])
            });
            let kind = r
                .get("types")
                .and_then(|t| t.as_array())
                .map(|t| t.iter().filter_map(|x| x.as_str()).take(2).collect::<Vec<_>>().join(" / "))
                .unwrap_or_default();
            Some(Place {
                name: r.get("formatted_address")?.as_str()?.to_string(),
                kind,
                lon,
                lat,
                bbox,
                src: PlaceSrc::Google,
            })
        })
        .collect())
}

// ---------------------------------------------------------------------------
// Linkuri Google Maps
// ---------------------------------------------------------------------------

/// Decodare procentuală (și „+” = spațiu) pentru părți de URL.
pub fn url_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'%' if i + 2 < b.len() => {
                match u8::from_str_radix(&s[i + 1..i + 3], 16) {
                    Ok(v) => {
                        out.push(v);
                        i += 3;
                        continue;
                    }
                    Err(_) => out.push(b'%'),
                }
            }
            b'+' => out.push(b' '),
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Este un link Google Maps (lung sau scurt)?
pub fn is_maps_link(s: &str) -> bool {
    let t = s.trim().to_ascii_lowercase();
    let t = t.trim_start_matches("https://").trim_start_matches("http://");
    t.starts_with("maps.app.goo.gl/")
        || t.starts_with("goo.gl/maps")
        || t.starts_with("g.co/kgs/")
        || t.starts_with("maps.google.")
        || (t.starts_with("www.google.") || t.starts_with("google.")) && t.contains("/maps")
}

/// Link scurt, care trebuie urmat (redirecționare) până la adresa completă.
pub fn is_short_link(s: &str) -> bool {
    let t = s.trim().to_ascii_lowercase();
    let t = t.trim_start_matches("https://").trim_start_matches("http://");
    t.starts_with("maps.app.goo.gl/") || t.starts_with("goo.gl/maps") || t.starts_with("g.co/kgs/")
}

fn two_nums(s: &str) -> Option<(f64, f64)> {
    let mut it = s.split(',');
    let a: f64 = it.next()?.trim().parse().ok()?;
    let b: f64 = it.next()?.trim().parse().ok()?;
    (a.abs() <= 90.0 && b.abs() <= 180.0).then_some((a, b))
}

fn query_param<'a>(url: &'a str, name: &str) -> Option<&'a str> {
    let q = url.split_once('?')?.1;
    let q = q.split('#').next().unwrap_or(q);
    q.split('&').find_map(|kv| {
        let (k, v) = kv.split_once('=')?;
        (k == name).then_some(v)
    })
}

/// Locul dintr-un link Google Maps complet: pinul (!3d…!4d…) are prioritate față de
/// centrul vederii (@lat,lon,zoom); recunoaște și ?q=lat,lon, ?query=, ?ll=, ?center=.
pub fn parse_maps_url(url: &str) -> Option<Place> {
    let u = url.trim();
    // pagina de consimțământ (UE) păstrează adresa originală în „continue”
    if u.contains("consent.google.") {
        if let Some(c) = query_param(u, "continue") {
            return parse_maps_url(&url_decode(c));
        }
    }
    let path = u.split('?').next().unwrap_or(u);
    // numele locului: /maps/place/<nume>/
    let name = path
        .split("/place/")
        .nth(1)
        .and_then(|r| r.split('/').next())
        .map(url_decode)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty() && two_nums(s).is_none());

    // 1) pinul: !3d<lat>!4d<lon> (ultima pereche)
    let mut pin = None;
    let mut rest = u;
    while let Some(i) = rest.find("!3d") {
        let after = &rest[i + 3..];
        let lat_s: String = after.chars().take_while(|c| c.is_ascii_digit() || *c == '.' || *c == '-').collect();
        if let Some(j) = after.find("!4d") {
            let lon_s: String = after[j + 3..]
                .chars()
                .take_while(|c| c.is_ascii_digit() || *c == '.' || *c == '-')
                .collect();
            if let (Ok(a), Ok(b)) = (lat_s.parse::<f64>(), lon_s.parse::<f64>()) {
                pin = Some((a, b));
            }
        }
        rest = after;
    }
    // 2) parametri de interogare
    let from_q = ["q", "query", "ll", "center", "destination", "daddr"]
        .iter()
        .find_map(|k| query_param(u, k).map(url_decode).and_then(|v| two_nums(&v)));
    // 3) centrul vederii: /@lat,lon,zoomz
    let at = path.split("/@").nth(1).and_then(|r| {
        let mut it = r.split(',');
        let a: f64 = it.next()?.parse().ok()?;
        let b: f64 = it.next()?.parse().ok()?;
        (a.abs() <= 90.0 && b.abs() <= 180.0).then_some((a, b))
    });
    // /maps/search/<lat>,+<lon>
    let search_coords = path
        .split("/search/")
        .nth(1)
        .and_then(|r| r.split('/').next())
        .map(url_decode)
        .and_then(|s| two_nums(&s.replace(' ', "")));
    let (lat, lon) = pin.or(from_q).or(search_coords).or(at)?;
    // nivelul de zoom din link (@lat,lon,13z) dă mărimea zonei afișate în Google Maps
    let zoom = path.split("/@").nth(1).and_then(|r| {
        let z = r.split(',').nth(2)?;
        let z = z.split('/').next()?.strip_suffix('z')?;
        z.parse::<f64>().ok().filter(|z| (1.0..=22.0).contains(z))
    });
    let bbox = zoom.map(|z| {
        // aproximativ 1000 × 700 px de hartă Google la acel nivel
        let w = 360.0 / 2f64.powf(z) * 1000.0 / 256.0;
        let h = w * 0.7 * lat.to_radians().cos();
        [lon - w / 2.0, lat - h / 2.0, lon + w / 2.0, lat + h / 2.0]
    });
    Some(Place {
        name: name.unwrap_or_else(|| format!("{lat:.6}, {lon:.6}")),
        kind: "Google Maps".into(),
        lon,
        lat,
        bbox,
        src: PlaceSrc::Google,
    })
}

/// Urmează redirecționările unui link scurt (maps.app.goo.gl) și citește locul din adresa finală.
pub fn resolve_short_link(url: &str) -> Result<Place, String> {
    let agent = super::basemap::make_agent_with(0, false);
    let mut cur = url.trim().to_string();
    if !cur.starts_with("http") {
        cur = format!("https://{cur}");
    }
    for _ in 0..8 {
        if !is_short_link(&cur)
            && let Some(p) = parse_maps_url(&cur)
        {
            return Ok(p);
        }
        let resp = agent.get(&cur).call().map_err(|e| e.to_string())?;
        let st = resp.status().as_u16();
        if !(300..400).contains(&st) {
            break;
        }
        let Some(loc) = resp.headers().get("location").and_then(|h| h.to_str().ok()) else {
            break;
        };
        cur = if loc.starts_with('/') {
            let base: String = cur.splitn(4, '/').take(3).collect::<Vec<_>>().join("/");
            format!("{base}{loc}")
        } else {
            loc.to_string()
        };
    }
    parse_maps_url(&cur).ok_or_else(|| tr!("linkul Google Maps nu conține coordonate").into())
}

// ---------------------------------------------------------------------------
// Coduri Plus (Open Location Code), afișate de Google Maps pentru orice loc
// ---------------------------------------------------------------------------

const OLC: &[u8; 20] = b"23456789CFGHJMPQRVWX";
const OLC_RES: [f64; 5] = [20.0, 1.0, 0.05, 0.0025, 0.000125];

fn olc_val(c: u8) -> Option<usize> {
    let c = c.to_ascii_uppercase();
    OLC.iter().position(|&x| x == c)
}

/// Codul Plus din text: „8GP8PV9C+2W” (complet) sau „PV9C+2W Pitești” (scurt + localitate).
/// Întoarce (cod, restul textului).
pub fn find_plus_code(s: &str) -> Option<(String, String)> {
    let t = s.trim();
    let (code, rest) = match t.split_once(char::is_whitespace) {
        Some((a, b)) => (a, b.trim()),
        None => (t, ""),
    };
    let code = code.trim_end_matches(',');
    let (pre, post) = code.split_once('+')?;
    if pre.len() < 2 || pre.len() > 8 || pre.len() % 2 == 1 || post.len() == 1 || post.len() > 7 {
        return None;
    }
    let pre_ok = pre.bytes().enumerate().all(|(i, c)| olc_val(c).is_some() || (c == b'0' && i >= 2 && pre.len() == 8));
    if !pre_ok || !post.bytes().all(|c| olc_val(c).is_some()) {
        return None;
    }
    // un cod scurt fără localitate e ambiguu doar dacă nu avem o referință; îl acceptăm
    Some((code.to_ascii_uppercase(), rest.trim_start_matches(',').trim().to_string()))
}

pub fn plus_is_full(code: &str) -> bool {
    code.find('+') == Some(8)
}

/// Decodează un cod complet: (lat, lon) al centrului și latura celulei în grade.
pub fn plus_decode(code: &str) -> Option<(f64, f64, f64)> {
    let digits: Vec<u8> = code.bytes().filter(|&c| c != b'+' && c != b'0').collect();
    if digits.len() < 2 {
        return None;
    }
    let (mut lat, mut lon) = (-90.0f64, -180.0f64);
    let mut res = 20.0;
    for (i, pair) in digits.chunks(2).take(5).enumerate() {
        res = OLC_RES[i];
        lat += olc_val(pair[0])? as f64 * res;
        if let Some(&c) = pair.get(1) {
            lon += olc_val(c)? as f64 * res;
        }
    }
    let (mut hlat, mut hlon) = (res, res);
    // al 11-lea caracter și următoarele: grilă 5 rânduri × 4 coloane
    for &c in digits.iter().skip(10).take(5) {
        let v = olc_val(c)?;
        hlat /= 5.0;
        hlon /= 4.0;
        lat += (v / 4) as f64 * hlat;
        lon += (v % 4) as f64 * hlon;
    }
    Some((lat + hlat / 2.0, lon + hlon / 2.0, hlat.max(hlon)))
}

/// Primele `n` cifre (n par, ≤ 10) ale codului pentru un punct.
fn plus_encode_prefix(lat: f64, lon: f64, n: usize) -> String {
    let mut la = (lat + 90.0).clamp(0.0, 179.999_999_9);
    let mut lo = (lon + 180.0).rem_euclid(360.0);
    let mut out = String::new();
    for &res in OLC_RES.iter().take(n / 2) {
        let a = (la / res).floor() as usize;
        let b = (lo / res).floor() as usize;
        la -= a as f64 * res;
        lo -= b as f64 * res;
        out.push(OLC[a.min(19)] as char);
        out.push(OLC[b.min(19)] as char);
    }
    out
}

/// Reface un cod scurt față de un punct de referință (algoritmul oficial „recoverNearest”).
pub fn plus_recover(short: &str, ref_lat: f64, ref_lon: f64) -> Option<(f64, f64, f64)> {
    if plus_is_full(short) {
        return plus_decode(short);
    }
    let plus = short.find('+')?;
    let pad = 8 - plus;
    let resolution = 20f64.powi(2 - (pad as i32) / 2);
    let half = resolution / 2.0;
    let rlat = (ref_lat / resolution).floor() * resolution;
    let rlon = (ref_lon / resolution).floor() * resolution;
    let full = format!("{}{}", plus_encode_prefix(rlat + 1e-9, rlon + 1e-9, pad), short);
    let (mut lat, mut lon, size) = plus_decode(&full)?;
    if ref_lat + half < lat && lat - resolution >= -90.0 {
        lat -= resolution;
    } else if ref_lat - half > lat && lat + resolution <= 90.0 {
        lat += resolution;
    }
    if ref_lon + half < lon {
        lon -= resolution;
    } else if ref_lon - half > lon {
        lon += resolution;
    }
    Some((lat, lon, size))
}

// ---------------------------------------------------------------------------
// Google Maps în browser
// ---------------------------------------------------------------------------

/// Adresa căutării în Google Maps (API-ul de URL, fără cheie).
pub fn maps_search_url(q: &str) -> String {
    format!("https://www.google.com/maps/search/?api=1&query={}", url_encode(q))
}

/// Adresa Google Maps centrată pe un punct, la un nivel de zoom.
pub fn maps_view_url(lat: f64, lon: f64, zoom: u8) -> String {
    format!("https://www.google.com/maps/@?api=1&map_action=map&center={lat:.6}%2C{lon:.6}&zoom={zoom}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_links() {
        let u = "https://www.google.com/maps/place/Pite%C8%99ti/@44.8564798,24.8691824,13z/data=!3m1!4b1!4m6!3m5!1s0x40b2bc!8m2!3d44.8605!4d24.8678!16zL20vMDM0eGJk";
        let p = parse_maps_url(u).unwrap();
        assert_eq!(p.name, "Pitești");
        assert!((p.lat - 44.8605).abs() < 1e-9 && (p.lon - 24.8678).abs() < 1e-9);
        let p = parse_maps_url("https://www.google.com/maps/@44.4268,26.1025,15z").unwrap();
        assert!((p.lat - 44.4268).abs() < 1e-9 && (p.lon - 26.1025).abs() < 1e-9);
        // zona din nivelul de zoom: la 15z circa 0,04° în lățime
        let [w, _, e, _] = p.bbox.unwrap();
        assert!((e - w - 0.0429).abs() < 0.001, "{}", e - w);
        let p = parse_maps_url("https://maps.google.com/?q=45.65,25.6").unwrap();
        assert!((p.lat - 45.65).abs() < 1e-9);
        let p = parse_maps_url("https://www.google.com/maps/search/44.43,+26.10?entry=tts").unwrap();
        assert!((p.lon - 26.10).abs() < 1e-9);
        let c = "https://consent.google.com/m?continue=https://www.google.com/maps/place/X/%4044.1,25.2,12z/data%3D!3d44.2!4d25.3&gl=RO";
        let p = parse_maps_url(c).unwrap();
        assert!((p.lat - 44.2).abs() < 1e-9 && (p.lon - 25.3).abs() < 1e-9);
        assert!(parse_maps_url("https://www.google.com/maps").is_none());
        assert!(is_maps_link("https://maps.app.goo.gl/AbCdEf"));
        assert!(is_short_link("maps.app.goo.gl/AbCdEf"));
        assert!(is_maps_link("https://www.google.ro/maps/place/x"));
        assert!(!is_maps_link("https://example.com/maps"));
    }

    #[test]
    fn plus_codes() {
        // exemplul din specificație: 8FVC9G8F+6X → 47.3655625, 8.5249375 (Zürich)
        let (lat, lon, _) = plus_decode("8FVC9G8F+6X").unwrap();
        assert!((lat - 47.365562).abs() < 1e-5, "{lat}");
        assert!((lon - 8.5249375).abs() < 1e-7, "{lon}");
        // cod scurt refăcut lângă referință
        let (lat2, lon2, _) = plus_recover("9G8F+6X", 47.4, 8.6).unwrap();
        assert!((lat2 - lat).abs() < 1e-9 && (lon2 - lon).abs() < 1e-9);
        // codul pentru București, refăcut din prefixul calculat
        let pre = plus_encode_prefix(44.4268, 26.1025, 4);
        let full = format!("{pre}CJ00+");
        assert!(plus_decode(&full).is_some());
        assert_eq!(find_plus_code("9G8F+6X Zürich").unwrap(), ("9G8F+6X".into(), "Zürich".into()));
        assert!(find_plus_code("8FVC9G8F+6X").is_some());
        assert!(find_plus_code("Str. Victoriei 10").is_none());
        assert!(find_plus_code("1+2").is_none());
    }

    #[test]
    fn google_json() {
        let g = r#"{"results":[{"formatted_address":"Pitești, Romania","geometry":{"location":{"lat":44.8565,"lng":24.8692},"viewport":{"northeast":{"lat":44.9,"lng":24.95},"southwest":{"lat":44.8,"lng":24.8}}},"types":["locality","political"]}],"status":"OK"}"#;
        let v = parse_geocode(g).unwrap();
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].bbox, Some([24.8, 44.8, 24.95, 44.9]));
        assert!(parse_geocode(r#"{"results":[],"status":"REQUEST_DENIED","error_message":"The provided API key is invalid."}"#)
            .unwrap_err()
            .contains("invalid"));
        assert!(parse_geocode(r#"{"results":[],"status":"ZERO_RESULTS"}"#).unwrap().is_empty());
        let p = r#"{"places":[{"formattedAddress":"Bulevardul Regina Elisabeta 2, București, Romania","location":{"latitude":44.435,"longitude":26.1},"viewport":{"low":{"latitude":44.43,"longitude":26.09},"high":{"latitude":44.44,"longitude":26.11}},"displayName":{"text":"Universitatea din București","languageCode":"ro"},"primaryTypeDisplayName":{"text":"Universitate"}}]}"#;
        let v = parse_places(p).unwrap();
        assert_eq!(v[0].name, "Universitatea din București, Bulevardul Regina Elisabeta 2, București, Romania");
        assert_eq!(v[0].kind, "Universitate");
        assert!(parse_places("{}").unwrap().is_empty());
        assert!(parse_places(r#"{"error":{"code":403,"message":"Places API (New) has not been used","status":"PERMISSION_DENIED"}}"#).is_err());
    }

    #[test]
    fn decode_url() {
        assert_eq!(url_decode("Pite%C8%99ti+jud"), "Pitești jud");
        assert_eq!(url_decode("100%"), "100%");
    }
}
