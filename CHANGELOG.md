# Änderungen

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
