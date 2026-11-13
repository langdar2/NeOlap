# NeOlap – Implementierungsplan

Version 10 · Stand 13.11.2026 · Neuimplementierung von TinyOlap 0.8.27 (Python, MIT, Thomas Zeutschler) in Rust

Änderungen gegenüber Version 9: Dimensionsverwaltung in API und Web-Oberfläche (Anlegen, Umbenennen, CSV-Import mit Vorschau, Löschen; Kap. 6.3 und 7, Entscheidungen E89–E92). Veröffentlicht am 13.11.2026 als `v1.1.0` (4 Binaries, 5 Python-Wheels, Image `ghcr.io/langdar2/neolap` für amd64 und arm64). Der Image-Pull ohne Anmeldung ist geprüft: Das Manifest von `1.1.0` und `latest` ist anonym abrufbar (HTTP 200, beide Plattformen); das Image selbst wurde dabei nicht gestartet. Das Repository ist öffentlich. Die Runner-Labels `macos-14` in `release.yml` und `wheels.yml` wurden durch `macos-latest` ersetzt, weil GitHub `macos-14` nicht mehr bedient. Offen: Referenzhardware für die Zielwerte (E85), Abnahme der Web-Oberfläche durch eine Person.

Release 1.1.0 (13.11.2026): Dimensionsverwaltung in API und Web-Oberfläche; Einzelheiten im [CHANGELOG](../CHANGELOG.md).

Release 1.0.1 (05.11.2026): breitere Pivot-Spalten, Beispiel „BAB-Planung“, Doku-Stand; Einzelheiten im [CHANGELOG](../CHANGELOG.md).

Änderungen gegenüber Version 8: Release 1.0.0 vorbereitet (Version gesetzt, CHANGELOG). Veröffentlicht am 05.11.2026 als `v1.0.0`: Binaries (4 Plattformen), Python-Wheels (5 Plattformen), Image `ghcr.io/langdar2/neolap` (amd64, arm64). Offen: Referenzhardware für die Zielwerte (E85), Abnahme der Web-Oberfläche durch eine Person, Image-Pull ohne Anmeldung nicht geprüft.

Änderungen gegenüber Version 7: M11 teilweise umgesetzt – Zielwerte an einem synthetischen Großmodell gemessen (E83–E85), zeitgesteuerte Sicherung im Server (E87), Handbuch ([Handbuch.md](Handbuch.md)), Workflows für Container-Messung und Release-Binaries. Offen: Referenzhardware für die Zielwerte, Release-Tag, veröffentlichtes Image.

Änderungen gegenüber Version 6: Python-Modul umgesetzt (Kap. 9); API-Tokens als SHA-256 statt Argon2 (E72); Basiszellen werden mit einem Hash-Zugriff gelesen (E73); Spaltenarten kommen aus der Position (E75).

Änderungen gegenüber Version 5: Web-Oberfläche umgesetzt (Kap. 7); Zahleneingabe nach deutscher Lesart, ein Punkt vor genau drei Ziffern ist ein Tausendertrenner (E61); strikte CSP und DOM ohne Markup aus Daten (E63, E64); der Server beendet beim Herunterfahren die Event-Streams (E69).

Änderungen gegenüber Version 4: Mehrbenutzer-Server umgesetzt (Kap. 6); Lesen gilt je Cube, Schreiben je Zelle (E51) – Leserechte auf Elementebene bleiben Phase 2; erster Administrator über Umgebungsvariablen (E55); Import in zwei Schritten (E56).

Änderungen gegenüber Version 3: Speicher- und Indexdesign nach Messung in M2 geändert (kein Roaring-Index, Abfrage per paralleler Spaltenabtastung; Kap. 4.2).

Änderungen gegenüber Version 2: 20 Mio. gefüllte Basiszellen als festes Maximum bestätigt; Server-Betrieb überall per Docker Compose (Kap. 6.4), umgesetzt bereits in M0.

Änderungen gegenüber Version 1: Projektname NeOlap; Größenrahmen 20 Dimensionen / 500.000 Elemente / 50 Ebenen; gemeinsame Nutzung ab v1 (Benutzer, Rechte, Konflikterkennung, Änderungsjournal); Speicher- und Indexdesign daraufhin überarbeitet; Excel-Anbindung bleibt Phase 2.

## 1. Ziel und Rahmen

NeOlap ist ein leichtgewichtiges, modellgetriebenes MOLAP-Werkzeug im Geist von MIS Alea / TM1: Dimensionen mit Hierarchien, Cubes, Rechenregeln, Rückschreiben auf jeder Ebene, Datenimporte. Es läuft als einzelnes Programm auf macOS (Apple Silicon und Intel), Linux und Windows, ohne Datenbankserver und ohne Lizenzkosten.

| Thema | Entscheidung |
|---|---|
| Umfang v1 | Kern-Engine, Mehrbenutzer-Server, Web-Oberfläche, Kommandozeile, Python-Modul |
| Regeln | Eigene Formelsprache (TM1/Alea-artig), mit dem Modell gespeichert |
| Rückschreiben | Direkt (Zelleingabe, auf Aggregaten mit Splashing) und per Datenimport |
| Nutzung | Gemeinsam: ein NeOlap-Server, mehrere Nutzer im Browser, per Python oder CLI |
| Bereitstellung | Server überall per `docker compose up` (Linux, macOS, Windows; amd64 und arm64) |
| Excel | Phase 2, erst wenn v1 stabil läuft |
| Ablage | Öffentliches GitHub-Repository `langdar2/NeOlap` |

### Größenrahmen und Leistungsziele

