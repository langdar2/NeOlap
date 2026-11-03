# Architekturentscheidungen

Kurzprotokoll der wesentlichen Entscheidungen. Neue Einträge oben anfügen.

## 2026-11-03 – Import und Export (M6)

| # | Entscheidung | Begründung |
|---|---|---|
| E34 | `replace_area` löscht je Dimension genau die Elemente, die in der Datei mit einem Wert vorkommen (Produkt der Elementmengen), nicht die Hüllmenge aller Zeilen | Plan 4.6 „genau den Bereich, den die Datei abdeckt". Eine leere Wertespalte (z. B. `Kosten` überall leer) gehört nicht zum Bereich; mit der Wiederholbarkeit (zweimal = einmal) per Property-Test belegt |
| E35 | Ein Import ist eine Operation (`Cube::bulk_write`): Rechte, Regelsperre und Konflikte werden für alle Änderungen vorab geprüft, dann wird ganz oder gar nicht geschrieben; ein Undo-Schritt nimmt alles zurück | Plan 4.6: „Jeder Import ist eine Transaktion, prüft Schreibrechte, landet im Journal, ist per Undo rücknehmbar" |
| E36 | `unknown_member = "reject"` lehnt den ganzen Import ab (Protokoll listet bis zu 200 Zeilen mit Grund); `skip` überspringt und zählt; `create_under:<Element>` legt fehlende Elemente an, vor dem Schreiben der Zellen und zuerst in der Datei | Reihenfolge verhindert, dass nach einem Abbruch Fakten auf nicht gespeicherte Elemente zeigen |
| E37 | `create_under` lehnt ein Elternelement ab, das selbst Zellen hält | Es würde zum Aggregat, seine Zellen gingen verloren; der Nutzer soll ein Aggregat oder ein leeres Element nennen |
| E38 | Es werden nur Basiselemente geladen; Aggregate in der Datei sind abgelehnte Zeilen | Aggregate werden zur Abfragezeit berechnet |
| E39 | Leere Wertezelle = keine Zelle (nicht 0); Zahlen mit `decimal = ","` akzeptieren Tausenderpunkte, mit `"."` Tausenderkommas; mehrfach vorkommende Zellen einer Datei werden summiert | Wie im Plan („Schreiben von 0 oder leer löscht"); Summieren entspricht dem breiten und langen Format ohne Eindeutigkeit |
| E40 | Spezifikation: Langformat über `value_column` (der Plan zeigt nur das breite Format); Wertedimension standardmäßig die einzige vom Typ Measures, sonst `options.value_dimension`; `deny_unknown_fields` meldet Tippfehler | Der Plan nennt „lang oder breit" für den Export; der Import soll dasselbe können |
| E41 | Import-Spezifikationen liegen als TOML-Text in der Tabelle `import_specs` (Schema-Version 2); die erste echte Migration (v1→v2) legt dabei `<Datei>.before-v2` an | Plan 4.6 „im Modell speicherbar"; damit ist der Migrationsablauf aus E31 erstmals an einer echten Migration geprüft |
| E42 | Export rechnet wahlweise mit Regeln (`with_rules`), lässt leere Zellen/Zeilen weg und begrenzt sich auf 50 Mio. Zellen; xlsx nur bis 1.048.575 Zeilen | Schutz vor versehentlich riesigen Dateien; für mehr gibt es CSV |

## 2026-11-03 – Persistenz und Journal (M5)

| # | Entscheidung | Begründung |
|---|---|---|
| E27 | Zellen werden unter `MemberId`-Schlüsseln gespeichert (4 Byte je Dimension), nicht unter DFS-Ordinalen; der Cube rechnet seine Spalten nach jeder Dimensionsänderung neu (`replace_dimension`) | Ordinale ändern sich bei jedem Commit einer Dimension; MemberIds bleiben stabil, solange ein freigewordener Platz erst nach dem Commit wiederverwendet wird (Editor) |
| E28 | Schreiben ist Write-through: Der Cube übergibt die Operation zuerst an einen `OpSink`, der sie in einer SQLite-Transaktion (Fakten + Journal) speichert; erst danach ändert sich der Speicher. Lehnt der Sink ab, ändert sich nichts | Einzige Möglichkeit, dass „Abbruch mitten im Import eine konsistente Datei hinterlässt" (Abnahme M5) und Datei und Speicher nie auseinanderlaufen |
| E29 | Kein Binär-Snapshot beim Start | Plan 4.5 macht ihn von der Messung abhängig: 10 Mio. Zellen laden in 2,2 s (Ziel < 20 s) |
| E30 | Exklusiver Zugriff über eine Sperrdatei (`<datei>.lock`, `File::try_lock`); Rust-Mindestversion 1.89 | Plan 4.5/E4: lokaler Modus exklusiv. `try_lock` gibt es seit Rust 1.89; das Docker-Image baut ohnehin mit 1.99 |
| E31 | Vor einer Schemamigration wird die Datei per `VACUUM INTO` nach `<datei>.before-v<N>` kopiert | Plan 6.4: „vorher automatisches Backup". Seit M6 (Schema 2) durch eine echte Migration getestet (E41) |
| E32 | Eine Dimensionsänderung verwirft Fakten entfernter Elemente und von Elementen, die zu Aggregaten wurden, und protokolliert die Zahl im Journal. Die Rückfrage und die Option „auf neues Kind `<Name> (Rest)` verschieben" (Plan 4.1) gehören in die Oberfläche | Die Engine kann nur ausführen; die Entscheidung trifft der Nutzer in M8/M9 |
| E33 | Der Aggregat-Cache wird weiterhin vollständig geleert; der Schreibpfad ist jetzt aber nicht mehr der Engpass (Splash mit 100.000 Zellen inkl. Journal: 0,22 s) | gemessen |

## 2026-11-03 – Regeln (M4)

| # | Entscheidung | Begründung |
|---|---|---|
| E21 | Syntaxfehler lehnen den ganzen Regeltext mit Zeile/Spalte ab; Regeln mit unbekanntem Element, unbekannter Dimension oder Funktion werden als ungültig markiert und übersprungen | Plan 4.4: „ungültige Regeln werden markiert, statt das Modell zu blockieren" – Modelländerungen dürfen Regeln nicht unbemerkt entwerten, die Statusliste (`RuleStatus`) meldet es dem Editor |
| E22 | Das Regelziel nennt Elemente immer wörtlich (`'x'` oder `DESC('x')`); berechnete Elementnamen (`dim:ausdruck`) gibt es nur rechts | Das Ziel muss in eine Mustertabelle übersetzbar sein (Zuordnung ohne Auswertung) |
| E23 | Regel ohne Präfix rechnet die Zelle auf jeder Ebene selbst (Aggregate werden nicht summiert); `N:` rechnet Basiszellen und Aggregate summieren die Ergebnisse; `C:` nur Aggregate | Entspricht ALL_LEVELS / BASE_LEVEL / AGGREGATION_LEVEL im Original |
| E24 | Ohne `FEED` gelten alle Zellbezüge der Formel als Feeder; Zellen, für die ein Feeder keinen gespeicherten Wert findet, entstehen nicht | Plan 13: „Feeder-Ableitung als Standard". Feeder mit berechneten Elementnamen können nicht umgekehrt werden und speisen nichts |
| E25 | Zellen, die eine Regel berechnet (oder deren Fläche eine Regelzelle enthält), sind für `write` gesperrt (`Denied` mit Regelzeile) | Plan 4.3: „Regelzellen sind gesperrt"; ein Splash über eine Regelzelle wäre nicht eindeutig |
| E26 | Leere Zellen zählen in Rechnungen als 0, `NULL` liefert eine leere Zelle; Vergleiche und Text sind unabhängig von Groß-/Kleinschreibung | Wie in TM1/Alea; Textwerte sind nur in Regelausdrücken möglich (keine Textzellen in Cubes, Plan Kap. 1) |

## 2026-11-03 – Rückschreiben (M3)

| # | Entscheidung | Begründung |
|---|---|---|
| E17 | Eingabe `-50` ist ein Delta; ein negativer Absolutwert wird als `=-50` geschrieben | Der Plan nennt `+50`/`-50` als Delta; ohne Präfix wäre ein negativer Zielwert sonst nicht eindeutig |
| E18 | Referenz-Splash `@Dim:Element,…:Wert` ordnet Basiselemente von Ziel und Referenz je genannter Dimension nach Position zu; gleiche Anzahl Pflicht | Die Plan-Syntax lässt offen, wie Referenz und Ziel zusammenpassen; Positionszuordnung ist eindeutig und prüfbar |
| E19 | Ein Schreibvorgang ohne tatsächliche Änderung erzeugt keine Operation (keine Generation, kein Undo-Eintrag) | Vermeidet Scheinkonflikte und leere Journaleinträge |
| E20 | Operationsprotokoll (alt/neu je Zelle) zunächst im Speicher, höchstens 1 Mio. Zelländerungen; ältere Operationen fallen heraus | Grundlage für Konflikte, Undo und Zellversionen in M3; M5 persistiert es als Journal. Ansichten, die älter als das Protokoll sind, bekommen die Meldung „bitte neu laden“ |

## 2026-11-03 – Speicherdesign (M2)

| # | Entscheidung | Begründung |
|---|---|---|
| E15 | Kein Roaring-Zeilenindex je Basiselement; stattdessen Zeilenzähler und paralleler Spaltenscan | Gemessen: Index 121 B/Zelle (mehr als alle Spalten), Prozessspeicher 10 Mio. Zellen 2,0 GB. Ohne Index 102 B/Zelle, 20 Mio. Zellen 2,4 GB; Abfragen mit 2–5 ms schneller statt langsamer, Laden 6× schneller. Ändert E3 nur im Detail: Index bleibt auf Basiselementen, nicht auf Aggregaten |
| E16 | Spaltenwachstum um 1/8 statt Verdoppeln | Verdoppeln kostete bis zu 50 B/Zelle Verschnitt |

## 2026-11-03 – Betrieb und Größenrahmen

| # | Entscheidung | Begründung |
|---|---|---|
| E9 | 20 Mio. gefüllte Basiszellen sind das feste Maximum | Vorgabe des Auftraggebers; erlaubt reines In-Memory-Design ohne Auslagerung, ≈ 3 GB RAM |
| E10 | Server-Betrieb überall per Docker Compose | Vorgabe des Auftraggebers; identischer Betrieb auf Mac, Linux-Server und Windows |
| E11 | Laufzeit-Image `distroless/cc-debian13:nonroot` | Klein, ohne Shell/Paketmanager (geringe Angriffsfläche), Nicht-root; Programm braucht nur glibc ≥ 2.34 und libgcc |
| E12 | Eingebauter Healthcheck `neolap healthcheck` | Distroless-Image hat kein curl/wget |
| E13 | TLS per optionalem Caddy-Profil statt eingebautem TLS als Standard | Automatische Zertifikate, kein Zertifikatsmanagement in NeOlap nötig; eingebautes TLS bleibt für Betrieb ohne Docker vorgesehen |
| E14 | Konfiguration im Container über `NEOLAP_*`-Umgebungsvariablen | Docker-üblich, in `.env` pflegbar |

## 2026-11-03 – Ausgangsentscheidungen

| # | Entscheidung | Begründung |
|---|---|---|
| E1 | Neuimplementierung in Rust statt Weiterentwicklung von TinyOlap (Python) | Original seit 05/2022 ohne Release, läuft nicht auf Python ≥ 3.10; Leistungsziele (20 Dim., 500.000 Elemente, Mehrbenutzer) mit Python nicht erreichbar |
| E2 | Regeln als eigene Formelsprache, keine Python-Funktionen | Portabel, mit dem Modell speicherbar, schnell, unabhängig von Python |
| E3 | Index nur auf Basiselementen, Aggregate über DFS-Bereiche/Bitmengen | Original indiziert jede Zeile für jeden Vorfahren – bei 20 Dim. × 50 Ebenen bis zu 1.000 Indexeinträge je Zelle |
| E4 | Gemeinsame Nutzung ab v1 über einen Server-Prozess; lokaler Modus exklusiv | Einfaches, konsistentes Nebenläufigkeitsmodell ohne verteilte Sperren |
| E5 | SQLite als Speicherformat, Journal in derselben Datei | Eine Datei, transaktionssicher, Online-Backup, keine Installation |
| E6 | Web-UI ohne Build-Schritt, ins Binary eingebettet | Einfache Auslieferung als einzelne Datei, keine Node-Toolchain nötig |
| E7 | Widersprüchliche Pfadgewichte zum selben Basiselement werden abgelehnt | Original liefert dafür ein zufallsabhängiges Ergebnis |
| E8 | Excel-Anbindung erst in Phase 2 | Entscheidung des Auftraggebers: erst wenn v1 stabil läuft |
