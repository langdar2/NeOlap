# NeOlap

Leichtgewichtige, modellgetriebene multidimensionale Datenbank (MOLAP) im Geist von MIS Alea und TM1 – mit Rückschreiben auf jeder Ebene, Rechenregeln, Datenimporten und gemeinsamer Nutzung im Browser. Geschrieben in Rust, ausgeliefert als einzelnes Programm für macOS, Linux und Windows.

> **Status:** Meilenstein M0 (Gerüst). Die Engine entsteht ab M1. Details und Roadmap: [docs/Implementierungsplan.md](docs/Implementierungsplan.md).

## Geplante Funktionen (v1)

- Dimensionen mit tiefen, unbalancierten Hierarchien (bis 500.000 Elemente, 50 Ebenen), Gewichten, Aliasen, Attributen
- Cubes mit bis zu 20 Dimensionen und bis zu 20 Mio. gefüllten Zellen, Aggregation zur Abfragezeit
- Rückschreiben auf Basis- und Aggregatzellen mit Splashing (proportional, gleich, Delta, Prozent, nach Referenz) und Undo
- Formelsprache für Regeln, z. B. `['Profit in %'] = ['Profit'] / ['Sales'];`
- Wiederholbare Datenimporte aus CSV/xlsx, Export nach CSV/xlsx
- Mehrbenutzer-Server mit Rollen, Schreibrechten bis auf Elementebene, Konflikterkennung und Änderungsjournal
- Web-Oberfläche mit Pivot-Raster, Kommandozeile `neolap`, Python-Modul `neolap`

## Starten mit Docker Compose (Server)

Voraussetzung: Docker mit Compose v2 (Docker Desktop, OrbStack, Colima oder Docker Engine).

```sh
cp .env.example .env              # optional anpassen (Port, Speicherlimit, …)
docker compose up -d --build      # bauen und starten
open http://localhost:8080        # Web-UI
docker compose logs -f neolap     # Logs
docker compose down               # stoppen – Daten bleiben im Volume "neolap-data"
```

Für Zugriff im Team mit HTTPS: in `.env` `NEOLAP_DOMAIN=<servername>` setzen und `docker compose --profile tls up -d` starten (Caddy holt bzw. erzeugt das Zertifikat automatisch).

Standardmäßig ist der Server nur vom eigenen Rechner erreichbar (`127.0.0.1`). Das Container-Speicherlimit ist auf 4 GB gesetzt (reicht für das Maximum von 20 Mio. gefüllten Zellen).

## Bauen und starten ohne Docker

Voraussetzung: Rust ≥ 1.85 ([rustup.rs](https://rustup.rs)).

```sh
cargo test                       # Tests
cargo run -p neolap-cli -- version
cargo run -p neolap-cli -- serve # Web-UI auf http://127.0.0.1:8080
```

Programm dauerhaft installieren:

```sh
cargo install --path crates/neolap-cli
neolap serve
```

Python-Modul (Entwicklungsstand) bauen:

```sh
python3 -m venv .venv && source .venv/bin/activate
pip install maturin
maturin develop -m crates/neolap-py/Cargo.toml
python -c "import neolap; print(neolap.version())"
```

## Projektstruktur

| Pfad | Inhalt |
|---|---|
| `crates/neolap-core` | Engine: Modell, Speicher, Abfrage, Splashing, Regeln, Persistenz, Import/Export, Rechte |
| `crates/neolap-server` | REST-API `/api/v1`, Anmeldung, Ereignisse, eingebettete Web-UI |
| `crates/neolap-cli` | Programm `neolap` |
| `crates/neolap-py` | Python-Modul (PyO3/maturin) |
| `ui/` | Web-Oberfläche (HTML/CSS/JS ohne Build-Schritt) |
| `Dockerfile`, `docker-compose.yml`, `deploy/` | Container-Betrieb, optionaler HTTPS-Proxy (Caddy) |
| `docs/` | Implementierungsplan, Entscheidungen, Handbuch |

## Herkunft und Lizenz

MIT-Lizenz, siehe [LICENSE](LICENSE). NeOlap ist eine Neuimplementierung auf Basis der Konzepte von [TinyOlap](https://github.com/Zeutschler/tinyolap) von Thomas Zeutschler (MIT), siehe [NOTICE.md](NOTICE.md).
