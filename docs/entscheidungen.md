# Architekturentscheidungen

Kurzprotokoll der wesentlichen Entscheidungen. Neue Einträge oben anfügen.

## 2026-11-13 – Dimensionsverwaltung (1.1.0)

| # | Entscheidung | Begründung |
|---|---|---|
| E89 | Umbenennen und Löschen einer Dimension sind nur erlaubt, solange kein Cube sie nutzt; sonst Konflikt (HTTP 409) und gesperrte Knöpfe in der UI | Cube-Dateien und Regeln verweisen auf Dimensionen über den Namen; ein Umbenennen würde sie stillschweigend brechen, ein Löschen Zellen verwaisen lassen |
| E90 | Modellierer sehen in `/model` auch Dimensionen, die kein Cube nutzt, mit dem Feld `used`; andere Rollen nicht | Eine neu angelegte Dimension wäre sonst bis zum ersten Cube unsichtbar und nicht zu befüllen |
| E91 | Der Dimensionsimport läuft in der UI immer zuerst als Vorschau (`dry_run`) und braucht eine Bestätigung. Im Modus „Abgleichen“ nennt die Vorschau die Elemente, die entfernt würden | Der Import kann Elemente und damit Zellwerte in allen Cubes löschen |
| E92 | Die Seite „Dimensionen“ ignoriert das `model`-Ereignis zu einer eigenen Umbenennung oder Löschung und baut sich selbst neu auf. Bei einer fremden Änderung lädt sie das Modell neu und wechselt auf die vorhandenen Dimensionen, wenn die geöffnete fehlt | Sonst fragte die Seite die Elemente unter dem alten Namen ab (404 in der Konsole) |

## 2026-11-04 – Härtung und Betrieb (M11)

| # | Entscheidung | Begründung |
|---|---|---|
| E83 | Zielwerte werden an einem synthetischen Großmodell gemessen (`synth_targets`): zwei Dimensionen mit 500.000 Elementen und 50 Ebenen, vier mit 20.000, vierzehn kleine, Mehrfach-Eltern, negative Gewichte, feste Zufallszahlen | Alle früheren Benchmarks nutzten Dimensionen mit 1.000 Blättern und zwei Ebenen und belegten die Zielwerte daher nicht |
| E84 | Der ungünstigste Fall für Aggregate sind große Elementmengen (Wurzel, obere Ebenen) auf mehreren Dimensionen zugleich, nicht zufällige Teilbäume | Zufällige Teilbäume sind klein und messen nur 3 ms; ein Zielwert, der nur dafür gilt, wäre wertlos |
| E85 | Zielwerte des Plans haben keine Referenzhardware. Gemessen auf M4 Pro (14 Kerne) und auf einem GitHub-Runner (2 Kerne, 4 GB). Auf 14 Kernen sind alle Ziele erreicht; auf 2 Kernen liegt der ungünstigste kalte Aggregatwert bei 20 Mio. Zellen über 1 s | Offen: Auftraggeber legt Referenzhardware fest, oder das Handbuch empfiehlt mindestens 4 Kerne (so derzeit) |
| E86 | Der Container-Benchmark läuft als Workflow `bench.yml` (manuell und wöchentlich), nicht bei jedem Push | Dauert etwa 5 Minuten und misst Zeiten, die auf geteilten Runnern schwanken; ein fester Grenzwert würde die CI unzuverlässig machen |
| E87 | Zeitgesteuerte Sicherung im Server: `backup/auto-<Zeit>.neolap`, Standard alle 24 h, 7 behalten (`NEOLAP_BACKUP_INTERVAL_HOURS`, `NEOLAP_BACKUP_KEEP`, 0 = aus). Löscht nur eigene `auto-`-Dateien, nie die von Hand erzeugten `backup-` | Ein Betrieb ohne Sicherung ist keiner; Handsicherungen dürfen nie automatisch verschwinden |
| E88 | Die erste zeitgesteuerte Sicherung entsteht nach einem Intervall, nicht beim Start | Ein Neustart-Zyklus kürzer als das Intervall verhindert sie; im Handbuch beschrieben, nicht verdeckt |

