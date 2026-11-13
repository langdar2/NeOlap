# Abnahme M9: Web-Oberfläche

Checkliste für eine Person am Mac. Dauer etwa 45 Minuten. Alle Punkte sind bisher nur im Headless-Browser (Chromium) durchgespielt worden, nie von Hand und nie in Safari oder Firefox. Die Shell-Blöcke in Abschnitt 0 und 10 sind wörtlich ausgeführt worden und laufen; die Browser-Punkte in 9.1 bis 9.5 sind nicht geprüft. Abschnitt 11 (Docker) wurde am 13.11.2026 mit dem veröffentlichten Image `ghcr.io/langdar2/neolap:1.1.0` durchgespielt (siehe dort), der lokale Build aber nicht.

Eintragen: Haken setzen, bei Abweichung die Nummer und was Sie gesehen haben notieren.

Abnahmedatum: ____________  Browser und Version: ____________  Geprüft von: ____________

## 0. Vorbereitung

```sh
cargo build -p neolap-cli
rm -rf /tmp/abnahme && mkdir -p /tmp/abnahme/data && cd /tmp/abnahme
N=~/projects/NeOlap/target/debug/neolap
$N demo tiny data/neolap.neolap
export NEOLAP_PASSWORD='abnahme-passwort-1'
$N user add data/neolap.neolap anna --role admin
$N user add data/neolap.neolap lena --role reader
$N user add data/neolap.neolap paul --role planner
$N serve --data-dir data
```

Der Server läuft dann auf `http://127.0.0.1:8080`. Passwort aller drei: `abnahme-passwort-1`. Für Abschnitt 8 zusätzlich ein zweites Browserfenster (privates Fenster).

Erwartung der Demo `tiny` (Cube `sales`, 192 Zellen): Zeile `Year`, Spalte `Sales` = **165.600**, `Cost` = **107.640**, `Profit` = **57.960**, `Profit in %` = 0,35.

## 1. Anmeldung

- [ ] **1.1** Falsches Passwort: Meldung „Benutzername oder Passwort stimmt nicht", keine Angabe, ob der Name existiert.
- [ ] **1.2** Fünfmal falsch, dann das richtige Passwort: Meldung zur Sperre (mehrere Minuten). Danach Server neu starten, um die Sperre zu löschen.
- [ ] **1.3** Anmelden als `anna`: Pivot-Seite erscheint, oben „anna · Admin", Anzeige „live" grün.
- [ ] **1.4** Seite neu laden (Cmd+R): Sie bleiben angemeldet.
- [ ] **1.5** Abmelden: Anmeldeformular. Zurück-Taste des Browsers zeigt keine Daten mehr.
- [ ] **1.6** Konsole des Browsers (Cmd+Alt+I) beim Laden der Anmeldeseite: **keine** roten Fehler.

## 2. Raster und Navigation

- [ ] **2.1** Zeile `Year` anklicken: Rahmen um die Zelle, unten die Statuszeile mit Adresse (`Year · Total · All · Profit`).
- [ ] **2.2** Pfeiltasten bewegen die Zelle, Tab geht nach rechts und am Zeilenende in die nächste Zeile, Umschalt+Tab zurück.
- [ ] **2.3** Pos1, Ende, Strg+Pos1, Strg+Ende, Bild auf/ab verhalten sich wie in Excel.
- [ ] **2.4** Umschalt+Pfeil markiert einen Bereich, die Statuszeile zeigt „Summe … (n Zellen)". Rechnen Sie bei zwei Zellen nach.
- [ ] **2.5** Maus ziehen markiert ebenfalls einen Bereich. Klick auf einen Zeilenkopf markiert die ganze Zeile, auf einen Spaltenkopf die Spalte.
- [ ] **2.6** Cmd+C auf einem Bereich, einfügen in Excel oder Numbers: Zahlen stehen in Zellen, nicht in einer Zeile.
- [ ] **2.7** Zeilen- und Spaltenköpfe bleiben beim Scrollen stehen und laufen mit (Bildschirm klein ziehen, damit gescrollt werden muss).
- [ ] **2.8** Optik: Aggregate fett, Regelzelle (`Profit in %`) kursiv und violett, beschreibbare Zellen mit blauem Strich unten, Negativwerte rot.

## 3. Hierarchien

