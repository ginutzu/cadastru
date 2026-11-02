# Cadastru 1.12.0

Autor: **Ginu Popescu** (scriemiaici@proton.me)

Aplicație GIS desktop pentru Windows, scrisă în Rust. Rulează fără instalare: două fișiere `.exe`, fără DLL-uri suplimentare.

**Descărcare:** arhiva cu executabilele, manualul și datele de exemplu este la [Releases](https://github.com/ginutzu/cadastru/releases/latest).

- **`cadastru.exe`**: interfața grafică (hartă, straturi, identificare, măsurare, export, convertor de coordonate)
- **`cadastru-cli.exe`**: linie de comandă pentru conversii, reproiecții și procesări în lot (scripturi `.bat`, PowerShell, planificator)

Cerințe: Windows 10/11 pe 64 de biți și o placă video cu OpenGL 2.0 sau mai nou (orice placă Intel, AMD sau NVIDIA din ultimii 10 ani). Pentru hărțile de fundal e nevoie de acces la internet.

La pornire, harta este în **Stereo 70 (EPSG:3844)**, centrată pe România, cu harta de fundal **OpenStreetMap**. Datele deschise se reproiectează din mers în Stereo 70; sistemul hărții se poate schimba oricând din butonul cu globul din bara de instrumente.

Documentație (în dosarul [`docs`](docs)): **`Manual_Cadastru.pdf`** (manualul de utilizare, cu capturi de ecran; se deschide și din aplicație, *Ajutor → Manual de utilizare* sau tasta F1) și **`Documentatie_tehnica_Cadastru.pdf`** (arhitectură, algoritmi, formate, linia de comandă, construire, validare).

Aplicația s-a numit anterior *Cartograf*; setările versiunii anterioare sunt preluate automat la prima pornire.

---

## 1. Funcționalități

### Interfață
- **bilingvă: română (implicit) și engleză**. Comutatorul **RO | EN** din dreapta barei de instrumente (sau *Vizualizare → Limbă / Language*) schimbă limba imediat, fără repornire; alegerea se păstrează în setări. Se traduc meniurile, dialogurile, mesajele, erorile și textele planșei tipărite; datele (nume de straturi, câmpuri, valori) rămân neschimbate. Manualul și linia de comandă sunt în română.
- fereastra se deschide **maximizată** (pe tot ecranul, cu bara de titlu și bara de activități Windows vizibile)
- temă **gri închis** (implicită) sau luminoasă, din *Vizualizare → Temă*; alegerea se păstrează în `%APPDATA%\Cadastru\setari.json`
- bara de instrumente e **grupată pe funcționalități**, cu numele grupului sub butoane: *Fișier* (Deschide), *Vizualizare* (Deplasare, Zoom +, Zoom −, Extindere completă, Vederea anterioară / următoare, Redraw), *Interogare* (Identificare, Căutare, Tabel), *Selecție* (Selectare, Selectare după atribute), *Instrumente* (Măsurare, Convertor) și *Hartă* (Basemap, sistemul hărții); butoanele care dublau alte comenzi (mărire / micșorare în centrul hărții) au fost scoase – rămân tastele + / − și butoanele de pe hartă. Pe ecrane înguste butoanele rămân doar cu pictograme, cu numele în indiciu
- **panoul Straturi** se poate îngusta trăgând marginea spre stânga (numele lungi se taie și apar întregi în indiciu, la trecerea cu mouse-ul) sau se poate **ascunde complet** cu « din colțul lui, cu *Vizualizare → Panoul Straturi* sau Ctrl+L (ori trăgând marginea până la capăt): harta ocupă toată lățimea și rămâne doar o bandă îngustă, pe care un click readuce panoul; starea se păstrează între porniri
- **control de navigație pe hartă**: săgeți de deplasare (click = un sfert de ecran, ținut apăsat = deplasare continuă), centru = extindere completă, butoane + / − pentru zoom
- instrumentele **Zoom +** (click = ×2, dreptunghi = zoom pe zonă) și **Zoom −** (click = ×0,5, dreptunghi = vederea curentă intră în zonă), **Deplasare**, **Identificare**, **Măsurare**
- istoric de vederi: **vederea anterioară / următoare**

### Hărți de fundal (butonul „Basemap”)
- tile-uri gratuite de la furnizori publici, grupate pe submeniuri:
  - **OpenStreetMap**: Standard, Humanitarian, France, DE, CyclOSM, OpenTopoMap (topografică)
  - **Esri**: World Imagery (satelit), Imagery Hybrid (satelit + etichete), Street Map, Topo Map, National Geographic, Terrain, Shaded Relief, Physical Map, Light / Dark Gray Canvas
  - **Google**: Hărți, Satelit, Hibrid, Teren
  - **Bing**: Aerial, Hibrid
  - **CARTO**: Positron, Dark Matter, Voyager
  - **Altele**: TopPlusOpen (BKG, topografică), Sentinel-2 cloudless (EOX)
- **Fără basemap**, opacitate reglabilă și **șablon XYZ personalizat** pentru orice alt serviciu de tile-uri (`https://server/{z}/{x}/{y}.png`, cu `{s}`, `{-y}` pentru TMS și `{q}` pentru quadkey)
- tile-urile (Web Mercator) sunt **reproiectate exact în sistemul hărții**: în Stereo 70 se suprapun corect peste date, inclusiv prin TransDatRO. Eroarea de desenare e sub 0,1 pixeli, la toate nivelurile de zoom.
- descărcare în fundal pe mai multe fire, de la centrul ecranului spre margini; până sosesc tile-urile noi se afișează o versiune mai grosieră
- cache pe disc în `%LOCALAPPDATA%\Cadastru\tile-cache` (tile-urile se reîmprospătează după 30 de zile; cache-ul se curăță automat peste ~1 GB). Fără internet se folosesc tile-urile din cache. Cache-ul se poate goli din meniul Basemap.
- funcționează prin proxy: variabilele `HTTPS_PROXY` / `HTTP_PROXY` sau proxy-ul din setările Windows. Scripturile PAC (configurare automată) nu sunt suportate. Certificatele se verifică prin magazinul de certificate Windows, deci merge și în rețelele cu inspecție TLS.
- atribuirea furnizorului apare în colțul din dreapta-jos al hărții; dacă tile-urile nu se pot descărca, acolo apare și motivul
- harta de fundal aleasă se salvează în spațiul de lucru (.cgw)
- **termeni de utilizare**: fiecare furnizor are condițiile lui. OpenStreetMap, Esri, CARTO și EOX permit utilizarea moderată, cu atribuire. Google și Bing nu permit oficial accesul direct la tile-uri în afara API-urilor lor; aceste opțiuni sunt incluse la cerere, iar folosirea lor rămâne pe răspunderea utilizatorului.

### Tipărire și Salvare ca PDF (meniul *Fișier*)
- **Tipărire… (Ctrl+P)** și **Salvează ca PDF…** compun o planșă din vederea curentă: titlu, chenar, harta, legendă, scară numerică și grafică, săgeata nordului, caroiaj de coordonate cu valori pe margini, sistemul de coordonate, data, o notă opțională (de ex. „Întocmit: …”) și atribuirea hărții de fundal
- hârtie A4, A3, A2, A1 sau A0, orientare peisaj sau portret
- scara: cea a vederii curente sau una fixă (1:500, 1:1000, 1:2000, 1:5000 … ori orice valoare 1:N); harta e centrată pe vederea curentă
- rezoluție 150, 200 sau 300 DPI; harta de fundal se descarcă din nou la rezoluția paginii
- elementele planșei se pot activa sau dezactiva individual (legendă, scară grafică, nord, caroiaj, harta de fundal)
- **Previzualizare** înainte de tipărire sau salvare
- la tipărire se deschide dialogul standard Windows de alegere a imprimantei. Mărimea hârtiei, orientarea și rezoluția sunt cele alese acolo, iar planșa se adaptează zonei tipăribile a imprimantei. Funcționează și cu „Microsoft Print to PDF”.
- PDF-ul are mărimea exactă a hârtiei. Planșa e o imagine de înaltă calitate, deci se deschide oriunde și se tipărește identic, dar textul din ea nu poate fi selectat.

### Vizualizare hartă
- straturi vectoriale și raster suprapuse, cu ordine, vizibilitate și opacitate reglabile
- **reproiecție din mers**: fiecare strat se afișează în sistemul hărții, indiferent de sistemul fișierului
- navigare: rotița mouse-ului pentru zoom la cursor; tragere sau butonul din mijloc pentru deplasare; zoom pe fereastră; extindere completă
- stiluri pentru vectori: culoare de contur și de umplere, **fără umplere (No fill)** pentru poligoane (doar conturul; și din meniul stratului), grosime, mărimea punctelor, etichete din orice câmp (cu evitarea suprapunerilor)
- **halo pentru etichete, pe fiecare strat**: contur care urmează literele, dreptunghi sub text sau fără halo; culoare automată (în contrast cu textul) sau aleasă, **transparență** 0–100 % și grosime; la fel pe hartă și la tipărire / PDF
- **Redraw (F5)**, în bara de instrumente și în *Instrumente*: ca REDRAWALL / REGEN din programele CAD – șterge grafica temporară (măsurătoarea, marcajul căutării, evidențierea de la identificare), regenerează afișarea tuturor straturilor, reîncearcă tile-urile de fundal eșuate și redesenează harta; selecțiile și vederea rămân
- rastere: compunere RGB din benzi la alegere, întindere min/max automată sau manuală, rampe de culori (Relief, Gri, Spectral, Viridis), umbrire de relief (hillshade), alb transparent pentru hărți scanate, NoData transparent
- randare adaptivă: rasterele mari se redesenează la rezoluția ecranului după fiecare zoom
- **ECW, MrSID și JPEG 2000 (doar citire)** prin GDAL-ul unei instalări QGIS / OSGeo4W de pe calculator (vezi *Formate*)
- scară grafică și scară numerică calculate geodezic; coordonatele cursorului în sistemul hărții și în WGS84 (zecimal și grade-minute-secunde)

### Instrumente
- **Identificare (I)**: atributele entității și valorile pixelilor din toate rasterele aflate sub cursor; pentru poligoane și linii afișează și aria, perimetrul și lungimea
- **Selectare (S)**: click = o entitate, Ctrl+click = adaugă / scoate, dreptunghi = toate entitățile atinse (Shift adaugă, Alt scoate); selecția apare cu galben pe hartă și ca număr lângă numele stratului, e sincronizată cu tabelul de atribute (Ctrl/Shift+click, bifa „Doar selecția”) și are operații în meniul stratului: selectează tot, inversează, deselectează, zoom la selecție, **exportă selecția** (în orice format vectorial, cu reproiecție). Ctrl+Shift+A golește selecția
- **Selectare după atribute (Ctrl+Shift+F)**: expresie în stilul SQL `WHERE` pe un strat – `admin_unit_id = 123`, `status = 1 AND is_active = 1`, `judet LIKE 'Arg%'`, `cod IN (13, 14)`, `supr BETWEEN 100 AND 500`, `obs IS NULL`, paranteze, `NOT`; lista câmpurilor și a valorilor distincte (cu numărul de apariții) se inserează cu click, expresia e verificată pe loc (eroarea cu poziția ei); selecție nouă, adaugă, scoate sau doar din selecția curentă; textele fără deosebire de majuscule și diacritice (sau exact); după selectare harta se **centrează și se încadrează pe selecție** (bifa „Zoom pe rezultat”, activă implicit, și butonul „Zoom pe selecție”)
- **Filtrul de încărcare** (*Fișier → La deschidere: doar entitățile din interiorul României*): la deschiderea fișierelor vectoriale se încarcă doar entitățile aflate în interiorul încadrării României în Stereo 70 (EPSG:3844: 114000, 235000 – 885000, 764000) – în întregime sau, opțional, și cele care doar ating zona; zona poate fi înlocuită cu alt dreptunghi; se aplică și la deschiderea în masă, la fișierele trase în fereastră și la importul CSV, iar pentru un strat deja încărcat există comanda „Scoate entitățile din afara României”
- **Căutare (Ctrl+F)**: locuri, adrese și obiective prin **OpenStreetMap Nominatim**, **Google** sau ambele (implicit doar în România), cu zoom și marcaj pe hartă. Căutarea Google folosește Places API (New) / Geocoding API cu o cheie Google Maps Platform a utilizatorului (introdusă în fereastra de căutare sau în variabila de mediu `GOOGLE_MAPS_API_KEY`); fără cheie, butonul **Deschide în Google Maps** trimite căutarea (sau vederea curentă) în browser, iar **linkul unui loc din Google Maps** – complet sau scurt (`maps.app.goo.gl/…`) – lipit în căutare duce harta exact acolo. Se recunosc și **codurile Plus** (`8GP8PV9C+2W` sau `PV9C+2W Pitești`), coordonatele „lat, lon” WGS84 și „X Y” în sistemul hărții; căutare rapidă a textului în toate straturile încărcate; **interogări după câmpuri** cu operatori (=, ≠, conține, începe cu, >, ≤, este gol …) legați prin **ȘI / SAU**, fără deosebire de majuscule și diacritice, cu lista valorilor existente; rezultatele se pot selecta, adăuga la selecție sau încadra pe hartă
- **Măsurare (M)**: distanță plană și pe elipsoid (Vincenty, WGS84), ultimul segment, arie și perimetru
- **Tabel de atribute (T)**: filtrare, sortare pe coloane, selectare cu evidențiere pe hartă, dublu-click pentru zoom la entitate
- **Convertor de coordonate**: un punct (introdus manual sau prin click pe hartă) sau o listă de puncte lipită din Excel („X Y” ori „ID X Y”)
- **TransDatRO** (*Instrumente → TransDatRO*): transformările oficiale ANCPI, punct cu punct sau pe fișiere text în formatul programului TransDatRO, cu salvarea rezultatului și adăugarea punctelor pe hartă (vezi secțiunea de mai jos)
- **Export pe tipuri de geometrie (DXF → SHP)** (*Fișier → Export pe tipuri de geometrie…* sau meniul stratului DXF): unul sau mai multe desene DXF sunt unite și scrise ca trei shapefile – `_puncte.shp` (puncte, texte, puncte din blocuri), `_linii.shp` (linii, polilinii, arce) și `_poligoane.shp` (polilinii închise, cercuri, hașuri) –, cu layerul AutoCAD în câmpul `Layer` și desenul de origine în câmpul `Fisier`; poliliniile închise pot fi trimise la linii; opțional doar selecția și reproiecție
- **Deschidere în masă** (*Fișier → Deschide toate fișierele de un tip dintr-un dosar…*): alegeți un dosar, aplicația găsește fișierele GIS din el și din toate subdosarele, le grupează pe tipuri (Shapefile, DXF, GeoTIFF, GeoPackage, .gdb …) cu numărul și mărimea lor, iar tipul ales se deschide în paralel, cu progres în bara de stare și buton de oprire; straturile primesc numele cu calea din subdosare (ex. `Comuna A/parcele`)
- **Conversie în lot**: un dosar întreg (opțional cu subdosarele) într-un alt format și/sau alt sistem de coordonate
- **Spațiu de lucru (.cgw)**: salvează straturile, stilurile, ordinea, sistemul hărții, vederea curentă și harta de fundal
- meniu contextual pe hartă: copiere coordonate (inclusiv lat/lon WGS84), centrare, identificare
- deschidere prin tragerea fișierelor peste fereastră sau ca argumente: `cadastru.exe fisier1.shp fisier2.tif`

### Import CGXML (cadastru ANCPI)
Fișierele `.cgxml`, arhivele `.zip` care le conțin și dosarele întregi (*Fișier → Deschide dosar cu fișiere CGXML*, sau trase peste fereastră) sunt convertite automat la deschidere și afișate ca straturile shapefile **teren** și **constructii**, în EPSG:3844. Conversia este o portare fidelă a nucleului **CGXML2SHP v3.0** (github.com/ginutzu/cgxml):
- aceleași reguli: punctele cu BUILDINGID aparțin construcției; sortare după NO; eliminarea punctelor duplicate (toleranță 0,5 mm); împărțirea în inele la revenirea în primul punct; clasificarea exterior / enclavă / parte multiplă prin vot de conținere; orientare ESRI
- aceleași câmpuri ca `teren.dbf` / `constructii.dbf`: ID, LANDID, MEAS_AREA, AREA_CALC, AREA_DIFF, PAPERCADNO, E2IDENTIF, CADSECTOR, TOPONO, CATFOLOS, ACTE, PROPRIETAR, FILENAME, OPERTYPE, EXPORTDATE
- opțiuni (*Instrumente → Opțiuni import CGXML*): închiderea inelelor, atribute extinse, construcții, inversare X↔Y, CNP/CUI în PROPRIETAR (dezactivat implicit, GDPR)
- meniul contextual al stratului: **salvează ca teren.shp / constructii.shp** sau **reîncarcă cu X și Y inversate**
- statisticile (terenuri, puncte, enclave, multipart, puncte ignorate) apar în bara de stare; parcelele sunt etichetate automat cu E2IDENTIF / PAPERCADNO

Verificat pe fișiere de test (enclave, insule în enclave, multipart, puncte nesortate, NO lipsă, LANDID negativ, construcții, ZIP cu `__MACOSX`): rezultat identic cu `core.py` din CGXML2SHP v3.0 (câmpuri, atribute, geometrii și arii).

### TransDatRO (ANCPI)
Port exact al algoritmului publicat de ANCPI în *TransDatRO_code_source* (codul sursă oficial):
- **ETRS89 ↔ Stereo 70** (Krasovski 1942) și **ETRS89 ↔ Stereo 30 București** (Hayford 1910), cu altitudini în sistemul **Marea Neagră 1975**
- pașii oficiali: proiecție stereografică oblică pe GRS80 → Helmert 2D cu 4 parametri → corecții din gridul de distorsiuni, interpolate bicubic spline pe 16 noduri; altitudinea normală se obține cu anomalia altitudinii din gridul de cvasigeoid
- cvasigeoid: **EGG97_QGRJ** pentru toată țara și gridul **București-Ilfov** (zitaBucx) în zona lui, ca în TransDatRO 4.x. Se poate impune doar EGG97 (ca în codul sursă 1.06).
- gridurile oficiale (identice în TransDatRO 1.06 și 4.08) sunt incluse în executabil. Când ANCPI publică griduri noi (de exemplu `EGG97_QGRJ.GRD`), le copiați în dosarul `grile` de lângă `cadastru.exe`, iar programul le folosește pe acelea.
- convenții: Stereo 70 x = Nord, y = Est; Stereo 30 x = Est, y = Nord (ca în TransDatRO)

Integrarea în restul aplicației:
- reproiecția din mers, exportul, conversia în lot, warp-ul raster și convertorul de coordonate folosesc **automat** TransDatRO pentru orice transformare din sau în Stereo 70 (EPSG:3844). Se poate dezactiva din *Instrumente → Reproiecție Stereo 70 cu TransDatRO*; atunci se folosește Helmert cu 7 parametri (EPSG:15994). Diferența dintre cele două metode e de ordinul a 1 m.
- în afara zonei acoperite de grid (în afara României), Stereo 70 trece automat pe Helmert cu 7 parametri, iar convertorul afișează metoda folosită
- **Stereo 1930 București** apare ca sistem de coordonate separat (căutați „stereo 30”). Se transformă numai prin TransDatRO, în zona acoperită de grid. În `.prj` și GeoTIFF este scris ca sistem local, deoarece alte programe GIS nu au o definiție pentru el.
- convertorul de coordonate transformă și altitudinea: h elipsoidal ↔ H Marea Neagră 1975
- Stereo 30 ↔ Stereo 70 se calculează prin ETRS89. TransDatRO 4.x are și o transformare directă Stereo 30 ↔ Stereo 70, cu fișiere într-un format nepublicat; față de aceasta, diferențele sunt de 2–4 cm.

### Formate

| Format | Extensii | Citire | Scriere | Observații |
|---|---|:-:|:-:|---|
| ESRI Shapefile | .shp .shx .dbf .prj .cpg | ✔ | ✔ | 2D/3D, poligoane cu goluri, codificare DBF detectată automat (.cpg, LDID, UTF-8 sau Windows-1250); scriere în UTF-8 sau Windows-1250 |
| GeoPackage | .gpkg | ✔ | ✔ | toate straturile din fișier; mai multe straturi într-un singur fișier |
| GeoJSON | .geojson .json | ✔ | ✔ | membrul „crs” pentru alte sisteme decât WGS84 |
| KML / KMZ | .kml .kmz | ✔ | ✔ | foldere, ExtendedData, MultiGeometry, gx:Track; la scriere transformă automat în WGS84 |
| GPX | .gpx | ✔ | ✔ | puncte, rute, trasee, cota (ele) |
| AutoCAD DXF | .dxf | ✔ | ✔ | citire: POINT, LINE, LWPOLYLINE/POLYLINE (cu arce „bulge”), CIRCLE, ARC, ELLIPSE, SPLINE, TEXT, MTEXT, INSERT (blocuri, cu ATTRIB), HATCH, 3DFACE, SOLID, polyface; la citire se elimină punctele cu „RTCM” în descriere (stațiile de bază GNSS), cu etichetele lor; scriere: DXF R12, compatibil cu orice program CAD |
| CSV / text | .csv .txt | ✔ | ✔ | coloane X/Y/Z sau WKT, separator `,` `;` tab, zecimale cu virgulă; dialog de import cu previzualizare |
| GeoTIFF | .tif .tiff | ✔ | ✔ | 8/16/32/64 biți, întregi sau reali, RGB/RGBA, paletă, tiled/strip, LZW/Deflate/JPEG/PackBits, NoData, BigTIFF, sisteme definite de utilizator în cheile GeoTIFF (recunoscute automat ca EPSG când se potrivesc) |
| ESRI ASCII Grid | .asc | ✔ | ✔ | |
| PNG / JPEG + world file | .png .jpg (.pgw .jgw .wld) | ✔ | ✔ | la scriere: world file, .prj și .aux.xml (recunoscute de QGIS/ArcGIS) |
| BMP + world file | .bmp | ✔ | – | |
| CGXML (cadastru ANCPI) | .cgxml, .zip, dosar | ✔ | – | convertit în straturile „teren” și „constructii” (vezi mai sus) |
| Esri File Geodatabase | dosar .gdb | ✔ | – | vezi mai jos |
| ECW | .ecw | ✔* | – | *prin GDAL-ul din QGIS / OSGeo4W; doar vizualizare (vezi mai jos) |
| MrSID | .sid | ✔* | – | *prin GDAL, cu pachetul gdal-mrsid; doar vizualizare |
| JPEG 2000 | .jp2 | ✔* | – | *prin GDAL (OpenJPEG sau ECW); doar vizualizare |

**ECW, MrSID și JPEG 2000**: formate proprietare, deschise **doar pentru vizualizare**. Decodarea are nevoie de bibliotecile producătorilor (ERDAS ECW/JP2 SDK, MrSID Decode SDK), gratuite pentru citire dar care nu pot fi incluse într-o aplicație cu licență MIT. Cadastru folosește GDAL-ul unei instalări **QGIS** (instalatorul complet include citirea ECW; MrSID cu pachetul OSGeo4W `gdal-mrsid`) sau **OSGeo4W**, găsit automat în `Program Files\QGIS …`, `C:\OSGeo4W`, PATH, ori ales în *Instrumente → Formate ECW / MrSID (GDAL)…*, fereastră care arată și driverele disponibile.
- în memorie se ține o imagine de ansamblu (cel mult 4096 px pe latură); la zoom, detaliul se citește din fișier doar pentru zona vizibilă, la rezoluția ecranului (și la rezoluția planșei, la tipărire); identificarea dă valorile pixelului la rezoluția completă
- Cadastru **nu scrie și nu convertește** în ECW / MrSID / JPEG 2000, iar aceste straturi nu se exportă în alte formate


**File Geodatabase (.gdb)**: se deschide din *Fișier → Deschide File Geodatabase (.gdb)…*, prin tragerea dosarului `.gdb` peste fereastră, ca argument (`cadastru.exe date.gdb`) sau alegând orice fișier `.gdbtable` din el. Fiecare feature class devine un strat; sunt citite și cele din feature dataset-uri. Tabelele fără geometrie apar ca straturi fără geometrie (cu tabel de atribute).
- funcționează fără ArcGIS și fără biblioteca Esri; formatul e citit direct (versiunile ArcGIS 10.x și ArcGIS Pro, inclusiv Pro 3.2+ cu întregi pe 64 de biți, date și ore separate și fusuri orare)
- puncte, multipuncte, linii, poligoane (cu goluri), 2D și 3D, sistemul de coordonate din definiția stratului
- curbele (arce de cerc, cercuri, Bézier, arce de elipsă) sunt transformate în segmente dese (cel mult 2° sau 1 mm abatere)
- atribute: întregi, reali, text, date (cu sau fără oră), GUID; câmpurile binare, raster și XML sunt omise
- nu sunt suportate: geodatabase-urile comprimate (CDF), personal geodatabase (.mdb), multipatch, adnotările, domeniile și relațiile; scrierea în .gdb nu e disponibilă (exportați în GeoPackage sau Shapefile)

### Sisteme de coordonate
- peste 6 000 de coduri EPSG, cu căutare după cod sau nume; definiții personalizate (șir PROJ sau WKT)
- recunoaște fișierele `.prj` în stil ESRI (de exemplu „Stereo_70” fără TOWGS84) și le asociază sistemului corect, EPSG:3844
- fișierele fără `.prj` primesc un sistem presupus după extindere (WGS84 sau Stereo 70), marcat cu portocaliu în lista de straturi; îl corectați din meniul contextual al stratului → „Setează sistemul de coordonate”
- **Stereo 70 ↔ ETRS89/WGS84** se face implicit cu **TransDatRO** (precizie centimetrică, vezi mai sus). Opțional, se poate folosi transformarea EPSG 15994 cu 7 parametri, care dă rezultate identice cu PROJ 9.8 (diferență sub 1 cm). Precizia acesteia din urmă e de aproximativ 1–3 m.
- WGS84 este tratat ca ETRS89 (diferență sub 1 m, ca în PROJ)

---

## 2. Linia de comandă

```bat
cadastru-cli formats
cadastru-cli info parcele.shp --features 3
cadastru-cli crs --search stereo
cadastru-cli crs EPSG:3844

:: conversie de format
cadastru-cli convert parcele.shp parcele.gpkg
cadastru-cli convert plan.dxf plan.shp --s-srs EPSG:3844

:: reproiecție
cadastru-cli convert parcele.shp parcele_wgs.kml --t-srs EPSG:4326
cadastru-cli convert ortofoto.tif ortofoto_s70.tif --t-srs EPSG:3844 --resampling bilinear --res 0.5

:: CSV cu coloane proprii (topografic, în Stereo 70 X = Nord, Y = Est)
cadastru-cli convert puncte.csv puncte.shp --x-field Y --y-field X --s-srs EPSG:3844

:: shapefile cu diacritice pentru programe vechi
cadastru-cli convert parcele.gpkg parcele.shp --out-encoding cp1250 -o

:: CGXML -> shapefile (ca CGXML2SHP): cadastru_teren.shp + cadastru_constructii.shp
cadastru-cli convert lucrare.zip D:\iesire\cadastru.shp
cadastru-cli convert D:\dosar_cgxml D:\iesire\cadastru.gpkg --cgxml-inversare-xy
cadastru-cli info imobil.cgxml --features 1

:: transformare de coordonate (argumente sau stdin)
cadastru-cli transform --s-srs EPSG:3844 --t-srs EPSG:4326 --dms 588000 328000
type puncte.txt | cadastru-cli transform --s-srs EPSG:3844 --t-srs EPSG:25835

:: TransDatRO: fișiere în formatul programului oficial (cod, B, L[, h] sau cod, x, y[, H])
cadastru-cli transdatro -d etrs89-st70 puncte_gnss.txt -o puncte_st70.txt
cadastru-cli transdatro -d st70-etrs89 borne.txt
cadastru-cli transdatro -d etrs89-st30 bucuresti.txt
:: direcții: etrs89-st70, st70-etrs89, etrs89-st30, st30-etrs89, st30-st70, st70-st30

:: reproiecție cu Helmert 7 parametri în loc de TransDatRO
cadastru-cli --fara-transdatro convert parcele.shp parcele_wgs.gpkg --t-srs EPSG:4326

:: conversie în lot (cu subdosare), totul în GeoPackage ETRS89/UTM 35N
cadastru-cli batch D:\date D:\date_gpkg --to gpkg --t-srs EPSG:25835 -r -o

:: filtre la citire: doar entitățile din România (EPSG:3844) și/sau o expresie
cadastru-cli convert adrese_europa.gpkg adrese_ro.shp --doar-romania
cadastru-cli convert uat.shp uat_active.gpkg --where "status = 1 AND is_active = 1"
cadastru-cli batch D:\europa D:\romania --to gpkg --doar-romania --partial -r -o
```

Opțiuni comune: `--s-srs` (sistemul sursă, suprascrie .prj), `--t-srs` (reproiecție), `-o` (suprascrie), `--encoding` (codificarea la citire), `--out-encoding`, `--layer` (un singur strat din GeoPackage), `--name-field` (câmpul de nume pentru KML/GPX/DXF), `--wkt` (CSV cu geometrie WKT), `--append` (adaugă la un GeoPackage existent), `--dxf-lines` (poliliniile închise rămân linii), `--dxf-pastreaza-rtcm` (nu elimina punctele cu „RTCM” în descriere), `--cgxml-fara-constructii`, `--cgxml-inversare-xy`, `--cgxml-cnp`, `--cgxml-dbf-simplu`, `--cgxml-fara-inchidere`, `--fara-transdatro` (Stereo 70 cu Helmert 7 parametri), `--doar-egg97` (altitudini doar cu cvasigeoidul național), `--doar-romania` (doar entitățile din încadrarea României în EPSG:3844), `--zona-3844 "minX,minY,maxX,maxY"` (alt dreptunghi), `--partial` (și entitățile care doar ating zona), `--where "EXPRESIE"` (doar entitățile care îndeplinesc expresia). Codul de ieșire e 0 la succes, 1 la eroare și 2 dacă o conversie în lot a avut erori.

---

Export pe tipuri de geometrie (DXF → trei shapefile: puncte, linii, poligoane):

```bat
cadastru-cli split D:\iesire\plan.shp desen1.dxf desen2.dxf --s-srs EPSG:3844
cadastru-cli split D:\iesire\toate.shp D:\desene -r --inchise-ca-linii -o
```

Opțiuni: `--fara-puncte`, `--fara-linii`, `--fara-poligoane`, `--inchise-ca-linii` (poliliniile închise ca linii; hașurile rămân poligoane), `--fara-camp-fisier`, `-r` (subdosare), plus `--s-srs`, `--t-srs`, `--out-encoding`, `-o`.

## 3. Scurtături

| Tastă | Acțiune |
|---|---|
| F1 | manualul de utilizare (PDF) |
| F5 | Redraw: curăță grafica temporară și redesenează harta |
| Ctrl+L | ascunde / arată panoul Straturi |
| Ctrl+O | deschide fișiere |
| Ctrl+S | salvează spațiul de lucru |
| Ctrl+P | tipărire |
| Home | extindere completă |
| + / - | mărire / micșorare |
| P / Z / X / I / S / M | deplasare / zoom + / zoom − / identificare / selectare / măsurare |
| Ctrl+F | căutare |
| Ctrl+Shift+F | selectare după atribute |
| Ctrl+Shift+A | deselectează tot |
| săgeți | deplasare cu un sfert de ecran |
| Backspace / Alt+Dreapta | vederea anterioară / următoare |
| T | tabel de atribute |
| Esc | anulează măsurarea și evidențierea; cu instrumentul Selectare golește selecția |
| dublu-click | zoom ×2 (cu instrumentul Deplasare) sau încheie măsurarea |
| click dreapta | meniu contextual (pe hartă sau pe strat) |

---

## 4. Ce nu face (față de Global Mapper)

Cadastru acoperă vizualizarea, conversia de format și reproiecția. Nu include:
- analize de teren: curbe de nivel, pante, volume, profile, bazine
- nori de puncte LiDAR (LAS/LAZ)
- servicii WMS/WMTS (hărțile de fundal sunt doar tile-uri XYZ), digitizare și editare de geometrii, compoziții de tipărire cu mai multe hărți sau planșe pe mai multe pagini
- formatul DWG (folosiți DXF); scrierea în File Geodatabase; scrierea sau conversia în ECW, MrSID, JPEG 2000 (acestea se pot doar vizualiza, prin GDAL)

Rasterele GeoTIFF / ASC / PNG / JPEG se încarcă integral în memorie. Pentru ortofotoplanuri foarte mari (mulți GB), decupați-le întâi sau folosiți ECW / JPEG 2000, care se citesc pe ferestre.

Dacă aplicația nu pornește într-o sesiune Remote Desktop pe Windows mai vechi, cauza e lipsa OpenGL în sesiunea remote. `cadastru-cli.exe` funcționează oricând.

---

## 5. Construire din sursă

1. Instalați Rust de la https://rustup.rs (varianta implicită, MSVC).
2. În dosarul `sursa`:
   ```bat
   cargo build --release
   ```
3. Executabilele apar în `sursa\target\release\`.

Teste: `cargo test`.

### Structura codului

```
src/
  lib.rs           biblioteca (folosită de ambele executabile)
  query.rs         interogări pe atribute (condiții cu ȘI / SAU)
  expr.rs          expresii pe atribute în stilul SQL WHERE (selectare după atribute, --where)
  area.rs          filtrul pe zonă la încărcare (încadrarea României în EPSG:3844)
  i18n.rs          limba interfeței (RO/EN), comutarea și traducerea textelor din constante
  model.rs         geometrii, atribute, straturi vectoriale, rastere
  crs.rs           sisteme de coordonate, transformări, corecții EPSG
  transdatro.rs    algoritmul TransDatRO (ANCPI) și formatul fișierelor lui
  geomio.rs        WKB / WKT
  convert.rs       conversie de fișiere și conversie în lot
  rasterops.rs     reproiecție raster (warp), reeșantionare, randare, hillshade
  formats/         shp, gpkg, geojson, kml, gpx, dxf, csv, cgxml, gdb (File Geodatabase), raster (GeoTIFF/ASC/PNG/JPG), gdalrt (ECW / MrSID / JPEG 2000 prin GDAL încărcat dinamic)
  main.rs          punctul de intrare al interfeței grafice
  gui/             fereastra principală, harta, hărțile de fundal (basemap.rs), tipărirea și PDF-ul (print.rs), selecția (select.rs), căutarea (search.rs), selectarea după atribute (attrsel.rs), filtrul de încărcare (loadfilter.rs), exportul pe tipuri (split.rs), deschiderea în masă (bulk.rs), manualul inclus (manual.rs), controlul de navigație, temele, dialogurile, spațiul de lucru
  bin/cli.rs       linia de comandă
i18n-macros/       macroul tr! (crate proc-macro): alege textul român sau englez, verificat la compilare
lang/en.tsv        pachetul pentru limba engleză: text românesc <TAB> text englez
assets/            pictograma, resursa Windows și gridurile TransDatRO
docs/              manualul de utilizare (PDF), inclus în executabil
```

Biblioteci folosite: egui/eframe (interfață), proj4rs + crs-definitions (proiecții EPSG), proj4wkt (WKT), tiff, image, rusqlite (SQLite inclus), earcutr (triangulare), roxmltree, zip, encoding_rs, rfd (dialoguri de fișiere Windows), clap, ureq + rustls (descărcarea tile-urilor), winreg (proxy-ul Windows), tiny-skia + ab_glyph (desenarea planșei de tipărire), windows-sys (tipărirea prin Windows), libloading (GDAL-ul opțional pentru ECW / MrSID, încărcat la rulare).

---

## 6. Validare

Testată față de PROJ 9.8, GDAL 3.12 și programul oficial TransDatRO ca referințe independente:
- **TransDatRO**:
  - setul oficial de puncte de test (Stereo 70 și Stereo 30, în ambele sensuri) trece în toleranța cerută de ANCPI (3 mm, 0,00003″);
  - pe 100.000 de puncte aleatoare pe toată țara și în București, rezultatele sunt identice cu programul Java oficial (`TransDatRO_code_source.jar`): coordonate până la ultima zecimală afișată, altitudini identice, aceleași puncte respinse ca fiind în afara zonei
- transformări de coordonate (puncte de pe tot teritoriul României): Stereo 70 ↔ WGS84 cu Helmert 7 parametri identic cu PROJ (sub 1 mm); Stereo 70 → ETRS89 UTM 35N și LAEA sub 1,3 cm; UTM, LAEA și Web Mercator identice
- conversii dus-întors SHP / GPKG / GeoJSON / KML / KMZ / GPX / DXF / CSV citite înapoi cu GDAL: geometrii identice (sub 1 mm pentru formatele în WGS84), atribute și diacritice păstrate, cote Z păstrate
- reproiecție raster comparată cu GDAL warp pe aceeași grilă: 99,7–99,9% pixeli identici (cel mai apropiat vecin)
- GeoTIFF, ASC, PNG: georeferențiere, tip de date, NoData și valori păstrate exact

- **File Geodatabase**: comparat cu GDAL (driverul OpenFileGDB) pe cele 99 de straturi din setul de teste GDAL (toate tipurile de geometrie, curbe, tabele rare, versiunile 9.x/10.x/Pro 3.2, câmpuri de dată cu fus orar). La 98 de straturi, numărul de entități, OBJECTID-urile, atributele și geometriile sunt identice. Singura diferență e aproximarea unei curbe Bézier: lungimea noastră e mai apropiată de lungimea stocată de ArcGIS (SHAPE_Length).
- **PDF**: fișierele trec verificările `qpdf --check`, pdfinfo și pypdf; mărimea paginii e exactă (A4 = 841,89 × 595,28 pt)

Dosarul `exemple` conține datele de test (inclusiv `cadastru.gdb`, aceleași parcele, drumuri și puncte ca File Geodatabase) și un spațiu de lucru (`test.cgw`) care le deschide pe toate.

- hărți de fundal: poziționarea tile-urilor verificată cu un server de test care desenează în tile-uri puncte calculate în Web Mercator, suprapuse peste aceleași puncte încărcate ca strat vectorial (Stereo 70, la toate scările); descărcarea HTTPS prin proxy și cache-ul verificate; eroarea de reproiecție a plaselor de tile-uri sub 0,1 pixeli pe toată România, la nivelurile 5–19

---

## 7. Documentația și licența

Sursele documentației sunt în `docs/src` (HTML + CSS, capturile în `docs/src/img`). PDF-urile se construiesc cu Chromium prin Playwright și pypdf:

```bash
cd docs/src
python3 build.py manual.html ../Manual_Cadastru.pdf "Cadastru 1.12.0 · Manual de utilizare"
python3 build.py documentatie.html ../Documentatie_tehnica_Cadastru.pdf "Cadastru 1.12.0 · Documentație tehnică"
```

`docs/Manual_Cadastru.pdf` este inclus în `cadastru.exe` (tasta F1), deci după reconstruirea lui trebuie recompilată aplicația.

Gridurile TransDatRO din `assets/transdatro` sunt cele publicate de ANCPI pentru programul oficial TransDatRO.

Licență: [MIT](LICENSE) © 2026 Ginu Popescu.

