# NeOlap – Handbuch für Betrieb und Administration

Dieses Handbuch beschreibt, wie man NeOlap installiert, betreibt, sichert und aktualisiert. Die Bedienung der Web-Oberfläche steht in der Oberfläche selbst; die Formelsprache für Regeln und das Python-Modul sind in der [README](../README.md) und in den Beispielen unter `crates/neolap-py/examples/` beschrieben.

## 1. Überblick

NeOlap besteht aus einem Programm `neolap`. Es ist zugleich Kommandozeile, Server mit Web-Oberfläche und Bibliothek für das Python-Modul. Alle Daten liegen in **einer SQLite-Datei** (`neolap.neolap`): Modell, Zellen, Benutzer, Regeln, Importvorlagen und Änderungsjournal.

Eine Datei kann zur selben Zeit nur **ein** Prozess öffnen (Server, Kommandozeile oder Python). Der Server hält die Datei, solange er läuft.

## 2. Hardware und Grenzen

| Größe | Grenze |
|---|---|
| Dimensionen je Cube | 20 |
| Elemente je Dimension | 500.000, bis 50 Ebenen tief |
| gefüllte Basiszellen je Cube | 20 Millionen |
| gleichzeitige Benutzer | 20 |

**Arbeitsspeicher:** Alle Zellen liegen im Speicher. Gemessen (Modell mit zwei Dimensionen zu 500.000 Elementen, vier zu 20.000, 20 Mio. Zellen): Der Spitzenverbrauch im Container betrug **2,6 GB**. Das Standardlimit von 4 GB reicht also mit etwa 1,3 GB Reserve. Mehrere große Cubes in einer Datenbank addieren sich.

**Prozessorkerne:** Aggregate werden über alle Kerne gerechnet. Das wirkt sich stark auf die Zeit aus (siehe Kapitel 8). Für das Maximum von 20 Mio. Zellen sind **mindestens 4 Kerne** sinnvoll.

## 3. Installation

### 3.1 Docker Compose (empfohlen für Server)

```sh
cp .env.example .env              # NEOLAP_ADMIN_USER und NEOLAP_ADMIN_PASSWORD setzen
docker compose up -d --build
```

Beim ersten Start legt der Server mit diesen beiden Werten den Administrator an. Danach die Zeilen aus der `.env` entfernen. Das Passwort braucht mindestens 10 Zeichen.

Die Daten liegen im benannten Volume `neolap-data` unter `/data`. `docker compose down` löscht sie nicht; `docker compose down -v` **löscht sie**.

### 3.2 Fertiges Programm aus dem Release

Pro Plattform gibt es ein Archiv mit dem Programm `neolap`, README, Lizenz und diesem Handbuch, dazu eine Prüfsumme: `neolap-<Version>-macos-universal.tar.gz` (Apple silicon und Intel), `…-linux-x86_64.tar.gz`, `…-linux-arm64.tar.gz`, `…-windows-x86_64.zip`.

```sh
shasum -a 256 -c neolap-<Version>-macos-universal.tar.gz.sha256    # Linux: sha256sum -c
tar xzf neolap-<Version>-macos-universal.tar.gz
./neolap/neolap version
```

**macOS:** Das Programm ist nicht signiert und nicht notarisiert (kein Apple-Entwicklerzertifikat). Lädt man das Archiv mit einem Browser herunter, blockiert Gatekeeper das Programm. Abhilfe: einmalig `xattr -d com.apple.quarantine neolap/neolap`, oder das Programm selbst bauen (`cargo install --path crates/neolap-cli`). Das Verhalten ist nach der Beschreibung von Apple formuliert und auf diesem Rechner nicht nachgestellt worden (Download per `gh` setzt die Markierung nicht).

**Windows:** SmartScreen kann bei einem unsignierten Programm warnen („Weitere Informationen“ → „Trotzdem ausführen“). Auch das ist nicht geprüft.

**Linux:** Die Programme sind gegen glibc gebunden; auf einer sehr alten Distribution kann das Programm nicht starten. Für Server ist das Docker-Image der empfohlene Weg.

Alle vier Archive werden von `release.yml` gebaut und dort ausgeführt (`neolap version`, Beispieldatenbank anlegen). Gestartet wurde auf Windows und Linux nur das Programm selbst, nicht der Server.

### 3.3 Programm ohne Docker, aus dem Quelltext

