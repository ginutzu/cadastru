//! OGC GeoPackage (.gpkg) – straturi vectoriale (citire și scriere).

use super::{ReadOptions, WriteOptions};
use crate::crs::Crs;
use crate::geomio::{read_wkb, write_wkb};
use crate::model::*;
use anyhow::{Context, Result, anyhow, bail};
use rusqlite::{Connection, OpenFlags, params, types::ValueRef};
use std::path::Path;
use crate::i18n::tr;

fn qi(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\"\""))
}

/// Decodifică blob-ul de geometrie GeoPackage (antet „GP” + WKB).
pub fn decode_gpkg_geom(b: &[u8]) -> Result<Option<Geometry>> {
    if b.len() < 8 || &b[0..2] != b"GP" {
        // unele aplicații scriu WKB simplu
        return read_wkb(b).map(Some);
    }
    let flags = b[3];
    if flags & 0x10 != 0 {
        return Ok(None); // geometrie goală
    }
    let env = match (flags >> 1) & 0x07 {
        0 => 0,
        1 => 32,
        2 | 3 => 48,
        4 => 64,
        _ => bail!("Antet GeoPackage invalid"),
    };
    let off = 8 + env;
    if off > b.len() {
        bail!("Geometrie GeoPackage trunchiată");
    }
    read_wkb(&b[off..]).map(Some)
}

pub fn encode_gpkg_geom(g: &Geometry, srs_id: i32, z: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(64);
    out.extend_from_slice(b"GP");
    out.push(0); // versiune
    let empty = g.is_empty();
    let flags: u8 = 0x01 | if empty { 0x10 } else { 0x02 }; // little-endian, plic xy
    out.push(flags);
    out.extend_from_slice(&srs_id.to_le_bytes());
    if !empty {
        let b = g.bounds();
        for v in [b.minx, b.maxx, b.miny, b.maxy] {
            out.extend_from_slice(&v.to_le_bytes());
        }
    }
    write_wkb(g, z, &mut out);
    out
}