## 2026-11-04 – Python-Modul (M10)

| # | Entscheidung | Begründung |
|---|---|---|
| E72 | **API-Tokens werden als SHA-256 gespeichert, nicht mehr als Argon2.** Alte Tokens (Argon2) gelten weiter und werden beim ersten erfolgreichen Gebrauch hochgestuft (best effort: ein Schreibfehler macht den Token nicht ungültig). Ein unbekannter Token rechnet denselben Hash wie ein bekannter. Argon2 bleibt für Passwörter | Ein Token ist ein zufälliges 192-Bit-Geheimnis, das nicht geraten wird; Argon2 bremst dort nur. Gemessen: jede Anfrage mit Token kostete 210 ms, ein Skript mit 1.000 Lesezugriffen über 3 Minuten; jetzt 0,95 ms. Bewusst hingenommen: Die Antwortzeit verrät, ob zu einer (zufälligen, 48-Bit-)Id noch ein alter Argon2-Eintrag existiert |
| E73 | **`Cube::value` liest Basiszellen mit einem Hash-Zugriff** statt über Selektivitätsplanung und Spaltenscan und legt sie nicht in den Cache | Der Export von 120.000 Basiszellen dauerte 172 s (je Zelle ein Scan über alle Zeilen), jetzt 0,24 s. Der Fehler stammte aus M2/M6 und betraf auch `GET /export` der Web-Oberfläche. Property-Test: Schnellpfad = Scan für jede Basiszelle zufälliger Cubes |
| E74 | Das Python-Paket besteht aus einer **dünnen Rust-Schicht** (`neolap._native`: lokaler Zugriff, Fehlerabbildung, GIL-Freigabe, Spaltenübergabe) und **Python für die gemeinsame API** mit zwei Backends (Datei, HTTP). Dieselben Tests laufen gegen beide | „Lokaler und Server-Modus haben dieselbe API" (Plan Kap. 9) ist so keine Zusage, sondern wird bei jedem Lauf geprüft. Das fand vier Unterschiede: `rules=False` wurde im Remote-Modus ignoriert, die Kategorienreihenfolge, die Spaltenart leerer Kennzahlen und der Wert von `NaN` |
| E75 | Spaltenart und Kategorienreihenfolge einer Tabelle kommen aus der **Position und der Reihenfolge des Auftretens**, nie aus dem Inhalt (`ExportTable::n_name_columns`). Ein Element `2026`, `007` oder `1e3` bleibt Text, eine überall leere Kennzahl bleibt eine Zahlenspalte, Monate bleiben in Modellreihenfolge statt alphabetisch | Raten nach dem Inhalt machte aus Jahreszahlen Zahlen; pandas sortiert Kategorien sonst alphabetisch (Apr, Aug, Dec …) |
| E76 | Zahlen gehen **ohne Exponent** an den Server (`1e-07` → `0.0000001`), verlustfrei über `Decimal(repr(x))`; negative Zahlen mit `=` als Absolutwert | Der Eingabeleser kennt keine Exponentialschreibweise; `repr(1e21)` war nicht schreibbar. Test: sieben Grenzfälle bitgenau in beiden Modi |
| E77 | `GET …/cell?opt.rules=false` liest ohne Regeln; Optionen tragen das Präfix `opt.` | Der Remote-Client ignorierte `rules=False` stillschweigend. Ein unpräfixierter Name könnte mit einer Dimension gleichen Namens kollidieren |
| E78 | Der Remote-Client nutzt **nur die Standardbibliothek** und übersetzt jeden Transportfehler (abgeschnittene Antwort, kein JSON, Hangup, 5xx) in eine `neolap`-Ausnahme; Tokens mit Zeilenumbruch, NUL, Leerzeichen oder Nicht-ASCII werden beim Verbinden abgelehnt; ein Token geht nur über https (außer an localhost); es steht nie in `repr`, Fehlertext oder Log | Ein Skript soll `except neolap.NeolapError` schreiben können; Header-Injektion über einen manipulierten Token ist ausgeschlossen (getestet gegen einen mitschneidenden Server) |
| E79 | Alle Ausnahmen erben von `neolap.NeolapError`; `neolap.PermissionError` steht **nicht** in `__all__`, damit `from neolap import *` den eingebauten Namen nicht überdeckt. 429 ist `TooManyRequests`, 502/503/504 sind `ConnectionFailed` | Eine Anmeldesperre ist kein fehlendes Recht, und eine Überlastung ist kein Speicherfehler |
| E80 | `Database.close()` gibt die Datei wirklich frei (das `Database`-Objekt wird fallengelassen, damit die Sperrdatei frei wird) und ist wiederholbar; danach scheitert jeder Aufruf mit `InputError` statt abzustürzen | Ohne das blieb die Datei bis zum Prozessende gesperrt |
| E81 | Das **GIL wird bei jeder Engine-Arbeit freigegeben** (`py.detach`). Belegt: ein zweiter Thread macht mit Freigabe ~2.300 von ~3.000 Ticks, ohne sie 8 (Pause 225 ms). Ein Wächter über den Quelltext fordert die Freigabe für jede arbeitende Methode | Plan Kap. 9 |
| E82 | Wheels: abi3 für Python ≥ 3.9 (auf 3.9.6 und 3.13 getestet), ohne Abhängigkeiten; pandas ist optional (`neolap[pandas]`) und `to_pandas()` sagt, was fehlt. Gebaut und geprüft nur für macOS arm64; die CI-Matrix für macOS x86_64, Linux x86_64/aarch64 und Windows ist geschrieben, aber nie gelaufen | Plan Kap. 9/12 |

