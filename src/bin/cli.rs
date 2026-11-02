//! cadastru-cli – conversii de formate, reproiecții și informații din linia de comandă.

use anyhow::{Result, bail};
use cadastru::convert::{self, ConvertOptions};
use cadastru::crs::{self, Crs, Transformer};
use cadastru::formats::{self, Kind, ReadOptions, WriteOptions};
use cadastru::model::*;
use cadastru::rasterops::Resampling;
use clap::{Parser, Subcommand, ValueEnum};
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

#[derive(Parser)]
#[command(name = "cadastru-cli", version, about = "Cadastru – conversie formate GIS, reproiecție și informații", long_about = None)]
struct Cli {
    /// Nu folosi TransDatRO pentru Stereo 70 (se folosește Helmert 7 parametri, EPSG:15994)
    #[arg(long, global = true)]
    fara_transdatro: bool,
    /// Altitudini: doar cvasigeoidul național EGG97 (fără cel pentru București-Ilfov)
    #[arg(long, global = true)]
    doar_egg97: bool,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Clone, Copy, ValueEnum)]
enum Rs {
    Nearest,
    Bilinear,
}

#[derive(clap::Args, Clone)]
struct InOpts {
    /// Sistemul de coordonate al datelor de intrare (suprascrie .prj), ex. EPSG:3844
    #[arg(long = "s-srs")]
    s_srs: Option<String>,
    /// Codificarea textului la citire (UTF-8, cp1250 ...)
    #[arg(long)]
    encoding: Option<String>,
    /// CSV: coloana X (est / longitudine)
    #[arg(long)]
    x_field: Option<String>,
    /// CSV: coloana Y (nord / latitudine)
    #[arg(long)]
    y_field: Option<String>,
    /// CSV: coloana Z (cotă)
    #[arg(long)]
    z_field: Option<String>,
    /// DXF: păstrează poliliniile închise ca linii (nu poligoane)
    #[arg(long)]
    dxf_lines: bool,
    /// DXF: păstrează punctele cu „RTCM” în descriere (implicit sunt eliminate)
    #[arg(long)]
    dxf_pastreaza_rtcm: bool,
    /// CGXML: nu genera stratul de construcții
    #[arg(long)]
    cgxml_fara_constructii: bool,
    /// CGXML: inversează X și Y (X = Nord, Y = Est în fișier)
    #[arg(long)]
    cgxml_inversare_xy: bool,
    /// CGXML: include CNP/CUI în PROPRIETAR (date cu caracter personal)
    #[arg(long)]
    cgxml_cnp: bool,
    /// CGXML: doar atributele de bază (ca opțiunea „DBF simplu”)
    #[arg(long)]
    cgxml_dbf_simplu: bool,
    /// CGXML: nu închide automat inelele poligoanelor
    #[arg(long)]
    cgxml_fara_inchidere: bool,
    /// Păstrează doar entitățile aflate în întregime în interiorul României
    /// (încadrarea în EPSG:3844: 114000, 235000 – 885000, 764000)
    #[arg(long)]
    doar_romania: bool,
    /// Ca --doar-romania, cu alt dreptunghi în EPSG:3844: "minX,minY,maxX,maxY"
    #[arg(long, value_name = "minX,minY,maxX,maxY")]
    zona_3844: Option<String>,
    /// Cu --doar-romania / --zona-3844: păstrează și entitățile care doar ating zona
    #[arg(long)]
    partial: bool,
    /// Păstrează doar entitățile care îndeplinesc expresia, ex. "status = 1 AND is_active = 1"
    #[arg(long, value_name = "EXPRESIE")]
    r#where: Option<String>,
}

