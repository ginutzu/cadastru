//! Actualizarea automată a listei SIRUTA de pe data.gov.ro (setul oficial publicat de INS)
//! și păstrarea unei copii locale pentru funcționarea offline (preluat din proiectul SIRUTA).
//!
//! Cache-ul este același cu al aplicației SIRUTA de sine stătătoare: `%LOCALAPPDATA%\SIRUTA`.

use super::data::{self, Dataset};
use cadastru::i18n::tr;
use chrono::{DateTime, Local, Utc};
use serde_json::{Value, json};
use std::io::Read;
use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::time::Duration;

const CKAN_SEARCH: &str = "https://data.gov.ro/api/3/action/package_search?\
fq=organization:institutul-national-de-statistica&q=siruta&sort=metadata_modified%20desc&rows=100";
const MAX_CSV_BYTES: u64 = 60 * 1024 * 1024;

#[derive(Clone, Debug, Default)]
pub struct Meta {
    /// Momentul ultimei actualizari reusite (UTC, RFC 3339).
    pub last_success: Option<String>,
    pub resource_name: String,
    pub resource_url: String,
    /// Data publicarii/modificarii resursei pe data.gov.ro.
    pub resource_modified: String,
    pub dataset_title: String,
}

impl Meta {
    pub fn last_success_local(&self) -> Option<DateTime<Local>> {
        self.last_success
            .as_deref()
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|d| d.with_timezone(&Local))
    }

    fn to_json(&self) -> Value {
        json!({
            "last_success": self.last_success,
            "resource_name": self.resource_name,
            "resource_url": self.resource_url,
            "resource_modified": self.resource_modified,
            "dataset_title": self.dataset_title,
        })
    }

    fn from_json(v: &Value) -> Meta {
        let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
        Meta {
            last_success: v
                .get("last_success")
                .and_then(|x| x.as_str())
                .map(|x| x.to_string()),
            resource_name: s("resource_name"),
            resource_url: s("resource_url"),
            resource_modified: s("resource_modified"),
            dataset_title: s("dataset_title"),
        }
    }
}

/// Folderul de date: %LOCALAPPDATA%\SIRUTA (Windows) sau ~/.local/share/SIRUTA.
pub fn data_dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local").join("share"))
        })
        .unwrap_or_else(exe_dir);
    let d = base.join("SIRUTA");
    let _ = std::fs::create_dir_all(&d);
    d
}

fn exe_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."))
}

fn csv_path() -> PathBuf {
    data_dir().join("siruta.csv")
}
fn meta_path() -> PathBuf {
    data_dir().join("meta.json")
}

pub fn load_meta() -> Meta {
    std::fs::read_to_string(meta_path())
        .ok()
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .map(|v| Meta::from_json(&v))
        .unwrap_or_default()
}

fn save_meta(m: &Meta) -> std::io::Result<()> {
    let tmp = meta_path().with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(&m.to_json()).unwrap())?;
    std::fs::rename(tmp, meta_path())
}

/// Incarca datele salvate local. Daca nu exista inca, cauta un fisier `siruta.csv`
/// langa executabil (pentru prima pornire fara internet).
pub fn load_local() -> Option<(Dataset, Meta, bool)> {
    if let Ok(bytes) = std::fs::read(csv_path()) {
        if let Ok(ds) = data::parse(&data::decode(&bytes)) {
            return Some((ds, load_meta(), false));
        }
    }
    let seed = exe_dir().join("siruta.csv");
    if let Ok(bytes) = std::fs::read(&seed) {
        if let Ok(ds) = data::parse(&data::decode(&bytes)) {
            let m = Meta {
                resource_name: tr!("siruta.csv (fișier local lângă aplicație)").into(),
                ..Default::default()
            };
            return Some((ds, m, true));
        }
    }
    None
}

pub enum Msg {
    Progress(String),
    Done(Result<UpdateOk, String>),
}

pub struct UpdateOk {
    /// `Some` daca a fost descarcata o editie noua.
    pub dataset: Option<Dataset>,
    pub meta: Meta,
}

pub fn spawn_update(tx: Sender<Msg>, ctx: eframe::egui::Context, force: bool) {
    let vp = super::viewport_id();
    std::thread::spawn(move || {
        let progress = |s: &str| {
            let _ = tx.send(Msg::Progress(s.to_string()));
            ctx.request_repaint_of(vp);
        };
        let res = run_update(force, &progress);
        let _ = tx.send(Msg::Done(res));
        ctx.request_repaint_of(vp);
    });
}