- [ ] **3.1** Pfeil vor `Year` zuklappen und wieder aufklappen. Danach `Q1` aufklappen: `Jan`, `Feb`, `Mar` erscheinen eingerückt.
- [ ] **3.2** `Q1` zuklappen: die drei Monate verschwinden. Klappen Sie `Q1` erneut auf, kommen sie zugeklappt zurück.
- [ ] **3.3** Spalte `Profit` aufklappen: `Sales`, `Cost` erscheinen als Spalten.
- [ ] **3.4** Dimension `years` per Maus von „Zeilen" nach „Spalten" ziehen: Tausch der Achsen, Werte stimmen weiter.
- [ ] **3.5** Filter `regions` auf `North` stellen: Werte sinken. Suchfeld darunter: „sou" eingeben, Enter: `South` wird gewählt.
- [ ] **3.6** „Leere ausblenden" an und aus: keine Fehler, Zeilen und Spalten ohne Werte verschwinden.

## 4. Eingabe und Splash

Aufbau: `Q1` und `Profit` aufklappen, Filter `regions` = `Total`, `products` = `All`.

- [ ] **4.1** Zelle `Jan / Sales` anklicken: Statuszeile nennt „Aggregat, beschreibbar" und **„wirkt auf 8 Basiszellen, 8 davon gefüllt"**.
- [ ] **4.2** `5000` tippen, Enter: Meldung „8 Zelle(n) geändert", `Jan / Sales` = 5.000, `Q1 / Sales` und `Year / Sales` sinken um 5.720 (also 28.120 und 159.880).
- [ ] **4.3** Strg+Z (oder „Rückgängig"): alle Werte wieder wie vorher (`Jan / Sales` = 10.720).
- [ ] **4.4** Eine Basiszelle: Filter `regions` = `North`, `products` = `sedan`, dann `Jan / Sales`. Statuszeile „Basiszelle". `100` tippen, Enter: Meldung „1 Zelle(n) geändert".
- [ ] **4.5** Escape während der Eingabe verwirft sie, der alte Wert bleibt.
- [ ] **4.6** F2 oder Doppelklick öffnet die Eingabe mit dem bisherigen Wert, nicht leer.
- [ ] **4.7** Entf auf einer Basiszelle löscht den Wert (Zelle leer).
- [ ] **4.8** Eingaben: `#500` (gleichmäßig), `+50`, `-50`, `+5%`, `-3%` auf einem Aggregat. Prüfen Sie jeweils, dass Summe und Verteilung zur Beschreibung im Plan (Kap. 4.3) passen.
- [ ] **4.9** `abc` oder `5x` eingeben: Meldung „keine gültige Eingabe", nichts wird geschrieben (Generation in der Statuszeile gleich).
- [ ] **4.10** Auf `Profit in %` (Regelzelle) und auf `Jan / Profit` tippen: **eine** Meldung, gesperrt, kein Editor. Mehrere Ziffern hintereinander stapeln keine Meldungen.

## 5. Zahlenformat (Entscheidung vom 03.11.)

Ziel: Zahlen aus einer deutschen Tabelle bedeuten das, was Sie sehen.

- [ ] **5.1** In eine Basiszelle `2.000` tippen: Ergebnis **2.000** (zweitausend), nicht 2.
- [ ] **5.2** `1.000,5` ergibt 1.000,5. `0,25` ergibt 0,25. `12.345` ergibt 12.345.
- [ ] **5.3** `1.5,5` und `1,,5` werden abgelehnt (Meldung, nichts geschrieben).
- [ ] **5.4** Aus Excel einen 2 × 2 Block mit deutschem Zahlenformat kopieren (z. B. `1.000,5`, `800`, `2.000`, `1.500,25`), im Raster auf die erste Zelle klicken, Cmd+V: alle vier Werte stimmen. Bei gesperrten Zellen im Block kommt eine Rückfrage.

## 6. Viele Zellen

- [ ] **6.1** Gesamtsumme anklicken (`Year / Sales`, Filter `Total`/`All`): Statuszeile zeigt „wirkt auf 96 Basiszellen" (12 Monate × 4 Regionen × 2 Produkte) und beim Tippen **keine** Rückfrage (unter 1.000).
- [ ] **6.2** Optional mit dem großen Modell aus Abschnitt 10: Gesamtsumme anklicken, `12345` tippen, Enter: Rückfrage „Bis zu 60.000 Basiszellen werden geändert". „Abbrechen" ändert nichts. Nochmal, „Schreiben": Summe ist 12.345. Strg+Z stellt den alten Wert exakt wieder her.

## 7. Weitere Seiten

- [ ] **7.1 Dimensionen:** `years` wählen, Baum aufklappen, ein Element anklicken. „Kind hinzufügen" legt ein Element an, „Umbenennen" und „Alias" funktionieren, „Löschen" fragt vorher und nennt, was verloren geht. Danach im Pivot prüfen, dass das neue Element erscheint.
- [ ] **7.1a Dimensionsverwaltung** (ab 1.1.0, als Modellierer): „Neue Dimension“, Name `Test`, Art `generic`: erscheint im Auswahlfeld als „Test (0) · ungenutzt“. „Element oben anlegen“: Der Zähler im Auswahlfeld springt ohne Neuladen auf „(1)“. „Import …“ mit Kopfzeile `child;parent;weight` und einigen Zeilen: Vorschau nennt die Zahlen, „Importieren“ füllt den Baum. Eine CSV ohne Kopfzeile wird mit einer verständlichen Meldung abgelehnt. „Dimension umbenennen“: Name im Auswahlfeld und Elemente bleiben. „Dimension löschen“: Rückfrage mit Elementzahl, danach ist die Dimension weg. Bei `years` (von einem Cube genutzt) sind „umbenennen“ und „löschen“ gesperrt und zeigen den Hinweis. Zweite Sitzung: Benennt sie die geöffnete Dimension um oder löscht sie, baut sich die Seite ohne Fehlermeldung neu auf. Import-Modus „Abgleichen“ (CSV mit weniger Elementen als die Dimension): Die Vorschau nennt die Elemente, die entfernt würden, und warnt, dass ihre Werte in allen Cubes gelöscht werden. „Abbrechen“ ändert nichts, „Importieren“ entfernt genau diese Elemente. Rollen: Ein `reader` und ein `planner` sehen den Menüpunkt „Dimensionen“ nicht; eine Dimension, die kein Cube nutzt, ist für sie auch mit Cube-Recht unsichtbar (nur Modellierer und Administratoren sehen sie). Bisher nur im Headless-Browser (Chromium) durchgespielt, nie von Hand und nie in Safari oder Firefox. Dort belegt: Anlegen, Element hinzufügen, Zähler im Auswahlfeld, Umbenennen, Import (auch „Abgleichen“ und Abbrechen), Löschen, gesperrte Knöpfe bei allen vier genutzten Dimensionen des Demo-Modells, fremde Umbenennung und Löschung bei offener Seite, Leser und Planer ohne Cube-Recht sowie Leser mit Leserecht (nur dieser Leser, den Planer mit Cube-Recht gesondert nicht). Nicht geprüft: kleine Bildschirme.
- [ ] **7.2 Regeln:** Zeilennummern sichtbar. Eine Zeile mit Syntaxfehler eingeben (`['Profit in %'] = ;`), „Prüfen": Meldung mit Zeilennummer, „Speichern" lehnt ab, der alte Text bleibt gespeichert. Eine Regel mit unbekanntem Element: „1 ungültig (werden übersprungen)".
- [ ] **7.3 Import:** Eine kleine CSV mit Spalten `Monat;Region;Produkt;Umsatz;Kosten` und passender Spezifikation (Vorlage der Seite als Ausgangspunkt, `[values]` ausfüllen). „Vorschau": Zähler, nichts ändert sich. Eine Zeile mit unbekannter Region: wird mit Zeilennummer und Grund gemeldet, nichts wird geladen. „Importieren": Rückfrage, Fortschritt, Abschlussbericht, Werte stehen im Pivot.
- [ ] **7.4 Journal:** zeigt Ihre Änderungen mit Zeit, Benutzer, Aktion und Zellzahl. Filter nach Cube und Benutzer.
- [ ] **7.5 Verwaltung** (nur als `anna`): Benutzer anlegen, Rolle ändern, Token erzeugen (wird einmal angezeigt), Recht erteilen, Backup erstellen (Meldung mit Dateiname, Datei liegt unter `data/backup/`). Den letzten Admin lässt sich nicht löschen oder herabstufen.
- [ ] **7.6 Export:** Buttons „CSV" und „Excel" oben im Pivot laden eine Datei, die sich öffnen lässt und die Zahlen enthält.

## 8. Mehrere Nutzer

Zwei Fenster: eines als `anna`, ein privates als `paul`. Vorher als `anna` in der Verwaltung `paul` das Recht „schreiben" auf `sales` erteilen (Bereich leer).

- [ ] **8.1 Live:** `paul` ändert eine Zelle, in `annas` Fenster erscheint nach etwa einer Sekunde der neue Wert, kurz gelb markiert, mit Hinweis „paul hat … geändert".
- [ ] **8.2 Konflikt:** In `annas` Fenster die Live-Anzeige stören (WLAN aus, 20 Sekunden warten bis „getrennt", `paul` ändert eine Zelle, WLAN an, **sofort** dieselbe Zelle in `annas` Fenster ändern). Erwartet: Dialog „Änderung von jemand anderem" mit Name, Zelle, aktuellem Wert und Ihrer Eingabe. Fokus liegt auf „Meine Eingabe verwerfen". Beide Wege prüfen. Hinweis: Nach dem Wiederverbinden lädt die Seite von selbst neu, der Dialog erscheint nur, wenn Sie schneller sind.
- [ ] **8.3 Leser:** Als `lena` anmelden. Ohne Recht: Meldung „Sie haben noch auf keinen Cube Zugriff". Nach Leserecht (Verwaltung): Pivot sichtbar, **keine** Zelle mit blauem Strich, Tippen ändert nichts, im Menü fehlen Dimensionen, Import und Verwaltung.
- [ ] **8.4 Bereich:** `paul` nur `regions = North` schreiben erlauben (Recht entziehen, neu erteilen mit Bereich). Schreiben in `North` geht, in `South` nicht (Meldung), und ein Splash auf `Total`, der `South` berührt, wird als Ganzes abgelehnt.
- [ ] **8.5 Neustart:** Server mit Strg+C beenden und neu starten, während `anna` angemeldet ist. Innerhalb weniger Sekunden erscheint die Anmeldung mit dem Hinweis, dass die Sitzung beendet ist.

## 9. Browser, Darstellung, Sicherheit

- [ ] **9.1** Safari **und** Firefox: Abschnitte 1 bis 4 wiederholen (Raster, Eingabe, Einfügen, Konflikt-Dialog). Notieren Sie Abweichungen.
- [ ] **9.2** Dunkelmodus (Systemeinstellung): Seite ist lesbar, Kontraste stimmen, Regelzellen und Fehlerzellen sind unterscheidbar.
- [ ] **9.3** Fenster auf Handybreite ziehen (unter 800 px): Seitenleiste rutscht über das Raster, nichts ist abgeschnitten, Raster scrollt.
- [ ] **9.4** Seite zoomen (Cmd+Plus bis 200 %): Raster bleibt bedienbar.
- [ ] **9.5** Tastatur: Tab erreicht alle Knöpfe, Fokus ist sichtbar, Dialoge lassen sich mit Escape schließen.
- [ ] **9.6** Hostiler Name: Als `anna` in „Dimensionen" ein Kind mit dem Namen `<img src=x onerror=alert(1)>` anlegen. Im Pivot und im Baum erscheint der Name als Text, es öffnet sich **kein** Alert. Das Element danach löschen.
- [ ] **9.7** Wählen Sie im Journal und in der Verwaltung beliebige Einträge: nirgends erscheinen Passwörter oder Token.

## 10. Leistung mit dem großen Modell (optional)

Erzeugt 3.000 Produkte × 20 Perioden (60.000 Zellen):

```sh
cd /tmp/abnahme && N=~/projects/NeOlap/target/debug/neolap
mkdir -p big/m/dimensions big/m/cubes big/data && python3 - <<'E'
import random
r=["child;parent;weight;alias","All;;;"]
for g in range(30):
    r.append(f"g{g};All;;")
    r += [f"p{g}_{p};g{g};;" for p in range(100)]
open("big/m/dimensions/products.csv","w").write("\n".join(r)+"\n")
open("big/m/dimensions/periods.csv","w").write("\n".join(["child;parent;weight;alias","Year;;;"]+[f"m{m};Year;;" for m in range(1,21)])+"\n")
open("big/m/dimensions/measures.csv","w").write("child;parent;weight;alias\nSales;;;\n")
open("big/m/dimensions/measures.toml","w").write('kind = "Measures"\n')
open("big/m/cubes/plan.toml","w").write('dimensions = ["products", "periods", "measures"]\n')
random.seed(7)
c=["products;periods;measures;value"]+[f"p{g}_{p};m{m};Sales;{random.randint(1,1000)}" for g in range(30) for p in range(100) for m in range(1,21)]
open("big/cells.csv","w").write("\n".join(c)+"\n")
open("big/spec.toml","w").write('name="perf"\ncube="plan"\nvalue_column="value"\nsource={file="cells.csv",delimiter=";"}\n[map]\nproducts="products"\nperiods="periods"\nmeasures="measures"\n')
E
$N new big/data/neolap.neolap && $N model import big/data/neolap.neolap big/m && $N load big/data/neolap.neolap big/spec.toml
NEOLAP_PASSWORD='abnahme-passwort-1' $N user add big/data/neolap.neolap anna --role admin
$N serve --data-dir big/data --listen 127.0.0.1:8081
```

- [ ] **10.1** Auf der Startseite `periods` von „Zeilen" nach „Spalten" ziehen und `g0` aufklappen. Das Raster erscheint ohne spürbare Wartezeit (Plan: unter 0,5 s für 50 × 20). Gemessen im Headless-Browser: 22 ms bis zum Zeichnen.
- [ ] **10.2** Mit Mausrad und Bildlauf-Leiste schnell durch die 100 Zeilen scrollen: keine Lücken, kein Ruckeln, Zeilenköpfe und Zellen bleiben ausgerichtet.
- [ ] **10.3** Splash auf die Gesamtsumme (Punkt 6.2).

## 11. Docker (einmalig, wenn Docker vorhanden ist)

Stand vom 13.11.2026: 11.1 bis 11.3 wurden mit dem fertigen Image `ghcr.io/langdar2/neolap:1.1.0` durchgespielt (`NEOLAP_IMAGE` in einer Env-Datei außerhalb des Repos, Port 18090, Headless-Browser) und verhielten sich wie beschrieben. **Nicht geprüft** ist der lokale Build mit `--build`: Er scheiterte auf dem Rechner dieser Prüfung an einem Zertifikat (`self-signed certificate in certificate chain` bei `cargo fetch`, vermutlich ein Proxy). Der Build in GitHub Actions läuft grün. Ebenfalls nicht geprüft: das Profil `tls` mit Caddy und die jeweils andere Prozessorarchitektur.

- [ ] **11.1** `cp .env.example .env`, in `.env` `NEOLAP_ADMIN_PASSWORD` setzen, `docker compose up -d --build`. Anmeldung mit `admin` und diesem Passwort funktioniert. (`.env` steht in der `.gitignore`.) Mit dem fertigen Image statt des Builds: `NEOLAP_IMAGE=ghcr.io/langdar2/neolap:1.1.0` in `.env` setzen und `docker compose up -d --no-build` starten.
- [ ] **11.2** `docker compose restart neolap`: Die Sitzung ist danach beendet (Sitzungen liegen im Speicher), die erneute Anmeldung funktioniert, die Daten sind noch da (Volume).
- [ ] **11.3** Mit offenem Browser-Tab `docker compose stop neolap`: Container endet in unter 5 Sekunden, nicht erst nach 30. Gemessen: 0,24 s, Exit-Code 0.
- [ ] **11.4** CI-Lauf auf GitHub (Workflow „Docker") ist grün. Der Schritt „Login, protected routes and data in the volume" lief zuletzt im Lauf `43420085626` erfolgreich.

## Bekannte Einschränkungen (keine Abnahmefehler)

Diese Punkte sind bekannt und stehen im Bericht zu M9. Nicht als Fehler melden, sondern entscheiden, ob sie vor v1 gelöst werden müssen.

- Prozentwerte erscheinen als `0.35`, nicht `35 %`: Das Modell kennt noch keine Zahlenformate.
- Eine gesperrte Zelle nennt keinen Grund („Rechte, Gewichte oder Regeln im Bereich").
- `1.000` ist immer eintausend, nie „eins Komma null null null".
- Bereichsrechte begrenzen nur das Schreiben. Wer einen Cube lesen darf, sieht ihn ganz.
- Sitzungen und Import-Jobs liegen im Speicher und enden mit einem Neustart.
- Servermeldungen (z. B. „2 row(s) rejected") sind teils englisch.
- Spaltenüberschriften sind rechtsbündig, der Aufklapp-Pfeil steht dort hinter dem Namen, bei Zeilen davor.

## Ergebnis

- [ ] Alle Punkte bestanden, M9 ist abgenommen.
- [ ] Mit Abweichungen abgenommen (Liste unten).
- [ ] Nicht abgenommen.

Abweichungen:

