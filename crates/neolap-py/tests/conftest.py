"""Fixtures: the same tests run against the local backend (a file) and the remote one (a real server).

The server is the real `neolap serve` binary on a free port, with an API token, so the remote
tests go through HTTP, authentication and the REST API exactly as a user would.
"""

import os
import shutil
import socket
import subprocess
import time
from pathlib import Path

import pytest

import neolap

ROOT = Path(__file__).resolve().parents[3]
BINARY = ROOT / "target" / "debug" / "neolap"


def _free_port() -> int:
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def _demo(path: Path) -> None:
    subprocess.run([str(BINARY), "demo", "tiny", str(path)], check=True, capture_output=True)


@pytest.fixture(scope="session")
def binary():
    if not BINARY.exists():
        pytest.skip(f"build the CLI first: cargo build -p neolap-cli ({BINARY})")
    return BINARY


@pytest.fixture
def local(tmp_path, binary):
    path = tmp_path / "plan.neolap"
    _demo(path)
    db = neolap.open(path, user="tester")
    yield db
    db.close()


@pytest.fixture(scope="session")
def big_cube_file(tmp_path_factory, binary):
    """3,000 products x 40 periods = 120,000 cells: one read of it takes long enough to measure."""
    import random

    d = tmp_path_factory.mktemp("big")
    (d / "m/dimensions").mkdir(parents=True)
    (d / "m/cubes").mkdir()
    rows = ["child;parent;weight;alias", "All;;;"] + [f"p{g}_{p};All;;" for g in range(30) for p in range(100)]
    (d / "m/dimensions/products.csv").write_text("\n".join(rows) + "\n")
    (d / "m/dimensions/periods.csv").write_text("\n".join(["child;parent;weight;alias", "Year;;;"] + [f"m{m};Year;;" for m in range(1, 41)]) + "\n")
    (d / "m/dimensions/measures.csv").write_text("child;parent;weight;alias\nSales;;;\n")
    (d / "m/dimensions/measures.toml").write_text('kind = "Measures"\n')
    (d / "m/cubes/plan.toml").write_text('dimensions = ["products", "periods", "measures"]\n')
    random.seed(1)
    cells = ["products;periods;measures;value"] + [f"p{g}_{p};m{m};Sales;{random.randint(1, 99)}" for g in range(30) for p in range(100) for m in range(1, 41)]
    (d / "cells.csv").write_text("\n".join(cells) + "\n")
    (d / "spec.toml").write_text('name="x"\ncube="plan"\nvalue_column="value"\nsource={file="cells.csv",delimiter=";"}\n[map]\nproducts="products"\nperiods="periods"\nmeasures="measures"\n')
    db = d / "big.neolap"
    for args in (["new", str(db)], ["model", "import", str(db), str(d / "m")], ["load", str(db), str(d / "spec.toml")]):
        subprocess.run([str(binary), *args], check=True, capture_output=True)
    return db


class Server:
    def __init__(self, process, url, token, path):
        self.process, self.url, self.token, self.path = process, url, token, path


@pytest.fixture
def server(tmp_path, binary):
    data = tmp_path / "data"
    data.mkdir()
    db_path = data / "neolap.neolap"
    _demo(db_path)
    env = dict(os.environ, NEOLAP_PASSWORD="a very long secret")
    subprocess.run([str(binary), "user", "add", str(db_path), "tester", "--role", "admin"], check=True, capture_output=True, env=env)
    token = subprocess.run([str(binary), "user", "token", str(db_path), "tester"], check=True, capture_output=True, env=env, text=True).stdout.strip()
    port = _free_port()
    proc = subprocess.Popen([str(binary), "serve", "--listen", f"127.0.0.1:{port}", "--data-dir", str(data)], stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, env={k: v for k, v in os.environ.items() if not k.startswith("NEOLAP_")})
    url = f"http://127.0.0.1:{port}"
    for _ in range(100):
        try:
            with socket.create_connection(("127.0.0.1", port), timeout=0.2):
                break
        except OSError:
            if proc.poll() is not None:
                raise RuntimeError("the server did not start: " + proc.stderr.read().decode())
            time.sleep(0.1)
    yield Server(proc, url, token, db_path)
    proc.terminate()
    try:
        proc.wait(timeout=10)
    except subprocess.TimeoutExpired:
        proc.kill()


@pytest.fixture
def remote(server):
    return neolap.connect(server.url, token=server.token)


@pytest.fixture(params=["local", "remote"])
def db(request):
    """The database under test: every test that takes `db` runs once per backend."""
    return request.getfixturevalue(request.param)
