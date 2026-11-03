# Architekturentscheidungen

Kurzprotokoll der wesentlichen Entscheidungen. Neue Einträge oben anfügen.

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
