"""The user-facing API, shared by the local and the remote backend.

A backend is any object with the methods `cubes`, `dimensions`, `generation`, `get`, `write`,
`undo`, `area`, `load`, `rules` and `backup` (see `_remote.RemoteDatabase` and the native
`LocalDatabase`). Nothing here knows which one it has, so both behave the same by construction.
"""

from __future__ import annotations

import math
import os
from decimal import Decimal
from typing import Any, Iterable, Mapping, Sequence

from ._frame import to_pandas
from .errors import InputError, NotFoundError


def _members(value: Any, what: str) -> list[str]:
    """One member name or a list of them → a list of names."""
    if isinstance(value, str):
        return [value]
    if isinstance(value, Iterable) and not isinstance(value, (bytes, Mapping)):
        names = [str(v) if not isinstance(v, str) else v for v in value]
        if not names:
            raise InputError(f"{what}: the list of members is empty")
        return names
    raise InputError(f"{what}: expected a member name or a list of names, not {type(value).__name__}")


def _input_text(value: Any) -> str:
    """A Python value → the input syntax of the server.

    Numbers are written with full precision (`repr`), so no digit is lost. Strings are the splash
    syntax as typed (`"#5000"`, `"+5%"`, `"@Actual:100"`); `None` or `""` deletes the cell.
    """
    if value is None:
        return ""
    if isinstance(value, bool):
        raise InputError("a cell value must be a number or a string in the splash syntax, not a bool")
    if isinstance(value, (int, float)) or type(value).__module__ == "numpy" and hasattr(value, "item"):
        number = float(value)
        if not math.isfinite(number):
            raise InputError(f"a cell value must be a finite number, not {value!r}")
        # The server's number reader knows no exponent ("1e-07"), so the shortest exact digits are
        # written out in plain positional form. This loses nothing: float() of the text is `number`.
        text = format(Decimal(repr(number)), "f")
        if "." in text:
            text = text.rstrip("0").rstrip(".")
        if text in ("", "-"):
            text = "0"
        # '=' makes a negative number an absolute value (a leading '-' alone means "subtract")
        return "=" + text if number < 0 else text
    if isinstance(value, str):
        return value
    raise InputError(f"a cell value must be a number or a string in the splash syntax, not {type(value).__name__}")


class Area:
    """A selection of cells of a cube, read when asked for (`to_pandas`, `values`, `rows`)."""

    def __init__(self, cube: "Cube", selection: Mapping[str, Sequence[str]], *, aggregates: bool, rules: bool, wide: str | None):
        self._cube = cube
        self._selection = [(d, list(m)) for d, m in selection.items()]
        self._aggregates = aggregates
        self._rules = rules
        self._wide = wide
        self._columns = None

    def __repr__(self) -> str:
        sel = ", ".join(f"{d}={'|'.join(m)}" for d, m in self._selection) or "everything"
        return f"Area({self._cube.name!r}: {sel})"

    def _load(self):
        if self._columns is None:
            self._columns = self._cube._db._backend.area(
                self._cube.name, self._selection, self._aggregates, self._rules, self._wide
            )
        return self._columns

    def to_pandas(self):
        """The area as a `DataFrame`: one row per filled cell, dimensions as categories."""
        return to_pandas(self._load())

    @property
    def columns(self) -> list[str]:
        return list(self._load().headers)

    def __len__(self) -> int:
        return len(self._load())

    def rows(self) -> list[tuple]:
        """The area as a list of tuples (member names, then the value). Prefer `to_pandas` for large areas."""
        cols = self._load()
        if hasattr(cols, "buffer"):
            import struct

            out = []
            decoded = []
            for i in range(len(cols.headers)):
                buf = cols.buffer(i)
                if cols.is_numeric(i):
                    n = len(buf) // 8
                    decoded.append([None if math.isnan(v) else v for v in struct.unpack(f"<{n}d", buf)])
                else:
                    n = len(buf) // 4
                    names = cols.names(i)
                    decoded.append([names[c] for c in struct.unpack(f"<{n}I", buf)])
            return list(zip(*decoded)) if decoded else []
        return [tuple(cols.value(r, i) for i in range(len(cols.headers))) for r in range(len(cols))]


class Cube:
    """One cube. Cells are addressed by one member name per dimension, in the order of `dimensions`."""

    def __init__(self, db: "Database", name: str, dimensions: list[str]):
        self._db = db
        self.name = name
        self.dimensions = dimensions

    def __repr__(self) -> str:
        return f"Cube({self.name!r}, dimensions={self.dimensions})"

    def _address(self, key: Any) -> list[str]:
        members = list(key) if isinstance(key, tuple) else [key]
        if len(members) != len(self.dimensions):
            raise InputError(
                f"cube '{self.name}' has {len(self.dimensions)} dimensions ({', '.join(self.dimensions)}), "
                f"the address has {len(members)} members"
            )
        for d, m in zip(self.dimensions, members):
            if not isinstance(m, str):
                raise InputError(f"the member for '{d}' must be a name (str), not {type(m).__name__}")
        return members

    # ---- single cells ----

    def __getitem__(self, key) -> float | None:
        """The value of a cell (base or aggregate, rules applied), or `None` if it is empty."""
        return self.get(key)

    def __setitem__(self, key, value) -> None:
        """Writes a cell. On an aggregate the value is spread over its base cells (proportional)."""
        self.write(key, value)

    def get(self, key, *, rules: bool = True) -> float | None:
        return self._db._backend.get(self.name, self._address(key), rules)

    def write(self, key, value, *, expected_generation: int | None = None) -> int:
        """Writes `value` (a number, or text in the splash syntax) and returns the number of cells changed.

        Text forms: `"500"` proportional, `"#500"` equal, `"+50"` / `"-50"` difference, `"+5%"` / `"-3%"`
        percent, `"@Actual:500"` after the structure of another area, `""` or `None` deletes.
        With `expected_generation` the write fails with `ConflictError` if a cell of the area was
        changed since the cube had that generation (see `generation`).
        """
        changed, _ = self._db._backend.write(self.name, self._address(key), _input_text(value), expected_generation)
        return changed

    def undo(self) -> int:
        """Reverts this user's last operation; returns the cells changed. Fails with `ConflictError` if someone changed them since."""
        changed, _ = self._db._backend.undo(self.name)
        return changed

    @property
    def generation(self) -> int:
        """Counter that grows with every change; pass it as `expected_generation` to detect conflicts."""
        return self._db._backend.generation(self.name)

    # ---- areas ----

    def area(self, *, aggregates: bool = False, rules: bool = False, wide: str | None = None, **selection) -> Area:
        """Selects cells: `cube.area(years="Q1", regions=["North", "South"])`.

        Dimensions that are not named give all their base members (all members with `aggregates=True`).
        `rules=True` calculates rule cells too; `wide="measures"` makes the members of that dimension columns.
        """
        known = {d.lower(): d for d in self.dimensions}
        chosen: dict[str, list[str]] = {}
        for dim, value in selection.items():
            if dim.lower() not in known:
                raise InputError(f"'{dim}' is not a dimension of cube '{self.name}' ({', '.join(self.dimensions)})")
            chosen[known[dim.lower()]] = _members(value, dim)
        if wide is not None and wide.lower() not in known:
            raise InputError(f"wide='{wide}' is not a dimension of cube '{self.name}'")
        return Area(self, chosen, aggregates=aggregates, rules=rules, wide=known[wide.lower()] if wide else None)

    def rules(self) -> str:
        """The rule text of the cube."""
        return self._db._backend.rules(self.name)

    # ---- loading ----

    def load(self, source, *, mapping: Mapping[str, str] | None = None, values: Mapping[str, str] | None = None,
             value_column: str | None = None, fixed: Mapping[str, str] | None = None, mode: str = "overwrite",
             unknown_member: str = "reject", decimal: str = ".", delimiter: str = ",", sheet: str | None = None,
             spec: str | None = None, dry_run: bool = False) -> dict:
        """Loads data as one operation and returns the report (a dict).

        `source` is a pandas `DataFrame`, a path to a `.csv`/`.xlsx` file, or `bytes`. Describe the
        columns either with `mapping` (dimension → column) plus `values` (member of the measures
        dimension → column, for one column per measure) or `value_column` (one column holds the
        number), and `fixed` (dimension → one member for the whole file); or pass a complete
        import specification (TOML text) as `spec`.

        `mode`: `"overwrite"`, `"add"` or `"replace_area"` (deletes exactly the area the data
        covers first, so loading the same data twice equals loading it once). With `dry_run=True`
        nothing is changed and the report shows what would happen. Rows that cannot be loaded are
        listed in `report["rejected"]`; with the default `unknown_member="reject"` one bad row
        stops the whole load (`report["failed"]` says why).
        """
        kind, data = _source_bytes(source, decimal, delimiter)
        if spec is None:
            spec = _build_spec(self, mapping, values, value_column, fixed, mode, unknown_member, decimal if kind == "csv" and not _is_frame(source) else ".", delimiter if kind == "csv" and not _is_frame(source) else ",", sheet)
        report = self._db._backend.load(spec, data, kind, dry_run)
        if report.get("failed"):
            lines = "; ".join(f"line {n}: {why}" for n, why in report.get("rejected", [])[:5])
            raise InputError(f"{report['failed']}" + (f" ({lines})" if lines else ""))
        return report


