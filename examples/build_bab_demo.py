#!/usr/bin/env python3
"""Builds the "BAB-Planung" example database: a cost centre plan (Betriebsabrechnungsbogen).

    python3 examples/build_bab_demo.py bab.neolap
    neolap serve --data-dir . --database bab.neolap

Structure: 10 Bereiche, each with 2-5 Abteilungen, each with 2-5 Teams (about 120 cost centres at the
bottom), 6 Kostenarten, 12 Monate, Version Plan and Ist. "Abweichung" and "Abweichung in %" are rules.
Needs only Python 3 and a `neolap` binary on the PATH (or NEOLAP=/path/to/neolap).
The numbers are made up (fixed seed), so the result is the same on every run.
"""

import csv
import math
import os
import random
import subprocess
import sys
import tempfile
from pathlib import Path

NEOLAP = os.environ.get("NEOLAP", "neolap")
MONTHS = ["Jan", "Feb", "Mär", "Apr", "Mai", "Jun", "Jul", "Aug", "Sep", "Okt", "Nov", "Dez"]
QUARTERS = {"Q1": MONTHS[0:3], "Q2": MONTHS[3:6], "Q3": MONTHS[6:9], "Q4": MONTHS[9:12]}

# Bereich -> Abteilungen (2-5 each); the number of teams per Abteilung (2-5) is drawn below
BEREICHE = {
    "Vertrieb": ["Außendienst", "Innendienst", "Key Account", "Vertriebssteuerung"],
    "Marketing": ["Marketingkommunikation", "Produktmarketing", "Online-Marketing"],
    "Produktion": ["Fertigung", "Montage", "Qualitätssicherung", "Instandhaltung", "Arbeitsvorbereitung"],
    "Logistik": ["Lager", "Versand", "Transport"],
    "Einkauf": ["Strategischer Einkauf", "Operativer Einkauf"],
    "Entwicklung": ["Konstruktion", "Elektronikentwicklung", "Softwareentwicklung", "Versuch und Test"],
    "IT": ["Infrastruktur", "Anwendungen", "IT-Support", "Datenplattform"],
    "Personal": ["Personalbetreuung", "Recruiting", "Aus- und Weiterbildung"],
    "Finanzen": ["Rechnungswesen", "Controlling", "Treasury", "Steuern", "Beteiligungen"],
    "Kundenservice": ["Hotline", "Technischer Service"],
}
# Kostenart -> Kostenarten (Blätter)
KOSTENARTEN = {
    "Personalkosten": ["Gehälter", "Sozialabgaben"],
    "Sachkosten": ["Reisekosten", "Material"],
    "Gemeinkosten": ["Raumkosten", "Abschreibungen"],
}
SALARY = 5_200      # EUR per head and month
RENT = 480          # EUR per head and month
# per Bereich: factor on salaries, Reisekosten per head, Material per head, Abschreibungen per head
PROFILE = {
    "Vertrieb": (1.10, 900, 60, 120), "Marketing": (1.05, 350, 700, 80), "Produktion": (0.90, 60, 2_400, 650),
    "Logistik": (0.80, 80, 500, 300), "Einkauf": (1.05, 200, 80, 90), "Entwicklung": (1.30, 220, 450, 400),
    "IT": (1.25, 90, 180, 520), "Personal": (1.00, 120, 60, 70), "Finanzen": (1.15, 80, 40, 80),
    "Kundenservice": (0.85, 260, 130, 110),
}


def write_dim(path: Path, name: str, kind: str, rows: list[tuple[str, str]]):
    with open(path / f"{name}.csv", "w", newline="", encoding="utf-8") as f:
        w = csv.writer(f, delimiter=";")
        w.writerow(["child", "parent", "weight", "alias"])
        for child, parent in rows:
            w.writerow([child, parent, "", ""])
    (path / f"{name}.toml").write_text(f'kind = "{kind}"\nsubsets = []\n', encoding="utf-8")