```sh
neolap serve --data-dir ./data --listen 127.0.0.1:8080
```

Alle Optionen gibt es auch als Umgebungsvariablen (Kapitel 4).

## 4. Konfiguration

| Variable | Standard | Bedeutung |
|---|---|---|
| `NEOLAP_LISTEN` | `127.0.0.1:8080` | Adresse und Port (im Container `0.0.0.0:8080`) |
| `NEOLAP_DATA_DIR` | `./data` | Verzeichnis für Datenbank und Sicherungen |
| `NEOLAP_DATABASE` | `neolap.neolap` | Dateiname der Datenbank |
| `NEOLAP_ADMIN_USER`, `NEOLAP_ADMIN_PASSWORD` | leer | erster Administrator, nur wirksam bei einer Datenbank ohne Benutzer |
| `NEOLAP_SECURE_COOKIES` | `false` | Cookie nur über HTTPS (hinter TLS auf `true`) |
| `NEOLAP_TRUST_PROXY` | `false` | `X-Forwarded-For` des Proxys für die Anmeldesperre auswerten |
| `NEOLAP_BACKUP_INTERVAL_HOURS` | `24` | Stunden zwischen Sicherungen, `0` = aus |
| `NEOLAP_BACKUP_KEEP` | `7` | wie viele zeitgesteuerte Sicherungen bleiben |
| `NEOLAP_MEM_LIMIT` | `4g` | Speicherlimit des Containers (Compose) |
| `NEOLAP_BIND`, `NEOLAP_PORT` | `127.0.0.1`, `8080` | auf welcher Adresse der Host den Port anbietet |

Der Server ist standardmäßig nur vom eigenen Rechner erreichbar. Für Zugriff im Netz gibt es zwei Wege: `NEOLAP_BIND=0.0.0.0` (nur in vertrauenswürdigen Netzen, ohne Verschlüsselung) oder das HTTPS-Profil mit Caddy (siehe README, `NEOLAP_DOMAIN`).

## 5. Benutzer, Rollen, Tokens

Rollen: `reader` (nur lesen), `planner` (schreiben in freigegebenen Bereichen, Importe ausführen), `modeler` (Dimensionen, Cubes, Regeln und Importvorlagen ändern), `admin` (alles, auch Benutzer und Sicherungen). Schreibrechte des Planners lassen sich je Cube und Bereich beschränken. Verwaltung in der Web-Oberfläche (Bereich Verwaltung) oder bei **gestopptem** Server mit der Kommandozeile:

```sh
docker compose stop neolap
docker compose run --rm --no-deps --entrypoint /usr/local/bin/neolap neolap user list /data/neolap.neolap
```

**Dimensionen verwalten** (ab Version 1.1.0, ab Rolle `modeler`): Auf der Seite „Dimensionen“ legt „Neue Dimension“ eine leere Dimension an (Art: `generic`, `time`, `version` oder `measures`). „Import …“ liest Elemente aus einer CSV-Datei oder eingefügtem Text, im Format „Eltern-Kind“ (Kopfzeile `child;parent;weight`) oder „Ebenen“. Vor dem Ausführen zeigt eine Vorschau, wie viele Elemente neu entstehen. Im Modus „Abgleichen“ werden Elemente gelöscht, die in der CSV fehlen. Die Vorschau nennt sie, und ihre Werte verschwinden in allen Cubes. „Dimension umbenennen“ und „Dimension löschen“ sind gesperrt, solange ein Cube die Dimension nutzt. Eine Dimension, die in keinem Cube vorkommt, sehen nur Modellierer und Administratoren. Die Schnittstelle dazu steht im Implementierungsplan, Kapitel 6.3.

Für Skripte und das Python-Modul gibt es API-Tokens. Ein Token wird **einmal** angezeigt und ist danach nicht mehr lesbar. Ein verlorener Token wird widerrufen und neu erstellt.

Nach 5 falschen Anmeldungen innerhalb von 5 Minuten sperrt der Server weitere Versuche, getrennt nach Benutzername und nach Adresse des Aufrufers.

## 6. Sicherung und Wiederherstellung

### 6.1 Was gesichert wird

Eine Sicherung ist eine vollständige, konsistente Kopie der Datenbankdatei. Sie entsteht über SQLite (`VACUUM INTO`) und kann **bei laufendem Server** erstellt werden, ohne Schreibvorgänge zu blockieren.

### 6.2 Zeitgesteuert (Standard)