def _is_frame(source) -> bool:
    return type(source).__module__.startswith("pandas") and hasattr(source, "to_csv")


def _source_bytes(source, decimal: str, delimiter: str) -> tuple[str, bytes]:
    if _is_frame(source):
        # a DataFrame is handed over as CSV with '.' as decimal and ',' as delimiter, whatever the caller's settings
        return "csv", source.to_csv(index=False, na_rep="").encode("utf-8")
    if isinstance(source, (bytes, bytearray)):
        return "csv", bytes(source)
    if isinstance(source, (str, os.PathLike)):
        path = os.fspath(source)
        if not os.path.isfile(path):
            raise InputError(f"no such file: {path}")
        with open(path, "rb") as f:
            return ("xlsx" if path.lower().endswith((".xlsx", ".xlsm")) else "csv"), f.read()
    raise InputError(f"cannot load from {type(source).__name__}: pass a DataFrame, a file path or bytes")


def _toml(text: str) -> str:
    """A TOML basic string."""
    return '"' + text.replace("\\", "\\\\").replace('"', '\\"').replace("\n", "\\n").replace("\r", "\\r").replace("\t", "\\t") + '"'


def _build_spec(cube, mapping, values, value_column, fixed, mode, unknown_member, decimal, delimiter, sheet) -> str:
    if mode not in ("overwrite", "add", "replace_area"):
        raise InputError("mode must be 'overwrite', 'add' or 'replace_area'")
    if not mapping and not fixed:
        raise InputError("describe the columns: mapping={dimension: column, ...} (and values= or value_column=), or pass spec=")
    if bool(values) == bool(value_column):
        raise InputError("give exactly one of values={member: column} and value_column=column")
    known = {d.lower(): d for d in cube.dimensions}

    def dim(name: str) -> str:
        if name.lower() not in known:
            raise InputError(f"'{name}' is not a dimension of cube '{cube.name}' ({', '.join(cube.dimensions)})")
        return known[name.lower()]

    src = [f"delimiter = {_toml(delimiter)}", f"decimal = {_toml(decimal)}"]
    if sheet:
        src.append(f"sheet = {_toml(sheet)}")
    lines = ["name = " + _toml(f"python load into {cube.name}"), "cube = " + _toml(cube.name), "source = { " + ", ".join(src) + " }"]
    if value_column:
        lines.append("value_column = " + _toml(value_column))
    lines.append("\n[map]")
    for d, col in (mapping or {}).items():
        lines.append(f"{_toml(dim(d))} = {_toml(col)}")
    if fixed:
        lines.append("\n[fixed]")
        for d, m in fixed.items():
            lines.append(f"{_toml(dim(d))} = {_toml(m)}")
    if values:
        lines.append("\n[values]")
        for member, col in values.items():
            lines.append(f"{_toml(member)} = {_toml(col)}")
    lines.append("\n[options]")
    lines.append(f"mode = {_toml(mode)}")
    lines.append(f"unknown_member = {_toml(unknown_member)}")
    return "\n".join(lines) + "\n"


class Database:
    """An open database, local (a file) or remote (a server). Same API either way."""

    def __init__(self, backend):
        self._backend = backend
        self._cubes: dict[str, Cube] = {}

    def __repr__(self) -> str:
        remote = type(self._backend).__name__ == "RemoteDatabase"
        return repr(self._backend) if remote else "Database(local file)"

    @property
    def cubes(self) -> list[str]:
        """Names of the cubes."""
        return list(self._backend.cubes())

    def cube(self, name: str) -> Cube:
        """The cube called `name` (case-insensitive). Raises `NotFoundError` if there is none."""
        for existing in self._backend.cubes():
            if existing.lower() == name.lower():
                dims = list(self._backend.dimensions(existing))
                return Cube(self, existing, dims)
        raise NotFoundError(f"cube '{name}' not found (available: {', '.join(self._backend.cubes()) or 'none'})")

    def __getitem__(self, name: str) -> Cube:
        return self.cube(name)

    def backup(self, target) -> None:
        """Writes a consistent copy of a local database; the original stays usable."""
        self._backend.backup(str(target))

    def close(self) -> None:
        """Releases the file (local) or does nothing (remote). The object cannot be used afterwards."""
        self._backend.close()  # a closed backend ignores this, so closing twice is harmless
        self._backend = _Closed()

    def __enter__(self) -> "Database":
        return self

    def __exit__(self, *exc) -> None:
        self.close()


class _Closed:
    """Stands in for the backend of a closed database: everything but `close` is an error."""

    def close(self) -> None:
        """Already closed."""

    def __getattr__(self, name):
        raise InputError("the database is closed")
