#!/usr/bin/env python3
"""Builds the "Umsatzplanung" example database used in the README screenshots.

    python3 examples/build_planning_demo.py planung.neolap
    neolap serve --data-dir . --database planung.neolap

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
REGIONS = {"Nord": ["Hamburg", "Berlin"], "Süd": ["München", "Stuttgart", "Zürich"]}
PRODUCTS = {
    "Fahrräder": ["Stadtrad", "E-Bike", "Rennrad"],
    "Zubehör": ["Helm", "Schloss", "Licht"],
}
# base level of the monthly revenue per product (EUR) and a seasonal factor
LEVEL = {"Stadtrad": 42_000, "E-Bike": 88_000, "Rennrad": 30_000, "Helm": 9_000, "Schloss": 6_000, "Licht": 4_500}
REGION_FACTOR = {"Hamburg": 1.0, "Berlin": 1.25, "München": 1.4, "Stuttgart": 0.9, "Zürich": 0.75}
COST_RATIO = {"Stadtrad": 0.71, "E-Bike": 0.78, "Rennrad": 0.69, "Helm": 0.52, "Schloss": 0.48, "Licht": 0.5}


def write_dim(path: Path, name: str, kind: str, root: str, tree: dict[str, list[str]] | None, extra=()):
    rows = [(root, "", "")]
    for group, leaves in (tree or {}).items():
        rows.append((group, root, ""))
        rows += [(leaf, group, "") for leaf in leaves]
    rows += list(extra)
    with open(path / f"{name}.csv", "w", newline="", encoding="utf-8") as f:
        w = csv.writer(f, delimiter=";")
        w.writerow(["child", "parent", "weight", "alias"])
        for child, parent, weight in rows:
            w.writerow([child, parent, weight, ""])
    (path / f"{name}.toml").write_text(f'kind = "{kind}"\nsubsets = []\n', encoding="utf-8")


def main(target: str) -> None:
    target_path = Path(target).resolve()
    if target_path.exists():
        sys.exit(f"{target_path} exists already; remove it first")
    with tempfile.TemporaryDirectory() as tmp:
        model = Path(tmp) / "model"
        (model / "dimensions").mkdir(parents=True)
        (model / "cubes").mkdir()
        (model / "rules").mkdir()
        write_dim(model / "dimensions", "Jahr", "Time", "Geschäftsjahr", QUARTERS)
        write_dim(model / "dimensions", "Region", "Generic", "Alle Regionen", REGIONS)
        write_dim(model / "dimensions", "Produkt", "Generic", "Sortiment", PRODUCTS)
        # Gewinn = Umsatz - Kosten (Kosten mit Gewicht -1); Marge ist eine Regel
        write_dim(model / "dimensions", "Kennzahl", "Measures", "Gewinn", None,
                  extra=[("Umsatz", "Gewinn", ""), ("Kosten", "Gewinn", "-1"), ("Marge in %", "", "")])
        (model / "cubes" / "Planung.toml").write_text(
            'dimensions = ["Jahr", "Region", "Produkt", "Kennzahl"]\n', encoding="utf-8")
        (model / "rules" / "Planung.rules").write_text(
            "['Marge in %'] = IF(['Umsatz'] = 0, NULL, ['Gewinn'] / ['Umsatz'] * 100);\n", encoding="utf-8")

        rnd = random.Random(2026)
        data = Path(tmp) / "werte.csv"
        with open(data, "w", newline="", encoding="utf-8") as f:
            w = csv.writer(f, delimiter=";")
            w.writerow(["Monat", "Region", "Produkt", "Kennzahl", "Wert"])
            for pi, product in enumerate(sum(PRODUCTS.values(), [])):
                for region in sum(REGIONS.values(), []):
                    for mi, month in enumerate(MONTHS):
                        # bikes peak in spring/summer, accessories follow with a delay
                        season = 1 + 0.45 * math.sin((mi - 1 - (pi >= 3)) / 12 * 2 * math.pi)
                        sales = LEVEL[product] * REGION_FACTOR[region] * season * rnd.uniform(0.92, 1.08)
                        cost = sales * COST_RATIO[product] * rnd.uniform(0.97, 1.03)
                        w.writerow([month, region, product, "Umsatz", round(sales)])
                        w.writerow([month, region, product, "Kosten", round(cost)])
        (Path(tmp) / "laden.toml").write_text(
            f'''name = "Planwerte laden"
cube = "Planung"
value_column = "Wert"
source = {{ file = "{data.as_posix()}", delimiter = ";" }}

[map]
Jahr = "Monat"
Region = "Region"
Produkt = "Produkt"
Kennzahl = "Kennzahl"

[options]
mode = "replace_area"
''', encoding="utf-8")

        # every change is recorded in the journal under this name, not under the login of the machine
        env = dict(os.environ, NEOLAP_USER="setup")
        run = lambda *a: subprocess.run([NEOLAP, *a], check=True, capture_output=True, text=True, env=env)
        run("new", str(target_path))
        run("model", "import", str(target_path), str(model))
        run("load", str(target_path), str(Path(tmp) / "laden.toml"))
        subprocess.run([NEOLAP, "user", "add", str(target_path), "demo", "--role", "admin"],
                       check=True, capture_output=True, text=True,
                       env=dict(env, NEOLAP_PASSWORD="demo-passwort-123"))
    print(run("info", str(target_path)).stdout)
    print("Anmeldung: Benutzer 'demo', Passwort 'demo-passwort-123'")


if __name__ == "__main__":
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    main(sys.argv[1])