Der Server schreibt alle 24 Stunden `backup/auto-<Datum>-<Uhrzeit>.neolap` in das Datenverzeichnis und behält die jüngsten 7. Die erste Sicherung entsteht 24 Stunden nach dem Start, nicht sofort. Wer den Server öfter neu startet als alle 24 Stunden, bekommt dadurch nie eine zeitgesteuerte Sicherung: dann `NEOLAP_BACKUP_INTERVAL_HOURS` kleiner wählen oder 6.3 nutzen.

Erfolg und Fehler stehen im Log (`scheduled backup written` bzw. `scheduled backup failed`). Eine fehlgeschlagene Sicherung wird beim nächsten Intervall wiederholt.

### 6.3 Von Hand bei laufendem Server

Als Administrator in der Web-Oberfläche (Verwaltung → „Backup erstellen“) oder über die API:

```sh
curl -b jar -H "x-csrf-token: $CSRF" -X POST -H 'content-type: application/json' -d '{}' \
  http://localhost:8080/api/v1/admin/backup
```

Die Datei heißt `backup/backup-<Datum>-<Uhrzeit>.neolap`. Diese Sicherungen löscht NeOlap **nie** selbst.

`neolap backup` auf der Kommandozeile funktioniert nur bei gestopptem Server, weil der Server die Datei sperrt (Meldung: *in use by another process*).

### 6.4 Sicherungen aus dem Container holen

Die Sicherungen liegen im Volume, auf demselben Datenträger wie die Datenbank. **Eine Sicherung auf demselben Datenträger schützt nicht vor dessen Ausfall.** Regelmäßig auf ein anderes System kopieren:

```sh
docker run --rm -v neolap_neolap-data:/data:ro -v "$PWD":/out busybox \
  sh -c 'cp /data/backup/auto-*.neolap /out/'
```

Der Volume-Name setzt sich aus dem Projektnamen (`neolap`) und `neolap-data` zusammen; `docker volume ls` zeigt ihn.

### 6.5 Wiederherstellen

```sh
docker compose stop neolap
docker run --rm -v neolap_neolap-data:/data -v "$PWD":/in:ro busybox \
  sh -c 'cp /in/auto-20261104-050000.neolap /data/neolap.neolap && rm -f /data/neolap.neolap-wal /data/neolap.neolap-shm && chown 65532:65532 /data/neolap.neolap'
docker compose up -d
```

Der Server läuft im Image als Benutzer 65532 (`nonroot`). Gehört die Datei einem anderen Benutzer, startet der Server nicht. Die Dateien `-wal` und `-shm` gehören zur alten Datenbank und müssen weg, sonst würde SQLite sie auf die zurückgespielte Datei anwenden.

**Prüfen Sie jede Wiederherstellung einmal, bevor Sie sie brauchen.** Eine Sicherung, die nie zurückgespielt wurde, ist nur eine Vermutung.

## 7. Aktualisieren

1. **Sichern** (6.3) und die Sicherung aus dem Container holen (6.4).
2. Neues Image holen oder bauen: `docker compose pull` bzw. `docker compose up -d --build`.
3. Der neue Server öffnet die Datenbank. Ist deren Schema älter, legt er **vorher selbst** eine Kopie `neolap.neolap.before-v<N>` an und überführt dann die Datei. Diese Kopie bleibt liegen, bis Sie sie löschen.
4. Im Log prüfen: `docker compose logs neolap` zeigt die Version und `listening on`.
5. In der Oberfläche anmelden und eine bekannte Zelle prüfen.

**Zurück zur alten Version:** Eine Datenbank, die ein neueres Schema hat, öffnet die alte Version **nicht** (Fehlermeldung statt stiller Beschädigung). Zurückgehen heißt: altes Image starten und die Sicherung aus Schritt 1 (oder `.before-v<N>`) zurückspielen. Änderungen seit der Sicherung gehen dabei verloren.

Das Modell lässt sich unabhängig davon als lesbare Dateien exportieren (`neolap model export`) und in jede Version importieren. Das ist keine Sicherung der Zellwerte, aber ein Weg, ein Modell zu übertragen.

## 8. Leistung

Gemessen mit dem synthetischen Großmodell (`cargo test --release -p neolap-core --lib -- --ignored --nocapture synth_targets`): zwei Dimensionen mit 500.000 Elementen und 50 Ebenen, vier mit 20.000, vierzehn kleine, Mehrfach-Eltern, negative Gewichte.