| Größe | Zielwert v1 |
|---|---|
| Dimensionen pro Cube | bis 20 |
| Elemente pro Dimension | bis 500.000 (Basis + Aggregate) |
| Hierarchietiefe | bis 50 Ebenen, ungleich verteilt (unbalancierte Bäume, Mehrfach-Eltern erlaubt) |
| Gefüllte Basiszellen | bis 20 Mio. (festes Maximum über alle Cubes einer Datenbank) |
| Gleichzeitige Nutzer | bis ca. 20 aktiv |
| Speicherbedarf | ca. 100 Byte je gefüllter Basiszelle bei 20 Dimensionen (gemessen) → 20 Mio. Zellen ≈ 2,4 GB RAM; Container-Standardlimit 4 GB |
| Einzelne Aggregatzelle über 10 Mio. Datensätze | < 100 ms (kalt), < 5 ms (aus Cache) |
| Pivot-Ansicht 50 × 20 Zellen mit Nullunterdrückung | < 500 ms bei 10 Mio. Datensätzen |
| Splash auf 100.000 Basiszellen inkl. Speichern | < 2 s |
| Import 1 Mio. CSV-Zeilen | < 10 s |
| Start (Laden) bei 10 Mio. Zellen | < 20 s |
| Dimensionsimport 500.000 Elemente (Eltern-Kind) | < 5 s |

Nicht in v1: Excel-Anbindung, vollständige Historie mit Zeitpunkt-Abfragen (das Änderungsjournal ist enthalten), Textzellen in Cubes, Python-Funktionsregeln, verteilte/hochverfügbare Server.

## 2. Bezug zum Original

Übernommen aus TinyOlap: Datenbank → Dimensionen → Cubes; Hierarchien mit Mehrfach-Eltern und Gewichten (z. B. Profit = Sales − Cost über Gewicht −1); Aliase, Attribute, Subsets, Zahlenformate; Elementnamen ohne Beachtung von Groß-/Kleinschreibung; dünn besetzte Speicherung nur auf Basisebene, Aggregation zur Abfragezeit; Regel-Geltungsbereiche „alle Ebenen“, „nur Basis“ (mit Feeder), „nur Aggregate“; Fehlerwerte #DIV0, #REF, #ERR; SQLite als Speicherformat; Beispielmodelle tiny, tutor, enterprise als Referenz.

Neu oder geändert:

- Schreiben auf aggregierte Zellen mit Splashing (das Original bricht hier mit „Write back to aggregated cells is not (yet) supported“ ab).
- Formelsprache statt Python-Regeln; Regeln sind Teil des Modells.
- Mehrbenutzerbetrieb mit Anmeldung, Rollen, Schreibrechten bis auf Elementebene, Konflikterkennung und Journal.
- Indexdesign für große, tiefe Dimensionen (Kap. 4.2). Das Original trägt jede Zeile in die Indexmenge jedes Vorfahren ein; bei 20 Dimensionen × 50 Ebenen wären das bis zu 1.000 Indexeinträge pro Zelle – für den neuen Größenrahmen nicht tragfähig.
- Mehrfach erreichbare Basiselemente: Wie im Original zählt ein Basiselement, das über mehrere Pfade unter einem Aggregat hängt, nur einmal (am Original geprüft: T = A + B mit A = {x, y}, B = {x, z} ergibt 3). Widersprüchliche Pfadgewichte (x unter A mit +1, unter B mit −1) liefern im Original ein zufallsabhängiges Ergebnis; NeOlap lehnt solche Hierarchien beim Speichern mit einer klaren Meldung ab.
- Deklarative, wiederholbare Import-Spezifikationen; Undo; Modell als lesbare Dateien exportierbar.
- Das Original läuft auf Python ≥ 3.10 nicht mehr (`from collections import Iterable`). Für Differenztests wird eine minimal gepatchte Kopie genutzt.

## 3. Architektur

```
NeOlap/
├─ crates/
│  ├─ neolap-core/     Bibliothek: Modell, Speicher, Abfrage, Splashing, Regeln, Persistenz, Import/Export, Rechte
│  ├─ neolap-server/   Mehrbenutzer-Server: REST-API, Anmeldung, Ereignisse (SSE), eingebettete Web-UI
│  ├─ neolap-cli/      Programm „neolap“: Verwaltung, Import, Abfragen, „neolap serve“
│  └─ neolap-py/       Python-Modul „neolap“ (PyO3 + maturin), lokal oder als Client eines Servers
├─ ui/                 Web-Oberfläche (HTML/CSS/JavaScript-Module, kein Build-Schritt)
├─ samples/            Beispielmodelle + Daten
├─ tests/golden/       Referenzergebnisse aus dem Python-Original
├─ docs/               Plan, Architekturentscheidungen, Handbuch
├─ deploy/             Caddyfile (TLS-Proxy)
├─ Dockerfile, docker-compose.yml, .env.example
└─ .github/workflows/  CI, Docker-Build/-Test/-Veröffentlichung
```

Die gesamte Fachlogik liegt in `neolap-core`; Server, CLI und Python-Modul sind dünne Hüllen. Die Web-UI wird in das Server-Binary eingebettet, sodass NeOlap als eine Datei ausgeliefert wird.

Betriebsarten:

| Modus | Beschreibung |
|---|---|
| Server | `docker compose up -d` oder `neolap serve` – ein Prozess hält die Datenbank, Nutzer arbeiten per Browser, Python (`neolap.connect(url)`) oder CLI (`--server url`). Standard für gemeinsame Nutzung. |
| Lokal | CLI oder Python öffnen die Datei direkt, exklusiv (Sperrdatei). Für Einzelarbeit, Tests, Notebooks, Batch-Importe bei gestopptem Server. |

Zentrale Abhängigkeiten (aktuelle Versionen geprüft am 03.11.2026): `hashbrown` 0.17, `rayon` 1, `parking_lot` 0.13, `serde`/`serde_json`, `rusqlite` 0.41 (bundled), `thiserror` 2, `csv` 1.4, `calamine` 0.38, `rust_xlsxwriter` 0.102, `axum` 0.8, `tokio` 1.54, `tower-http` 0.7, `rust-embed` 8, `clap` 4.6, `argon2` 0.6, `tracing`, `pyo3` 0.30; Tests: `proptest`, `criterion`, `insta`.