## 2026-11-03 – Web-Oberfläche (M9)

| # | Entscheidung | Begründung |
|---|---|---|
| E61 | **Zahlenformat deutsch:** ein einzelner Punkt vor genau drei Ziffern ist ein Tausendertrenner (`2.000` = 2000), das Komma ist immer das Dezimalzeichen, ein Punkt nach einer führenden Null oder nach mehr als drei Ziffern ist ein Dezimalpunkt (`0.500`, `1234.567`); mit beiden Zeichen ist das letzte das Dezimalzeichen. Mehrdeutiges (`1.5,5`, `1,,5`, `1.23.456`) wird abgelehnt | Beim Einfügen aus einer deutschen Tabelle wurde `2.000` stillschweigend zu 2 (der Kern las einen einzelnen Punkt als Dezimalpunkt und begründete das fälschlich mit der deutschen Lesart). **Preis:** `1.000` lässt sich nicht mehr als „eins Komma null null null" schreiben. Entscheidung des Auftraggebers. Der Parser hatte außerdem `1.5,5` still zu 15,5 gemacht (seit M3) |
| E62 | Die Zellvorprüfung der UI (`looksLikeInput`) ist **bewusst nie strenger als der Server**; den Vertrag hält `ui/tests/input-contract.tsv`, geprüft von einem Rust-Test gegen den echten Parser und von einem Node-Test gegen die UI | Eine von Hand nachgebaute Grammatik wich dreimal vom Server ab, einmal so, dass gültige Eingaben (`=-50`, `@dim:Element,…`) blockiert worden wären |
| E63 | Die UI baut ihr DOM nie aus Markup: `dom.js` kennt kein `innerHTML`, setzt Text nur per `textContent`, erlaubt nur Attribute einer Liste, verweigert `style` und `on*` mit Text und prüft Links (`javascript:`, `data:`, `//host` sind verboten). Ein Test durchsucht den gesamten UI-Quelltext nach `innerHTML`, `eval` u. ä. | Elementnamen, Aliase und Nutzernamen stammen von Nutzern; ein gespeichertes `<img onerror>` würde bei jedem Betrachter laufen. Belegt im Browser mit einem Element, dessen Name Markup ist; `dom.js` hält auch ohne CSP |
| E64 | Strikte CSP ohne `'unsafe-inline'`: `default-src 'self'; script-src 'self'; style-src 'self'; base-uri 'none'; object-src 'none'; form-action 'self'; frame-ancestors 'none'`. Das Raster positioniert Zellen per CSSOM (`el.style.transform`), nie über ein `style`-Attribut | Im Browser geprüft: unter `style-src 'self'` sind CSSOM-Zuweisungen erlaubt, Attribute blockiert |
| E65 | Die Aufklapp-Zustände der Achsen führt die UI selbst; sie holt Kinder mit `GET /dimensions/{d}/members?parent=` und schickt die sichtbaren Namen an `/view`. Es gibt keinen Server-Zustand je Ansicht | Der Plan nennt „Achsen mit Aufklappzustand" in der Anfrage; so bleibt der Server zustandslos |
| E66 | Vor einer Eingabe, die mehr als 1.000 Basiszellen erreicht, fragt die UI zurück (Zahl vom Server: `area` in `GET …/cell`, `base_cells` als Zeichenkette, da `u128`). Die Statuszeile zeigt die Reichweite schon bei der Auswahl. Lässt sich die Zahl nicht ermitteln, wird gefragt statt geschrieben | Plan 4.3 „die UI zeigt vorher die Zahl betroffener Zellen"; ein Splash auf eine Gesamtsumme trifft alle 60.000 Zellen eines Cubes |
| E67 | Konflikt: Der Dialog nennt Verursacher, Zelle, aktuellen Wert und die eigene Eingabe; der Fokus liegt auf der sicheren Schaltfläche „verwerfen"; „überschreiben" lädt die Ansicht neu und schreibt mit der neuen Generation | Ein versehentliches Enter darf nichts Zerstörerisches tun |
| E68 | `GET /session` meldet „nicht angemeldet" mit 200 (`user: null`); `/me` bleibt bei 401. Geht die Live-Verbindung verloren, prüft die UI die Sitzung: ist sie weg (Neustart), erscheint die Anmeldung; sonst verbindet sie neu und lädt jede offene Ansicht neu | Ein 401 beim Start erschien als roter Konsolenfehler; nach einem Serverneustart blieb die Seite auf „getrennt" stehen, bis die nächste Aktion scheiterte |
| E69 | **Beim Herunterfahren beendet der Server die Event-Streams** (`begin_shutdown`). Test: echter Prozess, offener Stream, SIGTERM, Ende < 5 s | Offene SSE-Verbindungen enden nie von selbst; ein einziger offener Browser-Tab hielt den Server nach SIGTERM am Leben (`docker stop` wäre nach 30 s mit SIGKILL geendet, ein laufender Import abgebrochen). Der CI-Schritt „Graceful shutdown" hätte das ohne Browser nicht gefunden |
| E70 | `ui/tests/` wird nicht eingebettet (`#[exclude = "tests/*"]`); ein Test prüft, dass Module als JavaScript ausgeliefert werden, Testdateien 404 liefern und `/../` nichts außerhalb erreicht | `rust-embed` bettet sonst den ganzen Ordner ein; belegt: `/tests/input-contract.tsv` wurde ausgeliefert |
| E71 | Die UI-Tests laufen mit Nodes eigenem Runner (`node --test ui/tests/*.test.js`), ohne Paketmanager, in der CI (Node 22, mit Node 22.18 und 26 geprüft) | Plan 4.6/E6: kein Node-Build |