/// Agentul HTTP: TLS cu certificatele sistemului, proxy din mediu sau din setările Windows
/// (ca restul aplicației), conectare max. 10 s, citire max. 60 s.
fn agent() -> Result<ureq::Agent, String> {
    use ureq::tls::{RootCerts, TlsConfig};
    Ok(ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(10)))
        .timeout_recv_response(Some(Duration::from_secs(60)))
        .timeout_recv_body(Some(Duration::from_secs(60)))
        .user_agent(concat!("Cadastru-SIRUTA/", env!("CARGO_PKG_VERSION"), " (aplicatie desktop)"))
        .proxy(crate::gui::basemap::proxy())
        .tls_config(TlsConfig::builder().root_certs(RootCerts::PlatformVerifier).build())
        .build()
        .new_agent())
}

struct Resource {
    name: String,
    url: String,
    modified: String,
    dataset_title: String,
}

/// Alege cea mai recenta resursa CSV SIRUTA publicata de INS.
fn pick_latest(v: &Value) -> Option<Resource> {
    let results = v.pointer("/result/results")?.as_array()?;
    let mut best: Option<Resource> = None;
    for pkg in results {
        let org = pkg
            .pointer("/organization/name")
            .and_then(|x| x.as_str())
            .unwrap_or("");
        if !org.is_empty() && org != "institutul-national-de-statistica" {
            continue;
        }
        let title = pkg.get("title").and_then(|x| x.as_str()).unwrap_or("").to_string();
        let Some(res) = pkg.get("resources").and_then(|r| r.as_array()) else {
            continue;
        };
        for r in res {
            let gs = |k: &str| r.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
            let name = gs("name");
            let url = gs("url");
            let fmt = gs("format").to_lowercase();
            let lname = name.to_lowercase();
            let is_csv = fmt == "csv" || url.to_lowercase().ends_with(".csv");
            if !is_csv || !lname.contains("siruta") || lname.contains("zone") || lname.contains("jud")
            {
                continue;
            }
            if url.is_empty() {
                continue;
            }
            let modified = {
                let lm = gs("last_modified");
                if lm.is_empty() { gs("created") } else { lm }
            };
            let better = match &best {
                None => true,
                Some(b) => {
                    (edition(&name), modified.as_str()) > (edition(&b.name), b.modified.as_str())
                }
            };
            if better {
                best = Some(Resource {
                    name,
                    url,
                    modified,
                    dataset_title: title.clone(),
                });
            }
        }
    }
    best
}

/// Editia unui fisier SIRUTA dedusa din nume: (an, semestru).
/// Ex.: "siruta_s1_2026.csv" -> (2026, 1); "SIRUTA_AN_2025.csv" -> (2025, 2) (editia anuala,
/// de la sfarsitul anului); "SIRUTA_2023_1.csv" -> (2023, 1); fara an -> (0, 0).
fn edition(name: &str) -> (u16, u8) {
    let n = name.to_lowercase();
    let b = n.as_bytes();
    let mut year = 0u16;
    let mut after = 0usize;
    let mut i = 0;
    while i + 4 <= b.len() {
        let w = &b[i..i + 4];
        let bounded = (i == 0 || !b[i - 1].is_ascii_digit())
            && (i + 4 == b.len() || !b[i + 4].is_ascii_digit());
        if bounded && w.iter().all(|c| c.is_ascii_digit()) {
            let y: u16 = std::str::from_utf8(w).unwrap().parse().unwrap();
            if (1990..2200).contains(&y) {
                year = y;
                after = i + 4;
                break;
            }
        }
        i += 1;
    }
    let tok = |t: &str| {
        n.split(|c: char| !c.is_ascii_alphanumeric())
            .any(|x| x == t)
    };
    let sem = if tok("s2") || tok("sem2") || tok("an") {
        2
    } else if tok("s1") || tok("sem1") {
        1
    } else {
        // forma veche "SIRUTA_2023_1.csv"
        match (b.get(after), b.get(after + 1)) {
            (Some(b'_' | b'-'), Some(d @ b'1'..=b'2')) => d - b'0',
            _ => 0,
        }
    };
    (year, sem)
}