## 4. Kern-Engine (neolap-core)

### 4.1 Dimensionen für 500.000 Elemente und 50 Ebenen

```rust
pub type MemberId = u32;

pub struct Dimension {
    name: String,
    kind: DimensionKind,                    // Generic, Time, Version, Measures, …
    names: Vec<CompactString>,              // Index = MemberId; gelöschte Slots wiederverwendbar
    lookup: HashMap<UniCase, MemberId>,     // Namen + Aliase, case-insensitive
    parents:  Csr<(MemberId, f32)>,         // kompakte Adjazenz (CSR) mit Kantengewicht
    children: Csr<(MemberId, f32)>,
    level: Vec<u8>,                         // 0 = Basis; Höhe über Basis
    depth: Vec<u8>,                         // Abstand von der Wurzel
    leaf_ord: Vec<u32>,                     // Basiselemente in DFS-Reihenfolge nummeriert
    leaf_range: Vec<Option<Range<u32>>>,    // je Aggregat: zusammenhängender Bereich, falls vorhanden
    leaf_sets: LruCache<MemberId, Arc<LeafSet>>, // Basismenge + Gewichte, bei Bedarf berechnet
    attributes: AttributeTable, subsets: Vec<Subset>, formats: …
}
```

Wesentliche Designpunkte:

- Basismengen von Aggregaten werden nicht vollständig vorberechnet (500.000 Elemente × bis zu 50 Ebenen wären bis zu 25 Mio. Einträge pro Dimension), sondern bei Bedarf per Tiefensuche ermittelt und im LRU-Cache gehalten. Darstellung als `LeafSet`: entweder Bereich `leaf_ord[a..b]` (bei reinen Bäumen der Normalfall, da Basiselemente in DFS-Reihenfolge nummeriert sind), oder Bitmenge über die Basiselemente (500.000 Bit = 61 KB), plus optional eine Gewichtstabelle nur für Basiselemente mit Gewicht ≠ 1.
- Mehrfach-Eltern (alternative Hierarchien) sind erlaubt; die DFS-Nummerierung folgt der ersten Hierarchie, alternative Aggregate nutzen Bitmengen.
- Änderungen über einen `DimensionEditor` mit `add`, `add_children`, `rename`, `remove`, `move_member`, `set_weight`, `set_alias`, Attributpflege; `commit()` prüft Zyklen, Namens-/Aliaskonflikte und widersprüchliche Pfadgewichte, berechnet `level`, `depth`, DFS-Nummerierung und invalidiert Caches. Commit für 500.000 Elemente < 1 s.
- Wird ein Basiselement gelöscht, werden seine Fakten entfernt (mit Rückfrage und Journal). Wird ein Basiselement zum Aggregat, werden seine Fakten wahlweise verworfen oder auf ein neues Kind „<Name> (Rest)“ verschoben.
- Elementsuche für die UI: Präfix- und Teilstringsuche über Namen und Aliase (sortierter Namensindex), damit auch bei 500.000 Elementen sofortige Treffer möglich sind.

### 4.2 Faktenspeicher und Abfrage

Nur Basiszellen werden gespeichert, spaltenorientiert:

```rust
pub struct FactStore {
    n_dims: usize,
    cols: Vec<Vec<u32>>,                  // je Dimension eine Spalte mit leaf_ord der Zeile
    values: Vec<f64>,
    key_index: HashTable<u32>,            // Hash über die Koordinaten → Zeile (ohne Schlüsselkopie)
    free_rows: Vec<u32>, live: Vec<bool>,
    counts: Vec<Vec<u32>>,                // je Dimension: Zeilenzahl je Basiselement (Selektivitätsschätzung)
}
```

Statt Indexmengen für jedes Aggregat (Original) gibt es keinen Zeilenindex pro Element, sondern nur Zeilenzähler je Basiselement. Der Roaring-Index der ersten Fassung kostete bei 20 Dimensionen rund 120 B je Zelle (mehr als alle Spalten zusammen); ein paralleler Spaltenscan über 20 Mio. Zeilen ist mit 2–5 ms schneller als der Umweg über Bitmaps. Abfrage einer Zelle mit Aggregaten:

1. Für jede Dimension das `LeafSet` des angefragten Elements holen; Dimensionen mit „alles“ (Wurzel ohne Gewichte) entfallen. Geschätzte Trefferzahl je Dimension aus den Zeilenzählern.
2. Die aktiven Dimensionen nach Selektivität ordnen, damit die Prüfung je Zeile früh abbricht.
3. Alle Zeilen parallel (`rayon`, Blöcke zu 64 Ki Zeilen) abtasten: je Dimension Bereichsprüfung (`a <= leaf_ord < b`) oder Bit-Test, Gewichte nur für Dimensionen mit Gewichtstabelle.
4. Summieren; Ergebnis im Aggregat-Cache ablegen.

Für Pivot-Ansichten gibt es zusätzlich einen Gruppierungspfad: Alle Fakten werden in einem Durchlauf den sichtbaren Zeilen-/Spaltenelementen zugeordnet (Basiselement → sichtbare Vorfahren auf der Achse, vorab als Tabelle berechnet) und aufsummiert. Das liefert die Nullunterdrückung gratis mit. Ein Aggregat-Cache je Cube (Schlüssel: Zelladresse) wird bei Schreibvorgängen geleert.

Speicherbedarf bei 20 Dimensionen (gemessen, 20 Mio. Zellen, 20 × 1.000 Basiselemente): Spalten 85 B (Wachstum um 1/8 statt Verdoppeln), Wert + Lebend-Flag 10 B, Hash-Index 7 B → rund 102 B je Zelle, Prozessspeicher 2,4 GB. Ladezeit 20 Mio. Zellen im Speicher 7 s; Abfragen: Aggregatzelle kalt 2–5 ms, Pivot 50 × 20 in 4 ms. Optionale spätere Optimierung: Bit-Packing der Spalten (19 Bit für 500.000 Elemente).

