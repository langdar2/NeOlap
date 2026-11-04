"""Guards over the sources of the native module, for rules that tests of behaviour cannot see cheaply."""

import re
from pathlib import Path

SRC = (Path(__file__).resolve().parents[1] / "src" / "lib.rs").read_text()

# methods of `LocalDatabase` that call into the engine for more than a field read
WORKERS = ["open", "create", "get", "write", "undo", "area", "load", "backup", "close"]


def _method(name: str) -> str:
    m = re.search(rf"    fn {name}\(", SRC)
    assert m, f"no method {name}"
    nxt = re.search(r"\n    (?:#\[|fn |/// )", SRC[m.end():])
    return SRC[m.start(): m.end() + (nxt.start() if nxt else len(SRC))]


def test_every_method_that_works_in_the_engine_releases_the_gil():
    # rustfmt may put `py` and `.detach(` on separate lines, so look for the call, not the spelling
    releases = lambda body: re.search(r"\bpy\s*\.detach\s*\(", body) is not None  # noqa: E731
    missing = [n for n in WORKERS if not releases(_method(n))]
    assert missing == [], f"these methods hold the interpreter lock while the engine works: {missing}"


def test_no_engine_error_is_turned_into_a_bare_runtime_error():
    # errors go through `err`, so users can catch NotFoundError, ConflictError, ...
    assert "RuntimeError" not in SRC
    assert "PyException::new_err" not in SRC
    assert SRC.count("map_err(err)") >= 9


def test_no_unwrap_or_expect_on_user_data_in_the_binding():
    code = "\n".join(l for l in SRC.splitlines() if not l.strip().startswith("//"))
    assert ".unwrap()" not in code, "an unwrap in the binding would abort the Python process on bad input"