fn run_update(force: bool, progress: &dyn Fn(&str)) -> Result<UpdateOk, String> {
    let agent = agent()?;
    progress(tr!("Se verifică ultima ediție SIRUTA publicată de INS…"));
    let body = agent
        .get(CKAN_SEARCH)
        .call()
        .map_err(|e| tr!("Nu se poate contacta data.gov.ro ({})", short_err(&e)))?
        .body_mut()
        .with_config()
        .limit(16 * 1024 * 1024)
        .read_to_string()
        .map_err(|e| tr!("Răspuns invalid de la data.gov.ro: {e}"))?;
    let v: Value =
        serde_json::from_str(&body).map_err(|e| tr!("Răspuns JSON invalid: {e}"))?;
    let res = pick_latest(&v)
        .ok_or_else(|| tr!("Nu a fost găsit niciun fișier SIRUTA CSV pe data.gov.ro").to_string())?;

    let old = load_meta();
    let now = Utc::now().to_rfc3339();
    let have_local = csv_path().exists();
    if !force && have_local && old.resource_url == res.url && old.resource_modified == res.modified {
        let meta = Meta {
            last_success: Some(now),
            resource_name: res.name,
            resource_url: res.url,
            resource_modified: res.modified,
            dataset_title: res.dataset_title,
        };
        save_meta(&meta).map_err(|e| tr!("Nu se poate salva local: {e}"))?;
        return Ok(UpdateOk { dataset: None, meta });
    }

    progress(&tr!("Se descarcă {}…", res.name));
    let mut resp = agent
        .get(&res.url)
        .call()
        .map_err(|e| tr!("Descărcarea a eșuat ({})", short_err(&e)))?;
    let mut bytes = Vec::new();
    resp.body_mut()
        .as_reader()
        .take(MAX_CSV_BYTES)
        .read_to_end(&mut bytes)
        .map_err(|e| tr!("Descărcare întreruptă: {e}"))?;

    progress(tr!("Se verifică datele descărcate…"));
    let ds = data::parse(&data::decode(&bytes))?;

    let tmp = csv_path().with_extension("csv.tmp");
    std::fs::write(&tmp, &bytes).map_err(|e| tr!("Nu se poate salva local: {e}"))?;
    std::fs::rename(&tmp, csv_path()).map_err(|e| tr!("Nu se poate salva local: {e}"))?;
    let meta = Meta {
        last_success: Some(now),
        resource_name: res.name,
        resource_url: res.url,
        resource_modified: res.modified,
        dataset_title: res.dataset_title,
    };
    save_meta(&meta).map_err(|e| tr!("Nu se poate salva local: {e}"))?;
    Ok(UpdateOk {
        dataset: Some(ds),
        meta,
    })
}

fn short_err(e: &ureq::Error) -> String {
    match e {
        ureq::Error::StatusCode(code) => format!("HTTP {code}"),
        ureq::Error::HostNotFound => tr!("fără conexiune la internet / DNS").into(),
        ureq::Error::ConnectionFailed => tr!("conexiune eșuată").into(),
        ureq::Error::Timeout(_) | ureq::Error::Io(_) => {
            tr!("timp de așteptare depășit sau conexiune întreruptă").into()
        }
        k => k.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn picks_latest_csv() {
        let v = json!({"result":{"results":[
            {"title":"SIRUTA 2025","organization":{"name":"institutul-national-de-statistica"},
             "resources":[{"name":"Legenda SIRUTA.xml","format":"XML","url":"x.xml","last_modified":"2026-09-01T00:00:00"},
                          {"name":"SIRUTA_AN_2025.csv","format":"CSV","url":"a.csv","last_modified":"2026-01-22T07:30:38"}]},
            {"title":"siruta_s1-2026","organization":{"name":"institutul-national-de-statistica"},
             "resources":[{"name":"siruta_s1_2026.csv","format":"CSV","url":"b.csv","last_modified":"2026-08-11T11:42:48"}]},
            {"title":"vechi","organization":{"name":"institutul-national-de-statistica"},
             "resources":[{"name":"SIRUTA-ZONE.CSV","format":"CSV","url":"z.csv","last_modified":"2027-01-01"}]}
        ]}});
        let r = pick_latest(&v).unwrap();
        assert_eq!(r.url, "b.csv");
    }

    #[test]
    fn editions() {
        assert_eq!(edition("siruta_s1_2026.csv"), (2026, 1));
        assert_eq!(edition("SIRUTA_AN_2025.csv"), (2025, 2));
        assert_eq!(edition("SIRUTA_S1_2025.csv"), (2025, 1));
        assert_eq!(edition("SIRUTA_2023_1.csv"), (2023, 1));
        assert_eq!(edition("SIRUTA.CSV"), (0, 0));
        assert!(edition("siruta_s1_2026.csv") > edition("SIRUTA_AN_2025.csv"));
        assert!(edition("SIRUTA_AN_2025.csv") > edition("SIRUTA_S1_2025.csv"));
    }
}