Werte sind `f64`. Leere Zellen sind „nicht vorhanden“, nicht 0; Schreiben von 0 oder leer löscht die Zelle.

### 4.3 Rückschreiben und Splashing

```rust
cube.write(&ctx, &addr, Input::parse("500")?)?;   // ctx = Nutzer, Rechte, erwartete Version
cube.undo(&ctx)?;
```

| Modus | Eingabe | Verhalten |
|---|---|---|
| Proportional (Standard) | `500` | Verteilt den Zielwert im Verhältnis der vorhandenen Basiswerte; sind alle leer oder ihre Summe 0, Ausweichen auf „Gleich“. |
| Gleich | `#500` | Gleichmäßig auf alle Basiskombinationen des Bereichs. |
| Delta | `+50` / `-50` | Addiert die Differenz proportional. |
| Prozent | `+5%` / `-3%` | Skaliert alle Basiswerte des Bereichs. |
| Referenz | `@Actual,2026:500` | Proportional zur Struktur eines anderen Bereichs. |
| Löschen | `0` oder leer | Löscht alle Basiswerte im Bereich. |

Schutzmechanismen: Zellgrenze für „Gleich“ (Standard 1 Mio. Zellen; die UI zeigt vorher die Zahl betroffener Zellen); Ablehnung bei Aggregaten mit Gewichten ≠ 1 im Zielbereich (Verteilung nicht eindeutig); Regelzellen sind gesperrt; Schreibrechte werden für alle betroffenen Basiszellen geprüft (Kap. 6.2); ein Splash ist eine Transaktion; Rundungsdifferenzen werden auf die größte Zelle gebucht, damit die Zielsumme exakt stimmt.

Undo: pro Nutzer ein Stapel der letzten 50 Operationen (Einzelwert, Splash, Einfügen, Import). Ein Undo wird abgelehnt, wenn ein anderer Nutzer betroffene Zellen inzwischen geändert hat (Prüfung über Zellversionen im Journal); die UI zeigt dann, wer was geändert hat.

### 4.4 Formelsprache für Regeln

```
# Kennzahl auf allen Ebenen
['Profit in %'] = IF(['Sales'] = 0, NULL, ['Profit'] / ['Sales']);

# nur Basisebene; Aggregate summieren die Regelergebnisse, Feeder bestimmt die Zeilen
N: ['Revenue'] = ['Quantity'] * ['Price']  FEED ['Quantity'];

# nur Aggregate
C: ['Price'] = ['Revenue'] / ['Quantity'];

# Planfortschreibung mit Dimensionsqualifizierer
[version:'Plan', years:'2027'] = [version:'Actual', years:'2026'] * 1.03;

# Attribute und Bezug auf andere Cubes
['Sales EUR'] = ['Sales'] * DB('fx', !years, ATTR('regions', !regions, 'currency'), 'Rate');
```

Semantik: Ziel links ist ein Muster (nicht genannte Dimensionen = alle Elemente; ein Aggregat im Muster meint das Aggregat selbst, `DESC('Total')` alle Nachfahren). Bezüge rechts sind relativ zur berechneten Zelle; `!dim` liefert den aktuellen Elementnamen. Präfixe: ohne = alle Ebenen, `N:` = nur Basis, `C:` = nur Aggregate (entspricht ALL_LEVELS, BASE_LEVEL, AGGREGATION_LEVEL im Original). Die erste passende Regel gewinnt; `CONTINUE` gibt an die nächste weiter. Operatoren `+ - * / ^`, Vergleiche, `AND OR NOT`; Funktionen v1: `IF, ISEMPTY, MIN, MAX, ABS, ROUND, SQRT, LN, EXP, ATTR, DB, ELEMLEVEL, ISLEAF, PARENT, DESC, CONTINUE`. Fehler werden zu Zellwerten `#DIV0`, `#REF`, `#ERR`, `#CYCLE` (Rekursionstiefe > 64).

Umsetzung: Lexer + Pratt-Parser → AST mit Positionen (präzise Fehlermeldungen) → Kompilieren mit aufgelösten MemberIds. Regelzuordnung über eine Mustertabelle je Cube mit Vorfilter auf der Measures-Dimension und Bitmengen je Dimension. Bei Modelländerungen werden Regeln neu kompiliert; ungültige Regeln werden markiert, statt das Modell zu blockieren. Regeländerungen sind Modelländerungen und brauchen die Rolle „Modellierer“.

### 4.5 Persistenz

Eine Datenbank = ein Verzeichnis oder eine SQLite-Datei `*.neolap`:

- `meta`: Modell als JSON (Dimensionen, Cubes, Regeltexte, Ansichten, Import-Spezifikationen), `schema_version` mit Migrationen.
- `facts_<cube>(k BLOB PRIMARY KEY, v REAL)` mit kompaktem Schlüssel; Änderungen write-through, eine Operation = eine Transaktion, WAL-Modus.
- `journal(id, ts, user, op, cube, area, n_cells, detail)` und `journal_cells(op_id, k, old, new)`: wer hat wann was geändert (Grundlage für Undo, Nachvollziehbarkeit und Konflikterkennung). Aufbewahrung konfigurierbar (Standard 90 Tage).
- `users`, `roles`, `grants` für die Rechteverwaltung.
- Schneller Start: Neben der SQLite-Datei optional ein spaltenorientierter Binär-Snapshot je Cube, der beim Start direkt geladen wird; SQLite bleibt die maßgebliche Quelle. Wird umgesetzt, falls das Startziel (< 20 s bei 10 Mio. Zellen) sonst nicht erreicht wird.
- Online-Backup über die SQLite-Backup-API (`neolap backup`, auch zeitgesteuert im Server).
- `neolap model export/import`: Modell als lesbare Dateien (`dimensions/*.csv` Eltern-Kind, `cubes/*.toml`, `rules/*.rules`) für Versionierung in Git.

