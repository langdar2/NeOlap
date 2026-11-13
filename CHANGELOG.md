# Änderungen

## 1.1.0

- Dimensionsverwaltung in der Web-Oberfläche und in der API: Dimensionen anlegen, umbenennen, per CSV importieren (mit Vorschau, Formate „Eltern-Kind“ und „Ebenen“, Modus „Nur hinzufügen“ oder „Abgleichen“) und löschen. Umbenennen und Löschen sind nur für Dimensionen möglich, die kein Cube nutzt.
- Neue Routen: `POST /api/v1/dimensions`, `POST /api/v1/dimensions/{d}/rename`, `POST /api/v1/dimensions/{d}/import`, `DELETE /api/v1/dimensions/{d}`.
- Web-Oberfläche: Der Elementzähler im Dropdown der Dimensionen bleibt nach Änderungen aktuell. Wird eine Dimension von jemand anderem umbenannt oder gelöscht, baut sich die Seite neu auf.

## 1.0.1

- Web-Oberfläche: Die Spalten im Pivot sind 150 statt 110 px breit. Lange Spaltenköpfe wie „Abweichung in %“ oder „Abschreibungen“ wurden vorher abgeschnitten.
- Neues Beispiel „BAB-Planung“ (`examples/build_bab_demo.py`): 10 Bereiche, 35 Abteilungen, 138 Teams, 6 Kostenarten, Plan und Ist, Abweichung als Regel. Mit Bildern im README.
- README und Handbuch: Stand nach dem Release 1.0.0 (Wheels, Image, offene Punkte).
- Release-Workflow für Python-Wheels: macOS x86_64 läuft auf `macos-15-intel`. Ein manueller Start mit `release_tag` hängt Wheels an ein bestehendes Release.

## 1.0.0

Erste Veröffentlichung. Neuimplementierung von TinyOlap 0.8.27 in Rust.

- Dimensionen mit Hierarchien, Mehrfach-Eltern, Gewichten, Aliasen, Attributen und Subsets (M1)
- Abfrage über Basis-Index und LeafSets, bis 20 Mio. gefüllte Basiszellen (M2)
- Rückschreiben mit Splash-Modi, Schutzregeln, Undo und Zellversionen (M3)
- Regelsprache mit Feedern und Fehlerwerten (M4)
- SQLite-Persistenz mit Journal, Backup und Sperrdatei (M5)
- Import/Export (CSV, xlsx, Spezifikationen, `replace_area`) (M6)
- Kommandozeile (M7)
- Mehrbenutzer-Server mit REST-API, Rollen, Bereichsrechten, Konflikterkennung, SSE und zeitgesteuerter Sicherung (M8, M11)
- Web-Oberfläche (M9)
- Python-Modul, lokal und gegen einen Server, mit pandas (M10)
- Benchmarks, Handbuch, Release-Binaries und Docker-Image (M11)

Bekannte Einschränkung: Für die Zielwerte gibt es keine Referenzhardware (E85). Empfohlen sind mindestens 4 Kerne.