fn srs_to_crs(conn: &Connection, srs_id: i64) -> Option<Crs> {
    let (org, code, def): (String, i64, String) = conn
        .query_row("SELECT organization, organization_coordsys_id, definition FROM gpkg_spatial_ref_sys WHERE srs_id = ?1", [srs_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .ok()?;
    if org.eq_ignore_ascii_case("EPSG")
        && let Ok(c) = Crs::from_epsg(code as u32)
    {
        return Some(c);
    }
    Crs::from_wkt(&def).ok()
}

pub fn list_layers(path: &Path) -> Result<Vec<String>> {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let mut st = conn.prepare(
        "SELECT table_name FROM gpkg_contents WHERE data_type = 'features' ORDER BY table_name",
    )?;
    let v = st
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(v)
}

pub fn read(path: &Path, _opts: &ReadOptions) -> Result<Vec<VectorLayer>> {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .with_context(|| tr!("Nu pot deschide {}", path.display()))?;
    let tables =
        list_layers(path).context("Nu este un GeoPackage valid (lipsește gpkg_contents)")?;
    let mut out = vec![];
    for t in tables {
        let (gcol, srs_id, zflag): (String, i64, i64) = conn
            .query_row(
                "SELECT column_name, srs_id, z FROM gpkg_geometry_columns WHERE table_name = ?1",
                [&t],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .with_context(|| format!("Stratul {t} nu are coloană de geometrie"))?;
        // coloanele
        let mut cols: Vec<(String, String, bool)> = vec![];
        {
            let mut st = conn.prepare(&format!("PRAGMA table_info({})", qi(&t)))?;
            let rows = st.query_map([], |r| {
                Ok((
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, i64>(5)? > 0,
                ))
            })?;
            for r in rows {
                cols.push(r?);
            }
        }
        let attr_cols: Vec<&(String, String, bool)> = cols
            .iter()
            .filter(|(n, ty, pk)| {
                !n.eq_ignore_ascii_case(&gcol) && !(*pk && ty.to_uppercase().contains("INT"))
            })
            .collect();
        let mut layer = VectorLayer::new(t.clone());
        layer.source = Some(path.to_path_buf());
        layer.crs = srs_to_crs(&conn, srs_id);
        layer.has_z = zflag > 0;
        for (n, ty, _) in &attr_cols {
            let u = ty.to_uppercase();
            let ft = if u.contains("INT") {
                FieldType::Integer
            } else if u.contains("REAL")
                || u.contains("FLOAT")
                || u.contains("DOUBLE")
                || u.contains("NUMERIC")
            {
                FieldType::Real
            } else if u.contains("BOOL") {
                FieldType::Boolean
            } else if u.contains("DATE") {
                FieldType::Date
            } else {
                FieldType::Text
            };
            let mut f = Field::new(n.clone(), ft);
            if let Some(w) = u
                .strip_prefix("TEXT(")
                .and_then(|s| s.strip_suffix(')'))
                .and_then(|s| s.parse().ok())
            {
                f.width = w;
            }
            layer.fields.push(f);
        }
        let sel: Vec<String> = std::iter::once(qi(&gcol))
            .chain(attr_cols.iter().map(|(n, _, _)| qi(n)))
            .collect();
        let mut st = conn.prepare(&format!("SELECT {} FROM {}", sel.join(", "), qi(&t)))?;
        let mut rows = st.query([])?;
        while let Some(r) = rows.next()? {
            let geom = match r.get_ref(0)? {
                ValueRef::Blob(b) => decode_gpkg_geom(b).unwrap_or(None),
                _ => None,
            };
            let mut attrs = Vec::with_capacity(attr_cols.len());
            for (i, f) in layer.fields.iter().enumerate() {
                let v = match r.get_ref(i + 1)? {
                    ValueRef::Null => Value::Null,
                    ValueRef::Integer(x) => Value::Int(x),
                    ValueRef::Real(x) => Value::Real(x),
                    ValueRef::Text(s) => Value::Text(String::from_utf8_lossy(s).into_owned()),
                    ValueRef::Blob(_) => Value::Null,
                };
                attrs.push(v.coerce(f.ftype));
            }
            layer.features.push(Feature { geom, attrs });
        }
        out.push(layer);
    }
    if out.is_empty() {
        bail!("GeoPackage-ul nu conține straturi vectoriale");
    }
    Ok(out)
}

fn geometry_type_name(l: &VectorLayer) -> &'static str {
    let mut name: Option<&'static str> = None;
    for f in &l.features {
        if let Some(g) = &f.geom {
            let n = match g {
                Geometry::Point(_) => "POINT",
                Geometry::MultiPoint(_) => "MULTIPOINT",
                Geometry::LineString(_) => "LINESTRING",
                Geometry::MultiLineString(_) => "MULTILINESTRING",
                Geometry::Polygon(_) => "POLYGON",
                Geometry::MultiPolygon(_) => "MULTIPOLYGON",
                Geometry::Collection(_) => "GEOMETRYCOLLECTION",
            };
            match name {
                None => name = Some(n),
                Some(p) if p == n => {}
                Some(p) => {
                    // promovare simplu -> multi
                    let pair = |a: &str, b: &str| (p == a && n == b) || (p == b && n == a);
                    name = Some(if pair("POINT", "MULTIPOINT") {
                        "MULTIPOINT"
                    } else if pair("LINESTRING", "MULTILINESTRING") {
                        "MULTILINESTRING"
                    } else if pair("POLYGON", "MULTIPOLYGON") {
                        "MULTIPOLYGON"
                    } else {
                        return "GEOMETRY";
                    });
                }
            }
        }
    }
    name.unwrap_or("GEOMETRY")
}

fn init_gpkg(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "PRAGMA application_id = 1196444487;
         PRAGMA user_version = 10400;
         CREATE TABLE IF NOT EXISTS gpkg_spatial_ref_sys (
            srs_name TEXT NOT NULL, srs_id INTEGER NOT NULL PRIMARY KEY, organization TEXT NOT NULL,
            organization_coordsys_id INTEGER NOT NULL, definition TEXT NOT NULL, description TEXT);
         CREATE TABLE IF NOT EXISTS gpkg_contents (
            table_name TEXT NOT NULL PRIMARY KEY, data_type TEXT NOT NULL, identifier TEXT UNIQUE, description TEXT DEFAULT '',
            last_change DATETIME NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')),
            min_x DOUBLE, min_y DOUBLE, max_x DOUBLE, max_y DOUBLE, srs_id INTEGER,
            CONSTRAINT fk_gc_r_srs_id FOREIGN KEY (srs_id) REFERENCES gpkg_spatial_ref_sys(srs_id));
         CREATE TABLE IF NOT EXISTS gpkg_geometry_columns (
            table_name TEXT NOT NULL, column_name TEXT NOT NULL, geometry_type_name TEXT NOT NULL,
            srs_id INTEGER NOT NULL, z TINYINT NOT NULL, m TINYINT NOT NULL,
            CONSTRAINT pk_geom_cols PRIMARY KEY (table_name, column_name),
            CONSTRAINT fk_gc_tn FOREIGN KEY (table_name) REFERENCES gpkg_contents(table_name),
            CONSTRAINT fk_gc_srs FOREIGN KEY (srs_id) REFERENCES gpkg_spatial_ref_sys (srs_id));
         INSERT OR IGNORE INTO gpkg_spatial_ref_sys VALUES ('Undefined cartesian SRS', -1, 'NONE', -1, 'undefined', 'undefined cartesian coordinate reference system');
         INSERT OR IGNORE INTO gpkg_spatial_ref_sys VALUES ('Undefined geographic SRS', 0, 'NONE', 0, 'undefined', 'undefined geographic coordinate reference system');",
    )?;
    let wgs = Crs::wgs84().to_wkt().unwrap_or_default();
    conn.execute("INSERT OR IGNORE INTO gpkg_spatial_ref_sys VALUES ('WGS 84 geodetic', 4326, 'EPSG', 4326, ?1, 'longitude/latitude coordinates in decimal degrees on the WGS 84 spheroid')", [wgs])?;
    Ok(())
}

fn ensure_srs(conn: &Connection, crs: Option<&Crs>) -> Result<i32> {
    let Some(c) = crs else { return Ok(-1) };
    if let Some(code) = c.epsg {
        let id = code as i32;
        conn.execute(
            "INSERT OR IGNORE INTO gpkg_spatial_ref_sys (srs_name, srs_id, organization, organization_coordsys_id, definition) VALUES (?1, ?2, 'EPSG', ?2, ?3)",
            params![c.name, id, c.to_wkt().unwrap_or_else(|| "undefined".into())],
        )?;
        return Ok(id);
    }
    // sistem personalizat: id nou >= 100000
    let id: i32 = conn
        .query_row("SELECT MAX(srs_id) FROM gpkg_spatial_ref_sys", [], |r| {
            r.get::<_, Option<i32>>(0)
        })?
        .unwrap_or(0)
        .max(99_999)
        + 1;
    conn.execute(
        "INSERT INTO gpkg_spatial_ref_sys (srs_name, srs_id, organization, organization_coordsys_id, definition) VALUES (?1, ?2, 'NONE', ?2, ?3)",
        params![c.name, id, c.wkt.clone().unwrap_or_else(|| "undefined".into())],
    )?;
    Ok(id)
}

pub fn write(layers: &[&VectorLayer], path: &Path, opts: &WriteOptions) -> Result<()> {
    if path.exists() && !opts.append {
        std::fs::remove_file(path)
            .with_context(|| format!("Nu pot suprascrie {}", path.display()))?;
    }
    let mut conn =
        Connection::open(path).with_context(|| format!("Nu pot crea {}", path.display()))?;
    init_gpkg(&conn)?;
    let tx = conn.transaction()?;
    for (li, l) in layers.iter().enumerate() {
        let mut tname = if layers.len() == 1 {
            opts.layer_name.clone().unwrap_or_else(|| l.name.clone())
        } else {
            l.name.clone()
        };
        if tname.trim().is_empty() {
            tname = format!("strat_{}", li + 1);
        }
        let exists: bool = tx.query_row(
            "SELECT COUNT(*) FROM gpkg_contents WHERE table_name = ?1",
            [&tname],
            |r| r.get::<_, i64>(0),
        )? > 0;
        if exists {
            if !opts.overwrite {
                bail!("Stratul „{tname}” există deja în {}", path.display());
            }
            tx.execute(&format!("DROP TABLE IF EXISTS {}", qi(&tname)), [])?;
            tx.execute(
                "DELETE FROM gpkg_geometry_columns WHERE table_name = ?1",
                [&tname],
            )?;
            tx.execute("DELETE FROM gpkg_contents WHERE table_name = ?1", [&tname])?;
        }
        let srs = ensure_srs(&tx, l.crs.as_ref())?;
        // nume de coloane unice, fără conflict cu fid/geom
        let mut cols: Vec<String> = vec![];
        for f in &l.fields {
            let mut n = f.name.clone();
            if n.eq_ignore_ascii_case("fid") || n.eq_ignore_ascii_case("geom") {
                n = format!("{n}_1");
            }
            while cols.iter().any(|c| c.eq_ignore_ascii_case(&n)) {
                n.push('_');
            }
            cols.push(n);
        }
        let defs: Vec<String> = l
            .fields
            .iter()
            .zip(&cols)
            .map(|(f, n)| {
                let t = match f.ftype {
                    FieldType::Integer => "INTEGER",
                    FieldType::Real => "REAL",
                    FieldType::Boolean => "BOOLEAN",
                    FieldType::Date => "DATE",
                    FieldType::Text => "TEXT",
                };
                format!("{} {}", qi(n), t)
            })
            .collect();
        let mut sql = format!(
            "CREATE TABLE {} (fid INTEGER PRIMARY KEY AUTOINCREMENT NOT NULL, geom BLOB",
            qi(&tname)
        );
        for d in &defs {
            sql.push_str(", ");
            sql.push_str(d);
        }
        sql.push(')');
        tx.execute(&sql, [])?;
        let b = l.bounds();
        let (bx0, by0, bx1, by1) = if b.is_valid() {
            (Some(b.minx), Some(b.miny), Some(b.maxx), Some(b.maxy))
        } else {
            (None, None, None, None)
        };
        tx.execute(
            "INSERT INTO gpkg_contents (table_name, data_type, identifier, description, min_x, min_y, max_x, max_y, srs_id) VALUES (?1, 'features', ?1, '', ?2, ?3, ?4, ?5, ?6)",
            params![tname, bx0, by0, bx1, by1, srs],
        )?;
        tx.execute(
            "INSERT INTO gpkg_geometry_columns VALUES (?1, 'geom', ?2, ?3, ?4, 0)",
            params![
                tname,
                geometry_type_name(l),
                srs,
                if l.has_z && l.features.iter().any(|f| f.geom.is_some()) { 1 } else { 0 }
            ],
        )?;
        let ph: Vec<String> = (0..=l.fields.len())
            .map(|i| format!("?{}", i + 1))
            .collect();
        let col_list: Vec<String> = std::iter::once("geom".to_string())
            .chain(cols.iter().map(|c| qi(c)))
            .collect();
        let mut ins = tx.prepare(&format!(
            "INSERT INTO {} ({}) VALUES ({})",
            qi(&tname),
            col_list.join(", "),
            ph.join(", ")
        ))?;
        for f in &l.features {
            let mut vals: Vec<rusqlite::types::Value> = Vec::with_capacity(l.fields.len() + 1);
            vals.push(match &f.geom {
                Some(g) => rusqlite::types::Value::Blob(encode_gpkg_geom(g, srs, l.has_z)),
                None => rusqlite::types::Value::Null,
            });
            for (v, fd) in f.attrs.iter().zip(&l.fields) {
                vals.push(match v.coerce(fd.ftype) {
                    Value::Null => rusqlite::types::Value::Null,
                    Value::Int(i) => rusqlite::types::Value::Integer(i),
                    Value::Real(r) => rusqlite::types::Value::Real(r),
                    Value::Bool(b) => rusqlite::types::Value::Integer(b as i64),
                    Value::Text(s) => rusqlite::types::Value::Text(s),
                });
            }
            ins.execute(rusqlite::params_from_iter(vals))
                .map_err(|e| anyhow!("Eroare la scrierea în GeoPackage: {e}"))?;
        }
    }
    tx.commit()?;
    Ok(())
}