### 4.6 Datenimport und -export

Import-Spezifikation (TOML, im Modell speicherbar):

```toml
name   = "Ist-Umsatz laden"
cube   = "sales"
source = { file = "ist_2026.csv", delimiter = ";", decimal = "," }   # auch .xlsx (Blatt angeben)

[map]
years    = "Jahr"
months   = { column = "Monat", alias = true }
regions  = "Region"
products = "Artikelgruppe"

[fixed]
version = "Actual"

[values]          # breites Format: Spalte → Measure
Sales = "Umsatz"
Cost  = "Kosten"

[options]
mode           = "replace_area"   # replace_area | add | overwrite
unknown_member = "reject"         # reject | skip | create_under:"Nicht zugeordnet"
```

`replace_area` löscht zuerst genau den Bereich, den die Datei abdeckt, und lädt dann – Importe sind damit wiederholbar. Jeder Import ist eine Transaktion, prüft Schreibrechte, landet im Journal, ist per Undo rücknehmbar und liefert ein Protokoll (gelesene Zeilen, geschriebene Zellen, abgelehnte Zeilen mit Grund); `--dry-run` zeigt nur das Protokoll. Import im Server läuft im Hintergrund mit Fortschrittsanzeige; Lesezugriffe anderer Nutzer bleiben währenddessen möglich (sie sehen den Stand vor dem Import, bis dieser abgeschlossen ist).

Dimensionsimport aus Eltern-Kind-Listen (`child;parent;weight`) oder Ebenenspalten (`L1;…;L50`), Attribute aus Zusatzspalten, Abgleichmodus (nur hinzufügen / synchronisieren mit Löschvorschau). Export beliebiger Bereiche nach CSV oder xlsx, lang oder breit, wahlweise mit Aggregaten und Regelwerten.

## 5. Nebenläufigkeit

- Der Datenbankzustand liegt hinter einem Lese-/Schreibsperrmechanismus je Cube; Modelländerungen sperren die Datenbank kurz exklusiv.
- Große Schreibvorgänge (Import, großer Splash) werden zunächst in einem Änderungspuffer vorbereitet und dann in einem kurzen exklusiven Schritt übernommen; Leser warten also nur Millisekunden.
- Jede Zelle hat implizit eine Version (Zähler der letzten Änderung, abgeleitet aus Journal und Cube-Generation). Clients senden beim Schreiben die Generation ihrer Ansicht mit; wurde eine betroffene Zelle seitdem von jemand anderem geändert, kommt eine Konfliktmeldung mit aktuellem Wert und Verursacher – der Nutzer entscheidet, ob er überschreibt.
- Änderungen werden per Server-Sent Events an offene Ansichten gemeldet; die UI aktualisiert betroffene Zellen und markiert sie kurz.

## 6. Mehrbenutzer-Server (neolap-server)

### 6.1 Anmeldung und Betrieb

- Benutzer mit Passwort (Argon2-Hash), Sitzung per sicherem Cookie; API-Tokens für Python/CLI/Skripte. Anbindung an Firmen-SSO (OIDC) ist für Phase 2 vorgesehen.
- Standardmäßig nur `127.0.0.1`; für das Team `--listen 0.0.0.0`. Ab Netzbetrieb ist TLS Pflicht: eingebautes TLS (rustls) mit eigenem Zertifikat oder Betrieb hinter einem Reverse Proxy.
- Bereitstellung: standardmäßig per Docker Compose (Kap. 6.4), alternativ als einzelnes Binary. Konfiguration über Umgebungsvariablen `NEOLAP_*` (Container) bzw. Kommandozeilenoptionen, später ergänzend `neolap.toml`; Logging über `tracing` nach stdout; Healthcheck-Endpunkt `/api/v1/health`.

### 6.2 Rollen und Rechte

| Rolle | Darf |
|---|---|
| Admin | Benutzer, Rechte, Backups, alles |
| Modellierer | Dimensionen, Cubes, Regeln, Import-Spezifikationen ändern |
| Planer | Lesen und Schreiben im freigegebenen Bereich, Importe ausführen |
| Leser | Nur lesen |

Rechte je Cube: lesen / schreiben / kein Zugriff. **Lesen gilt je Cube, Schreiben je Zelle** (E51): wer einen Cube lesen darf, liest ihn vollständig. Zusätzlich Bereichsrechte über Elemente einer oder mehrerer Dimensionen (z. B. Planer „Nord“ darf nur unter `regions:'Nord'` und `version:'Plan'` schreiben). Ein Recht auf ein Aggregat gilt für alle Nachfahren. Bei Splash und Import muss jede betroffene Basiszelle beschreibbar sein, sonst wird die gesamte Operation abgelehnt (mit Angabe des ersten Verstoßes). Leserechte auf Elementebene (Ausblenden von Elementen) sind Phase 2.

### 6.3 REST-API