## 2026-11-03 – Mehrbenutzer-Server (M8)

| # | Entscheidung | Begründung |
|---|---|---|
| E51 | **Lesen gilt je Cube, Schreiben je Zelle.** Wer einen Cube lesen darf, liest ihn vollständig (auch Aggregate); Bereichsrechte und `none` begrenzen nur das Schreiben. Ein Planer ohne Freigabe sieht den Cube nicht (Antwort „nicht gefunden", nicht „verboten"), Admin und Modellierer sind nur durch ihre Rolle begrenzt | Der Plan stellt Leserechte auf Elementebene ausdrücklich in Phase 2 (6.2). Ein Lesefilter müsste auch verhindern, dass Aggregate Verborgenes verraten (Summe minus sichtbarer Teil). Ein erster Entwurf baute einen solchen Filter; er ist verworfen, weil er nicht verlangt war und nicht sicher zu bekommen ist. **Einschränkung für Nutzer:** ein Bereichsrecht ist kein Datenschutz gegen Mitleser desselben Cubes |
| E52 | Rechte werden **an der zu ändernden Zelle** geprüft, nicht am angefragten Bereich: ein proportionaler Splash, der nur vorhandene Zellen trifft, wird erlaubt, wenn der Nutzer genau diese schreiben darf; ein Gleich-Splash erzeugt alle Kombinationen und wird abgelehnt, sobald eine fremde dabei ist. Die ganze Operation wird dann abgelehnt | Folgt aus „jede betroffene Basiszelle muss beschreibbar sein" (6.2); im Test belegt |
| E53 | Anmeldung: Sitzungs-Cookie (`HttpOnly`, `SameSite=Strict`, hinter TLS `Secure`) mit CSRF-Token für ändernde Anfragen; Bearer-Token braucht kein CSRF-Token (wird nicht automatisch mitgeschickt). Sitzungen liegen im Speicher und enden mit einem Neustart, nach Passwort-/Rollenwechsel und Löschen des Nutzers | Plan 6.1. Sitzungen nicht zu speichern ist bewusst einfach; wer neu startet, meldet sich neu an |
| E54 | Anmeldesperre: 5 Fehlversuche in 5 Minuten je Benutzername **und** je Client; die Sperre gilt auch für das richtige Passwort. Hinter dem Proxy zählt der **letzte** Eintrag von `X-Forwarded-For` (den der eigene Proxy setzt), nur mit `NEOLAP_TRUST_PROXY` | Der erste Eintrag ist vom Client frei wählbar und würde die Sperre je Client aushebeln (getestet). Ein Dienstangriff auf einen Namen kann diesen Nutzer sperren: bewusst in Kauf genommen |
| E55 | **Erster Administrator** über `NEOLAP_ADMIN_USER`/`NEOLAP_ADMIN_PASSWORD` beim ersten Start; leere Werte gelten als nicht gesetzt (so reicht Compose `${VAR:-}` durch), nur eines von beiden oder ein zu kurzes Passwort stoppen den Start mit Meldung; ohne beide startet der Server und warnt | Ohne Benutzer ließe sich der Docker-Betrieb nicht nutzen (das Image hat keine Shell, die CLI kann die gesperrte Datei nicht öffnen). Ein erster Entwurf mit Clap-`requires` ließ den Server **nie** starten; erst der Test mit der echten Binary zeigte das |
| E56 | Import als Hintergrundjob in zwei Schritten: Einlesen, Auflösen und Prüfen (`prepare_import`, nur lesend) läuft unter der **Lesesperre**, nur das Schreiben (`apply_prepared`) unter der Schreibsperre. Ändert sich das Modell dazwischen, wird abgelehnt („bitte erneut starten"); Zelländerungen erkennt die erwartete Generation. Importe mit `create_under` (ändern das Modell) und Vorschauen laufen in einem Schritt unter der Schreibsperre | Plan 4.6/5: Leser sollen währenddessen den alten Stand sehen. Gemessen: Das Schreiben in den Speicher kostet ~0,1 s von 1,4 s, der Rest ist Lesen. Das Schreiben in die Datei (Journal) bleibt unter der Schreibsperre |
| E57 | Uploads bis 256 MB, übrige Anfragen bis 2 MB; Importe mit `create_under` nur für Modellierer, Importe allgemein ab Planer | Schutz vor Speichererschöpfung; neue Elemente sind eine Modelländerung |
| E58 | Die Oberfläche fragt die Engine, ob eine Zelle schreibbar ist (`Cube::is_writable`), statt deren Regeln im Server nachzubauen | Ein Test zeigte, dass Aggregate mit Gewichten als schreibbar angeboten und dann abgelehnt würden |
| E59 | Ereignisse (SSE) gehen nur für Cubes, die der Empfänger lesen darf; wer mehr als 1.024 Ereignisse zurückliegt, bekommt `resync` und lädt die Ansicht neu | Verhindert, dass ein langsamer Client den Speicher füllt |
| E60 | Einschränkungen des Hintergrundbetriebs: Jobs liegen im Speicher (nach einem Neustart weg); beim Herunterfahren wartet der Server bis zu 30 s auf laufende Jobs | Ein Import ist eine Transaktion: er geschieht ganz oder gar nicht |

## 2026-11-03 – Kommandozeile, Modelldateien, Benutzer (M7)

| # | Entscheidung | Begründung |
|---|---|---|
| E43 | Die CLI-Befehle liegen in einer Bibliothek (`neolap_cli::run`), die Binary ergänzt nur `serve`/`healthcheck` und die Exit-Codes | Ende-zu-Ende-Tests laufen gegen die echte Binary und gegen `run`; der Docker-Betrieb (E11–E14) bleibt unverändert und ist nachgemessen |
| E44 | Passwörter nie als Argument: `NEOLAP_PASSWORD`, Standardeingabe oder interaktive Abfrage mit Wiederholung; API-Token geht auf stdout, die Warnung auf stderr | Argumente stehen in der Prozessliste und der Shell-Historie |
| E45 | Benutzer, Rollen und API-Tokens liegen in der Datei (Schema 3, Argon2id); Tokens `nlp_<id>_<geheimnis>` werden einmal gezeigt und nur als Hash gespeichert; ein Passwortwechsel macht alle Tokens des Nutzers ungültig; der letzte Admin kann weder gelöscht noch herabgestuft werden; falscher Name und falsches Passwort sind nicht unterscheidbar (auch nicht über die Zeit) | Plan 6.1; die Verwaltung gehört in den Kern, damit Server (M8) und CLI dieselbe Logik nutzen. Zufall über `getrandom` (plattformübergreifend), nicht über `/dev/urandom` |
| E46 | Modell-Export: `dimensions/<n>.csv` (eine Zeile je Kante, Alias und Attribute in der ersten Zeile des Elements) + `<n>.toml` (Art, Subsets nach Namen), `cubes/`, `rules/`, `imports/`; deterministisch, entfernt Dateien gelöschter Objekte, lässt fremde Dateien stehen | Plan 4.5; stabile Diffs für Git. Datei → Datenbank → Datei ist die Identität (getestet) |
| E47 | **Modell-Import behält die `MemberId`s bestehender Elemente** (Dimension wird bearbeitet, nicht neu aufgebaut); nur fehlende Elemente werden entfernt, neue bekommen neue Ids; ein unveränderter Import ändert und protokolliert nichts; vorher wird alles geprüft | Zellen liegen in der Datei unter MemberIds (E27). Ein erster Entwurf baute neu auf und hängte Zellen stillschweigend an falsche Elemente, was ein Roundtrip in eine *leere* Datenbank nicht zeigte; erst der Test mit Daten und umsortierter Datei deckte es auf |
| E48 | Dimensionsdateien werden ohne Trimmen gelesen (Namen mit Leerzeichen am Rand bleiben erhalten), fremde CSV-Dateien weiter mit Trimmen | Roundtrip-Treue für das, was NeOlap selbst schreibt |
| E49 | Ein bestehender Cube kann über den Modell-Import seine Dimensionen nicht ändern (klare Fehlermeldung) | Eine andere Dimensionsliste wäre ein neuer Cube; die Fakten ließen sich nicht abbilden |
| E50 | Schema 3 (`tokens`); Mindest-Rust 1.89 bleibt | siehe E30/E41 |

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