def main(target: str) -> None:
    target_path = Path(target).resolve()
    if target_path.exists():
        sys.exit(f"{target_path} exists already; remove it first")
    rnd = random.Random(2026)

    # cost centre tree: Unternehmen > Bereich > Abteilung > Team
    stelle_rows = [("Unternehmen", "")]
    teams: list[tuple[str, str, int]] = []          # (team, bereich, headcount)
    n_abt = 0
    for bereich, abteilungen in BEREICHE.items():
        stelle_rows.append((bereich, "Unternehmen"))
        for abt in abteilungen:
            n_abt += 1
            stelle_rows.append((abt, bereich))
            for i in range(rnd.randint(2, 5)):
                team = f"{abt} {chr(ord('A') + i)}"
                stelle_rows.append((team, abt))
                teams.append((team, bereich, rnd.randint(4, 16)))

    art_rows = [("Gesamtkosten", "")]
    for gruppe, leaves in KOSTENARTEN.items():
        art_rows.append((gruppe, "Gesamtkosten"))
        art_rows += [(leaf, gruppe) for leaf in leaves]
    leaves = sum(KOSTENARTEN.values(), [])

    with tempfile.TemporaryDirectory() as tmp:
        model = Path(tmp) / "model"
        (model / "dimensions").mkdir(parents=True)
        (model / "cubes").mkdir()
        (model / "rules").mkdir()
        dims = model / "dimensions"
        write_dim(dims, "Jahr", "Time", [("Geschäftsjahr", "")]
                  + [(q, "Geschäftsjahr") for q in QUARTERS] + [(m, q) for q, ms in QUARTERS.items() for m in ms])
        write_dim(dims, "Kostenstelle", "Generic", stelle_rows)
        write_dim(dims, "Kostenart", "Generic", art_rows)
        # Plan and Ist must not add up, so all four are roots without a common parent
        write_dim(dims, "Version", "Generic",
                  [("Plan", ""), ("Ist", ""), ("Abweichung", ""), ("Abweichung in %", "")])
        (model / "cubes" / "BAB.toml").write_text(
            'dimensions = ["Jahr", "Kostenstelle", "Kostenart", "Version"]\n', encoding="utf-8")
        (model / "rules" / "BAB.rules").write_text(
            "['Abweichung'] = ['Ist'] - ['Plan'];\n"
            "['Abweichung in %'] = IF(['Plan'] = 0, NULL, ['Abweichung'] / ['Plan'] * 100);\n",
            encoding="utf-8")

        data = Path(tmp) / "werte.csv"
        rows = 0
        with open(data, "w", newline="", encoding="utf-8") as f:
            w = csv.writer(f, delimiter=";")
            w.writerow(["Monat", "Team", "Kostenart", "Version", "Wert"])
            for team, bereich, head in teams:
                sal, travel, material, depr = PROFILE[bereich]
                plan = {
                    "Gehälter": head * SALARY * sal,
                    "Sozialabgaben": head * SALARY * sal * 0.21,
                    "Reisekosten": head * travel,
                    "Material": head * material,
                    "Raumkosten": head * RENT,
                    "Abschreibungen": head * depr,
                }
                drift = rnd.uniform(0.97, 1.06)      # a team is consistently a bit over or under plan
                for mi, month in enumerate(MONTHS):
                    season = {"Reisekosten": 1 + 0.25 * math.sin((mi - 2) / 12 * 2 * math.pi),
                              "Material": 1 + 0.15 * math.sin((mi - 4) / 12 * 2 * math.pi)}
                    for art in leaves:
                        plan_value = plan[art] * season.get(art, 1.0)
                        ist_value = plan_value * drift * rnd.uniform(0.94, 1.07)
                        w.writerow([month, team, art, "Plan", round(plan_value)])
                        w.writerow([month, team, art, "Ist", round(ist_value)])
                        rows += 2
        (Path(tmp) / "laden.toml").write_text(
            f'''name = "BAB-Werte laden"
cube = "BAB"
value_column = "Wert"
source = {{ file = "{data.as_posix()}", delimiter = ";" }}

[map]
Jahr = "Monat"
Kostenstelle = "Team"
Kostenart = "Kostenart"
Version = "Version"

[options]
mode = "replace_area"
''', encoding="utf-8")

        env = dict(os.environ, NEOLAP_USER="setup")
        run = lambda *a: subprocess.run([NEOLAP, *a], check=True, capture_output=True, text=True, env=env)
        run("new", str(target_path))
        run("model", "import", str(target_path), str(model))
        run("load", str(target_path), str(Path(tmp) / "laden.toml"))
        subprocess.run([NEOLAP, "user", "add", str(target_path), "demo", "--role", "admin"],
                       check=True, capture_output=True, text=True,
                       env=dict(env, NEOLAP_PASSWORD="demo-passwort-123"))
    print(f"{len(BEREICHE)} Bereiche, {n_abt} Abteilungen, {len(teams)} Teams, {rows} Zellen")
    print(run("info", str(target_path)).stdout)
    print("Anmeldung: Benutzer 'demo', Passwort 'demo-passwort-123'")


if __name__ == "__main__":
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    main(sys.argv[1])