| Methode | Pfad | Zweck |
|---|---|---|
| POST | `/api/login`, `/api/logout` | Sitzung |
| GET | `/api/model` | Dimensionen, Cubes, Ansichten (gefiltert nach Rechten) |
| GET | `/api/dimensions/{d}/members?parent=&search=&offset=&limit=` | Elemente seitenweise, Baum-Navigation, Suche |
| POST | `/api/dimensions` | Leere Dimension anlegen: `name`, `kind` (Modellierer) |
| PATCH | `/api/dimensions/{d}` | Elementänderungen (Modellierer) |
| POST | `/api/dimensions/{d}/rename` | Dimension umbenennen, nur wenn kein Cube sie nutzt (Modellierer) |
| POST | `/api/dimensions/{d}/import` | Elemente aus CSV-Text: `format` (`parent-child`, `levels`), `delimiter`, `mode` (`add`, `sync`), `dry_run` (Modellierer) |
| DELETE | `/api/dimensions/{d}` | Dimension löschen, nur wenn kein Cube sie nutzt (Modellierer) |
| POST | `/api/cubes/{c}/view` | Pivot berechnen: Achsen mit Aufklappzustand, Filter, Nullunterdrückung, Ausschnitt → Werte + Zellmetadaten (Basis/Aggregat/Regel, beschreibbar, Format) + Generation |
| POST | `/api/cubes/{c}/write` | Einzelwert oder Liste (Splash-Syntax), mit erwarteter Generation |
| POST | `/api/cubes/{c}/undo` | eigene letzte Operation zurücknehmen |
| GET/PUT | `/api/cubes/{c}/rules` | Regeltext lesen/speichern mit Syntaxprüfung |
| POST | `/api/imports` | Datei + Spezifikation, Vorschau oder Ausführung (Hintergrundjob) |
| GET | `/api/jobs/{id}` | Fortschritt und Protokoll |
| GET | `/api/cubes/{c}/export` | CSV/xlsx |
| GET | `/api/journal?cube=&user=&since=` | Änderungsjournal |
| GET | `/api/events` | Änderungsbenachrichtigungen (SSE) |
| … | `/api/admin/users`, `/api/admin/grants`, `/api/admin/backup` | Verwaltung |

API-Version im Pfad-Präfix ab dem ersten Release (`/api/v1/...`), damit Python-Clients und spätere Excel-Anbindung stabil bleiben.

### 6.4 Betrieb mit Docker Compose

Ziel: Auf jedem Rechner mit Docker (Linux-Server, Mac mit Docker Desktop/OrbStack/Colima, Windows) startet NeOlap mit `docker compose up -d`.

