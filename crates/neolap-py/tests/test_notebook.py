"""The example notebook is run as a test, in both modes, so it cannot rot when the API changes."""

import os
import re
import shutil
import socket
import subprocess
import time
from pathlib import Path

import pytest

nbformat = pytest.importorskip("nbformat")
nbclient = pytest.importorskip("nbclient")
pytest.importorskip("pandas")

NOTEBOOK = Path(__file__).resolve().parents[1] / "examples" / "tiny.ipynb"


def _run(env_extra, path_dirs):
    nb = nbformat.read(NOTEBOOK, as_version=4)
    old = {k: os.environ.get(k) for k in list(env_extra) + ["PATH"]}
    os.environ.update(env_extra)
    os.environ["PATH"] = os.pathsep.join([*path_dirs, os.environ["PATH"]])
    try:
        nbclient.NotebookClient(nb, timeout=120, kernel_name="python3", resources={"metadata": {"path": str(NOTEBOOK.parent)}}).execute()
    finally:
        for k, v in old.items():
            if v is None:
                os.environ.pop(k, None)
            else:
                os.environ[k] = v
    out = []
    for cell in nb.cells:
        if cell.cell_type == "code":
            # The kernel flushes stdout about every 0.2 s, so one cell's prints can arrive as one
            # block or as several, depending on how fast the machine is. Blocks of the same stream
            # are therefore joined as they are (the line breaks are already in the text), or the
            # number of blank lines would depend on timing.
            merged = []
            for o in cell.outputs:
                text = o.get("text") or o.get("data", {}).get("text/plain") or ""
                if merged and o.get("output_type") == "stream" and merged[-1][0] == o.get("name"):
                    merged[-1][1] += text
                else:
                    merged.append([o.get("name") if o.get("output_type") == "stream" else None, text])
            out.extend(text for _, text in merged)
    return "\n".join(str(x) for x in out)


def _check(text):
    # key numbers of the tutorial, so "runs without error" cannot hide wrong results
    assert "Basiszelle   : 1000.0" in text
    assert "Quartal       : 33840.0" in text
    assert "Jahr, Gewinn  : 57960.0" in text
    assert "Marge (Regel) : 0.35" in text
    assert "Quartal vorher/nachher: 33840.0 -> 34840.0" in text
    assert "24 Zellen geändert; Quartal jetzt 100000.0" in text
    assert "jede Zelle: 1000.0" in text
    assert "Quartal +10 %: 26400.0" in text
    assert "wieder wie am Anfang: 33840.0" in text
    assert re.search(r"Konflikt: \w+ hat Feb, North, sedan, Sales auf 1.0 gesetzt", text)
    assert "Vorschau : {'rows_read': 3, 'cells_updated': 6, 'cells_created': 0, 'dry_run': True}" in text
    assert "nochmal  : {'cells_created': 0, 'cells_updated': 0, 'cells_deleted': 0}" in text
    assert "unknown member 'Smarch'" in text and "unverändert: 2000.0" in text
    assert "fertig" in text


def test_the_notebook_runs_on_a_local_file(binary):
    _check(_run({"NEOLAP_NOTEBOOK_MODE": "local"}, [str(binary.parent)]))


def test_the_notebook_runs_against_a_server_and_gives_the_same_output(binary, server):
    remote = _run({"NEOLAP_NOTEBOOK_MODE": "remote", "NEOLAP_URL": server.url, "NEOLAP_TOKEN": server.token}, [str(binary.parent)])
    _check(remote)
    local = _run({"NEOLAP_NOTEBOOK_MODE": "local"}, [str(binary.parent)])
    # identical but for the name of the user in the conflict message
    norm = lambda t: re.sub(r"Konflikt: \w+ hat", "Konflikt: USER hat", t)  # noqa: E731
    assert norm(remote) == norm(local)
    assert server.token not in remote


def test_the_notebook_is_committed_without_outputs():
    nb = nbformat.read(NOTEBOOK, as_version=4)
    assert all(not c.get("outputs") and c.get("execution_count") is None for c in nb.cells if c.cell_type == "code")