#[derive(clap::Args, Clone)]
struct OutOpts {
    /// Sistemul de coordonate de ieșire (reproiecție), ex. EPSG:4326
    #[arg(long = "t-srs")]
    t_srs: Option<String>,
    /// Suprascrie fișierele existente
    #[arg(short = 'o', long)]
    overwrite: bool,
    /// Codificarea la scriere pentru DBF/DXF (implicit UTF-8 pentru DBF, cp1250 pentru DXF)
    #[arg(long = "out-encoding")]
    out_encoding: Option<String>,
    /// Raster: rezoluția de ieșire (unități ale sistemului destinație)
    #[arg(long)]
    res: Option<f64>,
    /// Raster: metoda de reeșantionare
    #[arg(long, value_enum, default_value = "nearest")]
    resampling: Rs,
    /// CSV: scrie geometria ca WKT chiar și pentru puncte
    #[arg(long)]
    wkt: bool,
    /// Câmpul folosit ca nume/etichetă (KML, GPX, DXF)
    #[arg(long)]
    name_field: Option<String>,
    /// GeoPackage: adaugă straturile la un fișier existent
    #[arg(long)]
    append: bool,
}

#[derive(Subcommand)]
enum Cmd {
    /// Afișează informații despre un fișier (straturi, câmpuri, sistem, extindere)
    Info {
        file: PathBuf,
        #[command(flatten)]
        input: InOpts,
        /// Afișează și primele N entități
        #[arg(long, default_value_t = 0)]
        features: usize,
    },
    /// Convertește un fișier în alt format și/sau alt sistem de coordonate
    Convert {
        input: PathBuf,
        output: PathBuf,
        /// Doar stratul cu acest nume (GeoPackage, GPX)
        #[arg(long)]
        layer: Option<String>,
        #[command(flatten)]
        inp: InOpts,
        #[command(flatten)]
        out: OutOpts,
    },
    /// Convertește toate fișierele dintr-un dosar
    Batch {
        input_dir: PathBuf,
        output_dir: PathBuf,
        /// Extensia formatului de ieșire (shp, gpkg, geojson, kml, dxf, csv, tif ...)
        #[arg(long)]
        to: String,
        /// Doar fișierele cu aceste extensii (ex. shp,dxf). Implicit: toate formatele suportate
        #[arg(long)]
        only: Option<String>,
        /// Caută și în subdosare
        #[arg(short, long)]
        recursive: bool,
        #[command(flatten)]
        inp: InOpts,
        #[command(flatten)]
        out: OutOpts,
    },
    /// Transformă coordonate între sisteme (din argumente sau de la intrarea standard)
    Transform {
        #[arg(long = "s-srs")]
        s_srs: String,
        #[arg(long = "t-srs")]
        t_srs: String,
        /// Perechi de coordonate: X Y [X Y ...]. Fără argumente se citesc linii „X Y” de la stdin
        coords: Vec<f64>,
        /// Număr de zecimale
        #[arg(long)]
        precision: Option<usize>,
        /// Afișează gradele și în format grade-minute-secunde
        #[arg(long)]
        dms: bool,
    },
    /// Transformare TransDatRO (ANCPI): ETRS89 <-> Stereo 70 / Stereo 30 și altitudini Marea Neagră 75.
    ///
    /// Intrarea e în formatul programului TransDatRO: un rând de titlu (opțional), apoi
    /// „cod, coord1, coord2[, h]” pe fiecare rând, încheiat opțional cu ENDF.
    /// ETRS89: latitudine, longitudine (44 38 43.52664 sau 44°38'43.52664"N sau grade zecimale);
    /// Stereo 70: x (Nord), y (Est); Stereo 30: x (Est), y (Nord).
    #[command(name = "transdatro")]
    TransDatRO {
        /// Direcția: etrs89-st70, st70-etrs89, etrs89-st30, st30-etrs89, st30-st70, st70-st30
        #[arg(short, long)]
        directie: String,
        /// Fișierul de intrare (implicit: intrarea standard)
        input: Option<PathBuf>,
        /// Fișierul de ieșire (implicit: ieșirea standard)
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
    /// Afișează definiția unui sistem de coordonate sau caută după nume
    Crs {
        /// EPSG:cod, cod, nume, șir PROJ sau fișier .prj
        query: String,
        /// Caută după nume în loc să afișeze definiția
        #[arg(short, long)]
        search: bool,
    },
    /// Unește fișiere vectoriale (de exemplu DXF) și le scrie în câte un shapefile pe tip de
    /// geometrie: <IESIRE>_puncte.shp, <IESIRE>_linii.shp, <IESIRE>_poligoane.shp.
    ///
    /// Intrările pot fi fișiere sau dosare (din dosare se iau toate fișierele .dxf).
    /// Exemplu: cadastru-cli split rezultat\plan.shp desen1.dxf desen2.dxf --t-srs EPSG:3844
    #[command(name = "split")]
    Split {
        /// Fișierul de bază al ieșirii (.shp); tipul se adaugă la nume
        output: PathBuf,
        /// Fișierele sau dosarele de intrare
        #[arg(required = true)]
        inputs: Vec<PathBuf>,
        /// Din dosare: și din subdosare
        #[arg(short, long)]
        recursive: bool,
        /// Nu scrie stratul de puncte
        #[arg(long)]
        fara_puncte: bool,
        /// Nu scrie stratul de linii
        #[arg(long)]
        fara_linii: bool,
        /// Nu scrie stratul de poligoane
        #[arg(long)]
        fara_poligoane: bool,
        /// Poliliniile închise, cercurile și elipsele merg la linii (hașurile rămân poligoane)
        #[arg(long)]
        inchise_ca_linii: bool,
        /// Nu adăuga câmpul Fisier cu fișierul de origine
        #[arg(long)]
        fara_camp_fisier: bool,
        #[command(flatten)]
        inp: InOpts,
        #[command(flatten)]
        out: OutOpts,
    },
    /// Lista formatelor suportate
    Formats,
}

fn read_opts(i: &InOpts) -> Result<ReadOptions> {
    let area = match (&i.zona_3844, i.doar_romania) {
        (Some(z), _) => Some(cadastru::area::AreaFilter {
            bounds: cadastru::area::parse_bounds(z)
                .ok_or_else(|| anyhow::anyhow!("--zona-3844: se așteaptă minX,minY,maxX,maxY, ex. 114000,235000,885000,764000"))?,
            partial: i.partial,
        }),
        (None, true) => Some(cadastru::area::AreaFilter {
            partial: i.partial,
            ..cadastru::area::AreaFilter::romania()
        }),
        (None, false) => None,
    };
    Ok(ReadOptions {
        area,
        where_expr: i.r#where.clone().filter(|w| !w.trim().is_empty()),
        encoding: i.encoding.clone(),
        x_field: i.x_field.clone(),
        y_field: i.y_field.clone(),
        z_field: i.z_field.clone(),
        crs: i.s_srs.as_deref().map(Crs::parse).transpose()?,
        dxf_closed_as_lines: i.dxf_lines,
        dxf_keep_rtcm: i.dxf_pastreaza_rtcm,
        cgxml_custom: true,
        cgxml: cadastru::formats::cgxml::CgxmlOptions {
            close_ring: !i.cgxml_fara_inchidere,
            rich_dbf: !i.cgxml_dbf_simplu,
            buildings: !i.cgxml_fara_constructii,
            swap_xy: i.cgxml_inversare_xy,
            id_code: i.cgxml_cnp,
        },
    })
}

fn write_opts(o: &OutOpts) -> WriteOptions {
    WriteOptions {
        encoding: o.out_encoding.clone(),
        overwrite: o.overwrite,
        layer_name: None,
        append: o.append,
        csv_wkt: o.wkt,
        name_field: o.name_field.clone(),
    }
}

fn print_info(path: &Path, inp: &InOpts, nfeat: usize) -> Result<()> {
    let (ds, info) = formats::open_ex(path, &read_opts(inp)?)?;
    println!("Fișier: {}", path.display());
    for m in &info {
        println!("{m}");
    }
    if let Some(f) = formats::format_for(path) {
        println!("Format: {}", f.name);
    }
    for d in &ds {
        println!();
        match d {
            Dataset::Vector(v) => {
                println!("Strat vectorial: {}", v.name);
                println!("  Entități: {}", v.features.len());
                println!(
                    "  Geometrie: {}{}",
                    v.geom_kind().label(),
                    if v.has_z { " (3D)" } else { "" }
                );
                println!(
                    "  Sistem: {}",
                    v.crs
                        .as_ref()
                        .map(|c| c.to_string())
                        .unwrap_or_else(|| "nedefinit".into())
                );
                println!("  Extindere: {}", v.bounds());
                println!("  Câmpuri ({}):", v.fields.len());
                for f in &v.fields {
                    println!("    {} : {}", f.name, f.ftype.label());
                }
                for (i, f) in v.features.iter().take(nfeat).enumerate() {
                    println!(
                        "  #{}: {}",
                        i + 1,
                        f.geom
                            .as_ref()
                            .map(|g| {
                                let w = cadastru::geomio::to_wkt(g, v.has_z);
                                if w.len() > 120 {
                                    format!(
                                        "{}…",
                                        &w[..w
                                            .char_indices()
                                            .nth(117)
                                            .map(|c| c.0)
                                            .unwrap_or(w.len())]
                                    )
                                } else {
                                    w
                                }
                            })
                            .unwrap_or_else(|| "(fără geometrie)".into())
                    );
                    for (fd, val) in v.fields.iter().zip(&f.attrs) {
                        println!("      {} = {}", fd.name, val);
                    }
                }
            }
            Dataset::Raster(r) => {
                println!("Raster: {}", r.name);
                let (w, h) = r.full.as_ref().map_or((r.width, r.height), |f| f.0.size());
                println!(
                    "  Dimensiuni: {} x {} pixeli, {} benzi ({})",
                    w,
                    h,
                    r.bands.len(),
                    r.bands[0].type_name()
                );
                if let Some(f) = &r.full {
                    println!(
                        "  Citit prin GDAL ({}), doar vizualizare; statisticile sunt calculate pe imaginea de ansamblu {} x {} px",
                        f.0.driver(),
                        r.width,
                        r.height
                    );
                }
                let (px, py) = match &r.full {
                    Some(f) => {
                        let g = f.0.geo_transform();
                        ((g[1].powi(2) + g[4].powi(2)).sqrt(), (g[2].powi(2) + g[5].powi(2)).sqrt())
                    }
                    None => r.pixel_size(),
                };
                println!("  Mărime pixel: {} x {}", fmt_real(px), fmt_real(py));
                println!(
                    "  Sistem: {}",
                    r.crs
                        .as_ref()
                        .map(|c| c.to_string())
                        .unwrap_or_else(|| "nedefinit".into())
                );
                println!("  Extindere: {}", r.bounds());
                println!(
                    "  NoData: {}",
                    r.nodata.map(fmt_real).unwrap_or_else(|| "-".into())
                );
                for b in 0..r.bands.len() {
                    let s = r.stats(b);
                    println!(
                        "  Banda {}: min={} max={} medie={:.3} abatere={:.3}",
                        b + 1,
                        fmt_real(s.min),
                        fmt_real(s.max),
                        s.mean,
                        s.stddev
                    );
                }
            }
        }
    }
    Ok(())
}

fn conv_opts(inp: &InOpts, out: &OutOpts, layer: Option<String>) -> Result<ConvertOptions> {
    Ok(ConvertOptions {
        read: read_opts(inp)?,
        write: write_opts(out),
        target: out.t_srs.as_deref().map(Crs::parse).transpose()?,
        layer,
        res: out.res,
        resampling: match out.resampling {
            Rs::Nearest => Resampling::Nearest,
            Rs::Bilinear => Resampling::Bilinear,
        },
    })
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    crs::set_transdatro(!cli.fara_transdatro);
    if cli.doar_egg97 {
        crs::set_height_model(cadastru::transdatro::HeightModel::Egg97);
    }
    match cli.cmd {
        Cmd::TransDatRO {
            directie,
            input,
            output,
        } => {
            use cadastru::transdatro::{self as tdr, Direction};
            let Some(dir) = Direction::from_key(&directie) else {
                bail!(
                    "Direcție necunoscută „{directie}”. Variante: {}",
                    Direction::ALL
                        .iter()
                        .map(|d| d.key())
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            };
            let text = match &input {
                Some(p) => {
                    let b = std::fs::read(p)?;
                    match String::from_utf8(b.clone()) {
                        Ok(s) => s,
                        Err(_) => encoding_rs::WINDOWS_1250.decode(&b).0.into_owned(),
                    }
                }
                None => {
                    let mut s = String::new();
                    std::io::Read::read_to_string(&mut std::io::stdin(), &mut s)?;
                    s
                }
            };
            let r = tdr::process_text(&text, dir, crs::height_model());
            match &output {
                Some(p) => {
                    std::fs::write(p, r.output.as_bytes())?;
                    eprintln!(
                        "{}: {} puncte transformate, {} erori -> {}",
                        dir.label(),
                        r.ok,
                        r.errors,
                        p.display()
                    );
                }
                None => print!("{}", r.output),
            }
            if r.errors > 0 {
                std::process::exit(2);
            }
        }
        Cmd::Info {
            file,
            input,
            features,
        } => print_info(&file, &input, features)?,
        Cmd::Split {
            output,
            inputs,
            recursive,
            fara_puncte,
            fara_linii,
            fara_poligoane,
            inchise_ca_linii,
            fara_camp_fisier,
            inp,
            out,
        } => {
            let ro = read_opts(&inp)?;
            let mut files = vec![];
            for i in &inputs {
                if i.is_dir() {
                    files.extend(convert::collect_files(i, recursive, &["dxf".to_string()])?);
                } else {
                    files.push(i.clone());
                }
            }
            if files.is_empty() {
                bail!("Nu am găsit fișiere de intrare");
            }
            let mut layers = vec![];
            for f in &files {
                for d in cadastru::formats::open(f, &ro)? {
                    if let Dataset::Vector(v) = d {
                        layers.push(v);
                    }
                }
            }
            println!("Fișiere citite: {}, straturi: {}", files.len(), layers.len());
            let so = convert::SplitOptions {
                points: !fara_puncte,
                lines: !fara_linii,
                polygons: !fara_poligoane,
                closed_as_lines: inchise_ca_linii,
                source_field: (!fara_camp_fisier).then(|| "Fisier".to_string()),
            };
            let target = out.t_srs.as_deref().map(Crs::parse).transpose()?;
            let mut wo = write_opts(&out);
            if !out.overwrite {
                for k in ["puncte", "linii", "poligoane"] {
                    let stem = output.file_stem().unwrap_or_default().to_string_lossy();
                    let p = output.with_file_name(format!("{stem}_{k}.shp"));
                    if p.exists() {
                        bail!("Fișierul există deja: {} (folosiți -o pentru suprascriere)", p.display());
                    }
                }
            }
            wo.overwrite = true;
            let r = convert::export_by_kind(&layers, target.as_ref(), &output, &so, &wo)?;
            for w in r.warnings {
                eprintln!("Atenție: {w}");
            }
            for (f, k, n) in r.written {
                println!("Scris: {} ({n} entități, {})", f.display(), k.label());
            }
        }
        Cmd::Convert {
            input,
            output,
            layer,
            inp,
            out,
        } => {
            let r = convert::convert_file(&input, &output, &conv_opts(&inp, &out, layer)?)?;
            for w in r.warnings {
                eprintln!("Atenție: {w}");
            }
            for f in r.written {
                println!("Scris: {}", f.display());
            }
        }
        Cmd::Batch {
            input_dir,
            output_dir,
            to,
            only,
            recursive,
            inp,
            out,
        } => {
            let to = to.trim_start_matches('.').to_lowercase();
            let exts: Vec<String> = match only {
                Some(o) => o
                    .split(',')
                    .map(|s| s.trim().trim_start_matches('.').to_lowercase())
                    .collect(),
                None => convert::default_input_exts(&to),
            };
            let co = conv_opts(&inp, &out, None)?;
            let (ok, err) = convert::batch(
                &input_dir,
                &output_dir,
                &to,
                &exts,
                recursive,
                &co,
                &mut |l| println!("{l}"),
            )?;
            println!("Gata: {ok} convertite, {err} erori");
            if err > 0 {
                std::process::exit(2);
            }
        }
        Cmd::Transform {
            s_srs,
            t_srs,
            coords,
            precision,
            dms,
        } => {
            let s = Crs::parse(&s_srs)?;
            let t = Crs::parse(&t_srs)?;
            let tr = Transformer::new(&s, &t)?;
            if tr.uses_transdatro() {
                eprintln!("(transformare TransDatRO)");
            }
            let prec = precision.unwrap_or(if t.is_geographic() { 9 } else { 4 });
            let show = |x: f64, y: f64, out: &mut dyn Write| -> Result<()> {
                match tr.xy_method(x, y) {
                    Some(((a, b), m)) => {
                        write!(out, "{:.*} {:.*}", prec, a, prec, b)?;
                        if m == crs::Method::Fallback {
                            write!(out, "   [în afara gridului TransDatRO: Helmert 7 parametri]")?;
                        }
                        if dms && t.is_geographic() {
                            write!(
                                out,
                                "   ({} {})",
                                crs::to_dms(b, true),
                                crs::to_dms(a, false)
                            )?;
                        }
                        writeln!(out)?;
                    }
                    None => writeln!(out, "EROARE: punctul {x} {y} nu poate fi transformat")?,
                }
                Ok(())
            };
            let stdout = std::io::stdout();
            let mut so = stdout.lock();
            if coords.is_empty() {
                for line in std::io::stdin().lock().lines() {
                    let line = line?;
                    let v: Vec<f64> = line
                        .split(|c: char| c.is_whitespace() || c == ',' || c == ';')
                        .filter(|t| !t.is_empty())
                        .filter_map(|t| t.parse().ok())
                        .collect();
                    if v.len() >= 2 {
                        show(v[0], v[1], &mut so)?;
                    } else if !line.trim().is_empty() {
                        writeln!(so, "EROARE: linie invalidă „{line}”")?;
                    }
                }
            } else {
                if coords.len() % 2 != 0 {
                    bail!("Coordonatele trebuie date în perechi X Y");
                }
                for c in coords.chunks(2) {
                    show(c[0], c[1], &mut so)?;
                }
            }
        }
        Cmd::Crs { query, search } => {
            if search {
                for (c, n) in crs::search(&query, 200) {
                    println!("EPSG:{c}\t{n}");
                }
            } else {
                let q = if Path::new(&query).exists() {
                    std::fs::read_to_string(&query)?
                } else {
                    query.clone()
                };
                let c = Crs::parse(&q)?;
                println!("Nume: {}", c.name);
                if let Some(e) = c.epsg {
                    println!("Cod: EPSG:{e}");
                }
                println!(
                    "Tip: {}",
                    if c.is_geographic() {
                        "geografic (grade)"
                    } else {
                        "proiectat"
                    }
                );
                println!("PROJ: {}", c.proj);
                if let Some(w) = c.to_wkt() {
                    println!("WKT: {w}");
                }
            }
        }
        Cmd::Formats => {
            println!(
                "{:<28} {:<16} {:<8} {}",
                "Format", "Extensii", "Tip", "Operații"
            );
            for f in formats::FORMATS {
                println!(
                    "{:<28} {:<16} {:<8} {}",
                    f.name,
                    f.exts.join(","),
                    if f.kind == Kind::Vector {
                        "vector"
                    } else {
                        "raster"
                    },
                    match (f.read, f.write) {
                        (true, true) => "citire/scriere",
                        (true, false) => "citire",
                        _ => "scriere",
                    }
                );
            }
        }
    }
    Ok(())
}

fn main() {
    if let Err(e) = run() {
        eprintln!("Eroare: {e:#}");
        std::process::exit(1);
    }
}