| Baustein | Umsetzung |
|---|---|
| Image | Mehrstufiges `Dockerfile`: Build in `rust:1.99-slim-trixie`, Laufzeit `gcr.io/distroless/cc-debian13:nonroot` (nur glibc/libgcc, keine Shell, läuft als Nicht-root-Benutzer 65532). Erwartete Größe ca. 25–30 MB. |
| Architekturen | linux/amd64 und linux/arm64 (Apple Silicon, ARM-Server). Lokal baut Docker nativ für die eigene Architektur. |
| Daten | Benanntes Volume `neolap-data` → `/data` (Datenbanken, Journal, Backups). Überlebt `down`, `up --build` und Updates. |
| Konfiguration | `.env` (Vorlage `.env.example`): Port, Bind-Adresse, Speicherlimit, Log-Level, optional fertiges Image aus der Registry. |
| Netz | Standard: Port nur an `127.0.0.1` gebunden. Für Teamzugriff Profil `tls`: Caddy als Reverse Proxy mit automatischem HTTPS (Let's Encrypt bei öffentlichem Namen, sonst lokale CA); SSE ungepuffert. |
| Gesundheit | `HEALTHCHECK` über `neolap healthcheck` (eingebaut, da das Image weder Shell noch curl hat); Caddy startet erst, wenn NeOlap gesund ist. |
| Stoppen | `docker stop` sendet SIGTERM; NeOlap beendet offene Anfragen und (ab M5) laufende Transaktionen sauber, `stop_grace_period` 30 s. |
| Härtung | `read_only`-Dateisystem, `cap_drop: ALL`, `no-new-privileges`, `tmpfs` für `/tmp`, Speicherlimit (Standard 4 GB für 20 Mio. Zellen). |
| Backup | Ab M5: `docker compose exec neolap neolap backup …` bzw. zeitgesteuertes Backup im Server in `/data/backup`; Sicherung des Volumes per `docker run --rm -v neolap_neolap-data:/data …`. |
| Verteilung | GitHub Actions baut und testet das Image bei jedem Push auf `main` (Start, Health, sauberer Stopp). Bei Versions-Tags `v*` wird ein Multi-Arch-Image nach `ghcr.io/langdar2/neolap` veröffentlicht, anonym abrufbar (für `v1.1.0` geprüft); Nutzung über `NEOLAP_IMAGE` in `.env`, eine Anmeldung bei `ghcr.io` ist dafür nicht nötig. |
| Updates | `docker compose pull && docker compose up -d` (Registry) bzw. `git pull && docker compose up -d --build`; Schema-Migrationen laufen beim Start automatisch (ab M5), vorher automatisches Backup. |

## 7. Web-Oberfläche (ui/)

- Pivot-Raster: Dimensionen per Drag & Drop auf Zeilen, Spalten, Filter; Hierarchien aufklappen (ebenenweise oder „alle Basiselemente“); Nullunterdrückung; virtuelles Scrollen; Zahlenformate aus dem Modell; Filterauswahl mit Suche (unverzichtbar bei 500.000 Elementen).
- Eingabe direkt in der Zelle inklusive Splash-Syntax; beschreibbare, gesperrte und Regelzellen farblich unterschieden; Tastaturnavigation wie in Excel; Bereich aus Excel einfügen; Strg+Z = Undo; Konfliktdialog; Live-Aktualisierung bei Änderungen anderer.
- Ansichten unter Namen speichern, privat oder für alle.
- Dimensionseditor: Baum mit Lazy Loading und Suche, Hinzufügen, Umbenennen, Verschieben, Gewichte, Attribute.
- Dimensionsverwaltung (Modellierer): neue Dimension anlegen, Dimension umbenennen oder löschen (beides nur, solange kein Cube sie nutzt; sonst sind die Knöpfe gesperrt), Elemente aus CSV importieren mit Vorschau vor dem Ausführen. Der Elementzähler im Auswahlfeld bleibt nach Änderungen aktuell; benennt oder löscht jemand anderes die geöffnete Dimension, baut sich die Seite neu auf.
- Regeleditor mit Zeilennummern und Fehlermarkierung.
- Importseite: Datei hochladen, Spalten zuordnen, Vorschau mit Protokoll, ausführen, Spezifikation speichern.
- Journal-Ansicht: Wer hat wann was geändert, Sprung zur Zelle.
- Verwaltung: Benutzer, Rollen, Bereichsrechte, Backup.

Technik: reines HTML/CSS/JavaScript als ES-Module, ohne Node-Build, eingebettet ins Binary. Das Raster ist ein eigenständiges Modul mit Canvas- oder DOM-Recycling-Rendering, damit große Ansichten flüssig bleiben.

## 8. Kommandozeile (neolap-cli)

```
neolap new plan.neolap
neolap demo tiny plan.neolap
neolap user add plan.neolap dirk --role admin
neolap dim import plan.neolap products produkte.csv --format parent-child [--sync]
neolap load plan.neolap import/ist_2026.toml [--dry-run]
neolap get  plan.neolap sales 2026 Q1 Total cars Sales
neolap set  plan.neolap sales 2027 Year Total Total Sales 1200000 [--mode equal]
neolap export plan.neolap sales --area "years=2026,version=Actual" -o ist.xlsx
neolap rules check|apply plan.neolap sales rules/sales.rules
neolap model export plan.neolap ./modell/
neolap backup plan.neolap backup/plan-2026-11-03.neolap
neolap serve plan.neolap --listen 0.0.0.0:8443 --tls cert.pem key.pem
```

Alle Daten-Befehle funktionieren lokal (Dateipfad) oder gegen einen laufenden Server (`--server https://… --token …`). Ausgaben als Tabelle oder `--json`.

## 9. Python-Modul (neolap-py)

maturin, abi3-Wheels für Python ≥ 3.9 (macOS arm64/x86_64, Linux, Windows).

```python
import neolap

db = neolap.open("plan.neolap")                                   # lokal, exklusiv
db = neolap.connect("https://olap.intern:8443", token="…")        # als Client eines Servers

cube = db.cube("sales")
cube["2026", "Jan", "North", "sedan", "Sales"] = 100.0
cube["2027", "Q1", "Total", "cars", "Sales"] = 5000              # Splash proportional
cube.write(("2027", "Q1", "Total", "cars", "Sales"), "#5000")    # Splash gleichmäßig
df = cube.area(years="2026", version="Actual").to_pandas()
cube.load(df, mapping={...}, mode="replace_area")
```

Lokaler und Server-Modus haben dieselbe API. Lese- und Schreibzugriffe geben den GIL frei; große Bereiche werden spaltenweise übergeben (schnell nach pandas).

**Stand der Umsetzung (M10):** Die API wie oben, mit zwei Abweichungen vom Beispiel: `cube.load` braucht zusätzlich zur Spaltenzuordnung `mapping` entweder `values={Element: Spalte}` (eine Spalte je Kennzahl) oder `value_column=` (lange Tabelle), dazu optional `fixed={Dimension: Element}`; und **polars wird nicht unterstützt**, nur pandas (`to_pandas()`, `load(df)`). Alle Ausnahmen erben von `neolap.NeolapError` und sind lokal und remote dieselben. Beschreibung und Begründungen: `docs/entscheidungen.md` E72–E82. Beispiel: `crates/neolap-py/examples/tiny.ipynb` (läuft als Test in beiden Modi).

## 10. Meilensteine

Grobe Schätzung in Personentagen. Jeder Meilenstein endet mit lauffähigem, getestetem Stand und Abnahme, bevor der nächste beginnt.

| # | Meilenstein | Inhalt | Abnahmekriterium | PT |
|---|---|---|---|---|
| M0 | Gerüst | Workspace, CI, Lizenz, README, Plan, Health-Endpunkt, Python-Modul-Hülle, Docker Compose | `cargo test` grün; `neolap serve` zeigt Startseite; Wheel baut; `docker compose up` startet gesund | 1,5 |
| M1 | Dimensionen | Elemente, Hierarchien, Gewichte, Aliase, Attribute, Subsets, Editor, LeafSets, Suche, Dimensionsimport | Portierte Tests grün; 500.000 Elemente / 50 Ebenen: Import < 5 s, Commit < 1 s | 4 |
| M2 | Abfrage | FactStore, Basis-Index, Selektivitätsplanung, gewichtete Aggregation, Gruppierungspfad, Cache | Golden-Test „tiny“ = Original; Property-Test Aggregat = Σ Basis × Gewicht; Benchmark 10 Mio. Zellen × 20 Dim. erfüllt Ziele | 6 |
| M3 | Rückschreiben | Basis-Schreiben, Splash-Modi, Schutzregeln, Undo, Zellversionen | Property-Tests: Zielwert exakt, Proportionen erhalten, Undo stellt Zustand her | 3 |
| M4 | Regeln | Lexer, Parser, Compiler, Scopes, Feeder, Funktionen, Fehlerwerte | Regeln der Beispielmodelle übersetzt, Ergebnisse = Original | 5 |
| M5 | Persistenz + Journal | SQLite-Format, Laden, Migrationen, Journal, Backup, Modell-Export | Roundtrip-Test; Abbruch mitten im Import hinterlässt konsistente Datei; Start < 20 s bei 10 Mio. Zellen | 3 |
| M6 | Import/Export | CSV/xlsx, Spezifikationen, Modi, Protokoll | 1 Mio. Zeilen < 10 s; zweimal `replace_area` = einmal | 3 |
| M7 | CLI (lokal) | Befehle aus Kap. 8 | End-to-End-Skript läuft durch | 2 |
| M8 | Server | REST, Anmeldung, Rollen, Bereichsrechte, Konflikte, SSE, Hintergrundjobs, TLS | API-Integrationstests inkl. Rechte- und Konfliktfällen; Lasttest 20 simulierte Nutzer | 7 |
| M9 | Web-UI | Raster mit Eingabe, Editoren, Import, Journal, Verwaltung | Abnahme-Checkliste am Mac ([docs/Abnahme-M9.md](Abnahme-M9.md)); Ansicht 50 × 20 < 500 ms | 9 |
| M10 | Python + CLI-Client | Bindings lokal und remote, Wheels, pandas, Notebook-Beispiele | `pip install` auf macOS arm64; Notebook „tiny“ läuft lokal und gegen Server | 3 |
| M11 | Härtung + Betrieb | Benchmarks mit 20 Mio. Zellen, Optimierung, Backup/Update-Ablauf im Container, Handbuch, Release-Binaries und -Images | Zielwerte aus Kap. 1 erreicht, auch im Container mit 4 GB Limit; Release 1.0 | 4 |

Summe ca. 50 Personentage. Zwischenstände zum Ausprobieren: nach M3 (Engine mit Splashing), nach M7 (vollständig per Kommandozeile), nach M9 (gemeinsames Arbeiten im Browser).

## 11. Teststrategie

- Portierte Unit-Tests aus dem Original, soweit fachlich relevant.
- Differenztests gegen das (gepatchte) Python-Original für die Beispielmodelle: identische Basiswerte laden, jede Zelle auf 1e-9 genau vergleichen (Hierarchien mit widersprüchlichen Pfadgewichten ausgenommen).
- Property-Tests (proptest) für Aggregation, Splashing, Undo, Persistenz-Roundtrip, Import-Idempotenz, Rechteprüfung.
- Generator für große synthetische Modelle (20 Dimensionen, 500.000 Elemente, unbalancierte Bäume bis Tiefe 50, Mehrfach-Eltern) für Benchmarks und Stresstests.
- Snapshot-Tests (insta) für Regel-Fehlermeldungen und API-Antworten.
- Nebenläufigkeitstests: parallele Leser/Schreiber, Konflikt- und Undo-Fälle.
- Lasttest des Servers mit 20 simulierten Nutzern (Ansichten + Schreiben).
- UI: Abnahme-Checkliste mit Screenshots; automatisierte Browsertests bei Bedarf.

## 12. CI und Auslieferung

- GitHub Actions im öffentlichen Repository: Auf jedem Push, bei Pull Requests auf `main`, wöchentlich und manuell laufen Linux (fmt, clippy, test), macOS und Windows. Die frühere Beschränkung von macOS und Windows auf Pull Requests ging von einem privaten Repository aus (macOS-Minuten zählen dort zehnfach); sie ist seit 13.11.2026 aufgehoben, nachdem ein Windows-Fehler (`export_is_readable_and_deterministic`, behoben in `28dccad`) bei einem Push unbemerkt blieb. Ob die Minuten bei diesem öffentlichen Repository tatsächlich nicht zählen, ist nicht geprüft.
- Docker-Workflow (`.github/workflows/docker.yml`): baut das Image, startet es mit Compose, prüft Health, Web-UI und sauberes Herunterfahren; bei Tags `v*` Multi-Arch-Veröffentlichung nach `ghcr.io/langdar2/neolap`.
- Release-Builds: macOS Universal Binary (arm64 + x86_64), Linux (x86_64, arm64), Windows; Docker-Image; Python-Wheels über `maturin-action`.
- Gatekeeper: Ohne Apple-Entwicklerzertifikat blockiert macOS heruntergeladene Binaries; Alternativen: lokal bauen (`cargo install --path crates/neolap-cli`), einmalig `xattr -d com.apple.quarantine neolap`, oder später signieren und notarisieren.

## 13. Risiken und Gegenmaßnahmen

| Risiko | Gegenmaßnahme |
|---|---|
| Speicherbedarf bei 20 Dimensionen und vielen Zellen | Nur Basis-Index, Spalten mit u32, Messung in M2; Bit-Packing als Reserve; Zellzahl-Annahme früh bestätigen |
| Abfragen über breite Aggregate in tiefen, unbalancierten Bäumen | DFS-Nummerierung (Bereichsprüfung), Selektivitätsplanung, Gruppierungspfad für Ansichten, Cache |
| Splashing „Gleich“ erzeugt Millionen Zellen | Zellgrenze, Vorschau der betroffenen Zellzahl |
| Gleichzeitige Änderungen überschreiben sich | Generationen/Zellversionen, Konfliktdialog, Journal, Undo nur ohne Fremdänderung |
| Rechteprüfung bremst große Splashes/Importe | Rechte als Bitmengen je Dimension vorkompiliert, Prüfung pro Dimension statt pro Zelle |
| N-Regeln mit Feedern schwer verständlich | Feeder-Ableitung als Standard, Warnungen, Beispiele |
| Web-UI ohne Framework wird aufwendig | Raster zuerst als eigenständiges Modul, übrige Seiten schlicht; Wechsel auf ein kleines Framework ohne Build (z. B. Preact) bleibt möglich |
| Sicherheit im Netzbetrieb | Standard nur localhost, TLS Pflicht im Netz, Argon2, Rate-Limit beim Login, keine Standardpasswörter |

## 14. Offene Punkte

1. Wie viele Nutzer gleichzeitig, und reichen Schreibrechte auf Elementebene oder werden auch Leserechte auf Elementebene schon in v1 gebraucht?

Geklärt: max. 20 Mio. gefüllte Basiszellen (03.11.2026); Server läuft überall per Docker Compose (03.11.2026).

## 15. Phase 2

Excel-Anbindung über ein Office.js-Add-in (Mac, Windows, Browser) mit Funktionen wie `=NEOLAP.GET(…)` und Rückschreiben gegen die REST-API; Firmen-SSO (OIDC); Leserechte auf Elementebene; Historie mit Zeitpunkt-Abfragen; Parquet-Import; Textzellen und Kommentare; Regel-Debugger („woher kommt dieser Wert?“).
