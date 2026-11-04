# NeOlap

Leichtgewichtige, modellgetriebene multidimensionale Datenbank (MOLAP) im Geist von MIS Alea und TM1 – mit Rückschreiben auf jeder Ebene, Rechenregeln, Datenimporten und gemeinsamer Nutzung im Browser. Geschrieben in Rust, ausgeliefert als einzelnes Programm für macOS, Linux und Windows.

> **Status:** Meilensteine M0–M10 sind umgesetzt: Engine (Dimensionen, Abfrage, Rückschreiben, Regeln, Persistenz, Import/Export), Kommandozeile, Mehrbenutzer-Server mit REST-API, Web-Oberfläche und Python-Modul (lokal und gegen einen Server). Das Python-Wheel ist nur für macOS arm64 gebaut und geprüft. Die Web-Oberfläche ist im Headless-Browser geprüft, aber noch nicht von einer Person am Mac abgenommen (Abnahme-Checkliste des Plans). Details und Roadmap: [docs/Implementierungsplan.md](docs/Implementierungsplan.md).

## Funktionen (v1)

- Dimensionen mit tiefen, unbalancierten Hierarchien (bis 500.000 Elemente, 50 Ebenen), Gewichten, Aliasen, Attributen
- Cubes mit bis zu 20 Dimensionen und bis zu 20 Mio. gefüllten Zellen, Aggregation zur Abfragezeit
- Rückschreiben auf Basis- und Aggregatzellen mit Splashing (proportional, gleich, Delta, Prozent, nach Referenz) und Undo
- Formelsprache für Regeln, z. B. `['Profit in %'] = ['Profit'] / ['Sales'];`
- Wiederholbare Datenimporte aus CSV/xlsx, Export nach CSV/xlsx
- Mehrbenutzer-Server mit Anmeldung, Rollen, Schreibrechten bis auf Elementebene, Konflikterkennung, Live-Ereignissen (SSE) und Änderungsjournal
- Kommandozeile `neolap` (lokal auf einer Datei)
- Web-Oberfläche: Pivot-Raster mit Aufklappen, Eingabe in der Zelle (Splash-Syntax), Einfügen aus Tabellenkalkulationen, Undo (Strg+Z), Konfliktdialog, Live-Aktualisierung; Dimensions- und Regeleditor, Import mit Vorschau, Journal, Verwaltung
- Python-Modul `neolap`: dieselbe API auf einer Datei und gegen einen Server, DataFrames, Laden aus pandas/CSV/xlsx

## Starten mit Docker Compose (Server)

Voraussetzung: Docker mit Compose v2 (Docker Desktop, OrbStack, Colima oder Docker Engine).

```sh
cp .env.example .env              # anpassen: NEOLAP_ADMIN_PASSWORD setzen (mind. 10 Zeichen)
docker compose up -d --build      # bauen und starten
docker compose logs -f neolap     # Logs
docker compose down               # stoppen – Daten bleiben im Volume "neolap-data"
```

**Erster Start:** Ohne Benutzer kann sich niemand anmelden. Setze deshalb in `.env` `NEOLAP_ADMIN_USER` und `NEOLAP_ADMIN_PASSWORD`; der Server legt damit beim ersten Start den Administrator an (die Zeilen danach aus der Datei entfernen). Weitere Benutzer legt der Admin über die REST-API an (`POST /api/v1/admin/users`) oder, bei gestopptem Server, mit `neolap user add`. Die Datenbank liegt im Volume als `/data/neolap.neolap`.

Die Web-Oberfläche liegt unter `http://localhost:8080`. Anmelden und eine Zelle lesen über die REST-API (Beispiel):

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
cargo test                       # Tests der Engine, des Servers und der Kommandozeile
node --test ui/tests/*.test.js   # Tests der Web-Oberfläche (Node ≥ 22, keine Abhängigkeiten)
cargo run -p neolap-cli -- version
cargo run -p neolap-cli -- serve # Web-UI auf http://127.0.0.1:8080
```

Programm dauerhaft installieren:

```sh
cargo install --path crates/neolap-cli
neolap serve
```

## Python

```sh
pip install neolap            # ohne Abhängigkeiten; für DataFrames: pip install 'neolap[pandas]'
```

```python
import neolap

db = neolap.open("plan.neolap")                                 # eine Datei, exklusiv
db = neolap.connect("https://olap.intern:8443", token="nlp_…")  # oder ein Server (Token: `neolap user token`)

cube = db.cube("sales")
cube["Jan", "North", "sedan", "Sales"] = 100.0                  # Zelle schreiben
cube["Q1", "Total", "All", "Sales"] = 5000                      # Aggregat: Wert wird verteilt
cube.write(("Q1", "Total", "All", "Sales"), "#5000")            # gleichmäßig verteilen
df = cube.area(years=["Jan", "Feb", "Mar"], regions="North").to_pandas()   # Bereich als DataFrame
df["value"] *= 1.05                                                         # mit pandas rechnen ...
cube.load(df, mapping={d: d for d in cube.dimensions}, value_column="value")  # ... und zurückladen (eine Operation)
```

Die Beispiele laufen unverändert auf einer Datei und gegen einen Server, ebenso das Notebook [`crates/neolap-py/examples/tiny.ipynb`](crates/neolap-py/examples/tiny.ipynb). Eine Datei ist exklusiv: ein laufender Server und ein Python-Prozess können nicht dieselbe Datei öffnen (`neolap.LockedError`). Ein Token geht nur über https (außer an localhost) und steht nie in einer Fehlermeldung. Alle Fehler sind `neolap.NeolapError` (`NotFoundError`, `ConflictError`, `InputError`, …).

Entwicklung (Rust-Erweiterung und Tests):

```sh
python3 -m venv .venv && source .venv/bin/activate
pip install maturin pytest pandas numpy nbformat nbclient ipykernel
cargo build -p neolap-cli                                      # die Tests starten die echte Binary
maturin develop -m crates/neolap-py/Cargo.toml
pytest crates/neolap-py/tests                                  # jeder Test läuft lokal und gegen einen Server
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