| Messung | Ziel | M4 Pro, 14 Kerne | GitHub-Runner, 2 Kerne, 4 GB |
|---|---|---|---|
| kalter Aggregatwert, 10 Mio. Zellen, ungünstigster Fall | < 100 ms | 63–71 ms | nicht gemessen |
| kalter Aggregatwert, 20 Mio. Zellen, ungünstigster Fall | (Ziel gilt für 10 Mio.) | 103–117 ms | 0,56–1,5 s |
| kalter Aggregatwert, zufällige Teilbäume, 20 Mio. | < 100 ms | 3 ms | 64 ms (Max. 100 ms) |
| Aggregatwert im Cache | < 5 ms | < 1 µs | 2 µs |
| Pivot 50 × 20, 10 Mio. Zellen | < 500 ms | 53 ms | nicht gemessen |
| Pivot 50 × 20, 20 Mio. Zellen | | 82 ms | 977 ms |
| Splash über 100.000 Zellen | < 2 s | 9 ms | nicht gemessen |
| Import von 1 Mio. CSV-Zeilen | < 10 s | 1,7 s (mit Datei 5,9 s) | nicht gemessen |
| Start mit 10 Mio. Zellen | < 20 s | 2,6 s | nicht gemessen |
| Import einer Dimension mit 495.000 Elementen | < 5 s | 0,29 s | nicht gemessen |
| Spitzenspeicher, 20 Mio. Zellen | ≈ 2,4 GB | 2,1 GB Daten | 2,6 GB im Container |

**Wie die Zahlen zu lesen sind:** Die Zielwerte nennen keine Rechnergröße. Auf einem Rechner mit 14 Kernen sind alle erreicht. Auf dem 2-Kern-Runner liegt der ungünstigste kalte Wert bei 20 Mio. Zellen über einer Sekunde, weil der Scan über alle Zeilen geht und mit der Kernzahl skaliert. Das Maximum von 20 Mio. Zellen verlangt daher einen Rechner mit mehreren Kernen, wenn Aggregate über sehr große Elementmengen mehrerer Dimensionen interaktiv sein sollen. Ein Wert, der einmal berechnet wurde, kommt danach aus dem Cache, bis sich Zellen ändern.

## 9. Fehlersuche

| Beobachtung | Ursache und Abhilfe |
|---|---|
| Niemand kann sich anmelden | Die Datenbank hatte beim Start keinen Benutzer und `NEOLAP_ADMIN_*` waren nicht gesetzt (Warnung im Log). Server stoppen, `neolap user add <Datei> <Name> --role admin`. |
| *database is in use by another process* | Ein anderer Prozess (meist der Server) hat die Datei offen. Erst stoppen, oder den Server benutzen. |
| Container wird beendet (Exit 137) | Speicherlimit überschritten (Kapitel 2). `NEOLAP_MEM_LIMIT` erhöhen oder Zellen reduzieren. |
| Server startet nach Wiederherstellung nicht | Besitzer oder `-wal`-Dateien, siehe 6.5. |
| Anmeldung funktioniert über HTTPS nicht | `NEOLAP_SECURE_COOKIES=true` nur setzen, wenn der Zugriff wirklich über HTTPS läuft; umgekehrt braucht HTTPS dieses Flag. |
| Anmeldesperre trifft alle Benutzer | Hinter einem Proxy `NEOLAP_TRUST_PROXY=true`, sonst gelten alle als ein Client. |

## 10. Bekannte Einschränkungen (v1)

- Kein eingebautes TLS; für HTTPS das Caddy-Profil oder einen eigenen Proxy verwenden.
- Anmeldungen (Sitzungen) und laufende Importe liegen im Speicher: Nach einem Neustart müssen sich alle neu anmelden, ein laufender Import wird beim geordneten Stopp zu Ende geführt, bei hartem Abbruch verworfen.
- Keine Zahlenformate im Modell, kein Grund für gesperrte Zellen.
- Die Python-Wheels im Release (macOS arm64/x86_64, Linux x86_64/aarch64, Windows) werden von `wheels.yml` gebaut, installiert und importiert; ausprobiert wurde nur macOS arm64. Kein PyPI-Paket.
- Die Web-Oberfläche ist im Headless-Browser geprüft, die Abnahme durch eine Person steht aus ([Abnahme-M9.md](Abnahme-M9.md)).
