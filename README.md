# NeOlap

Leichtgewichtige, modellgetriebene multidimensionale Datenbank (MOLAP) im Geist von MIS Alea und TM1 – mit Rückschreiben auf jeder Ebene, Rechenregeln, Datenimporten und gemeinsamer Nutzung im Browser. Geschrieben in Rust, ausgeliefert als einzelnes Programm für macOS, Linux und Windows.

> **Status:** Meilensteine M0–M8 sind umgesetzt: Engine (Dimensionen, Abfrage, Rückschreiben, Regeln, Persistenz, Import/Export), Kommandozeile und Mehrbenutzer-Server mit REST-API. **Die Web-Oberfläche (Pivot-Raster, M9) und das Python-Modul (M10) fehlen noch**: Die Startseite zeigt nur den Serverstatus. Bedienung heute über Kommandozeile und REST-API. Details und Roadmap: [docs/Implementierungsplan.md](docs/Implementierungsplan.md).

## Funktionen (v1)

- Dimensionen mit tiefen, unbalancierten Hierarchien (bis 500.000 Elemente, 50 Ebenen), Gewichten, Aliasen, Attributen
- Cubes mit bis zu 20 Dimensionen und bis zu 20 Mio. gefüllten Zellen, Aggregation zur Abfragezeit
- Rückschreiben auf Basis- und Aggregatzellen mit Splashing (proportional, gleich, Delta, Prozent, nach Referenz) und Undo
- Formelsprache für Regeln, z. B. `['Profit in %'] = ['Profit'] / ['Sales'];`
- Wiederholbare Datenimporte aus CSV/xlsx, Export nach CSV/xlsx
- Mehrbenutzer-Server mit Anmeldung, Rollen, Schreibrechten bis auf Elementebene, Konflikterkennung, Live-Ereignissen (SSE) und Änderungsjournal
- Kommandozeile `neolap` (lokal auf einer Datei)
- noch offen: Web-Oberfläche mit Pivot-Raster (M9), Python-Modul `neolap` (M10)

## Starten mit Docker Compose (Server)

Voraussetzung: Docker mit Compose v2 (Docker Desktop, OrbStack, Colima oder Docker Engine).

```sh
cp .env.example .env              # anpassen: NEOLAP_ADMIN_PASSWORD setzen (mind. 10 Zeichen)
docker compose up -d --build      # bauen und starten
docker compose logs -f neolap     # Logs
docker compose down               # stoppen – Daten bleiben im Volume "neolap-data"
```

**Erster Start:** Ohne Benutzer kann sich niemand anmelden. Setze deshalb in `.env` `NEOLAP_ADMIN_USER` und `NEOLAP_ADMIN_PASSWORD`; der Server legt damit beim ersten Start den Administrator an (die Zeilen danach aus der Datei entfernen). Weitere Benutzer legt der Admin über die REST-API an (`POST /api/v1/admin/users`) oder, bei gestopptem Server, mit `neolap user add`. Die Datenbank liegt im Volume als `/data/neolap.neolap`.

Anmelden und eine Zelle lesen (Beispiel, die Web-Oberfläche folgt in M9):

```sh
curl -c jar -H 'content-type: application/json' -d '{"user":"admin","password":"…"}' http://localhost:8080/api/v1/login
curl -b jar http://localhost:8080/api/v1/model
```

Skripte nutzen statt Cookies besser einen API-Token (`POST /api/v1/admin/users/<name>/token`, wird einmal angezeigt) mit `Authorization: Bearer <token>`. Ändernde Anfragen per Cookie brauchen zusätzlich den Header `X-CSRF-Token` (Wert aus der Antwort von `/login`).

Für Zugriff im Team mit HTTPS: in `.env` `NEOLAP_DOMAIN=<servername>`, `NEOLAP_SECURE_COOKIES=true` und `NEOLAP_TRUST_PROXY=true` setzen und `docker compose --profile tls up -d` starten (Caddy holt bzw. erzeugt das Zertifikat automatisch). Ohne `NEOLAP_TRUST_PROXY` zählt die Anmeldesperre alle Nutzer hinter dem Proxy als einen Client.

Standardmäßig ist der Server nur vom eigenen Rechner erreichbar (`127.0.0.1`). Das Container-Speicherlimit ist auf 4 GB gesetzt (reicht für das Maximum von 20 Mio. gefüllten Zellen).

## Bauen und starten ohne Docker

Voraussetzung: Rust ≥ 1.89 ([rustup.rs](https://rustup.rs)).

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
