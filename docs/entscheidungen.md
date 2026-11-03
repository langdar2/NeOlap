# Architekturentscheidungen

Kurzprotokoll der wesentlichen Entscheidungen. Neue Einträge oben anfügen.

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
