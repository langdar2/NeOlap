"""NeOlap: a lightweight multidimensional database with write-back, rules and shared use.

    import neolap

    db = neolap.open("plan.neolap")                                  # a file, exclusive
    db = neolap.connect("https://olap.intern:8443", token="nlp_…")   # a server

    cube = db.cube("sales")
    cube["Jan", "North", "sedan", "Sales"] = 100.0                   # write a cell
    cube["Q1", "Total", "All", "Sales"] = 5000                       # an aggregate: splash, proportional
    cube.write(("Q1", "Total", "All", "Sales"), "#5000")             # splash, equal
    df = cube.area(years="Q1", regions="North").to_pandas()

The local and the remote database have the same API and raise the same exceptions.
"""

from . import _native
from ._cube import Area, Cube, Database
from ._remote import RemoteDatabase
from .errors import (
    AuthenticationError,
    ConflictError,
    ConnectionFailed,
    InputError,
    LockedError,
    NeolapError,
    NotFoundError,
    PermissionError,
    StorageError,
    TooManyRequests,
)

__version__ = _native.__version__

# `PermissionError` is deliberately not in `__all__`: `from neolap import *` must not hide the builtin.
# Use `neolap.PermissionError`.
__all__ = [
    "open",
    "create",
    "connect",
    "version",
    "Database",
    "Cube",
    "Area",
    "NeolapError",
    "NotFoundError",
    "ConflictError",
    "InputError",
    "LockedError",
    "StorageError",
    "ConnectionFailed",
    "AuthenticationError",
    "TooManyRequests",
]


def version() -> str:
    """The NeOlap version string."""
    return __version__


def open(path, *, user: str = "python") -> Database:
    """Opens a database file exclusively. `user` is the name recorded in the journal."""
    return Database(_native.LocalDatabase.open(str(path), user))


def create(path, *, user: str = "python") -> Database:
    """Creates a new, empty database file."""
    return Database(_native.LocalDatabase.create(str(path), user))


def connect(url: str, *, token: str, timeout: float = 60.0, verify: bool = True) -> Database:
    """Connects to a NeOlap server with an API token (see `neolap user token` or the admin page).

    A token is only sent over https, except to localhost. `verify=False` skips the certificate
    check (for a test server with a self-signed certificate); never use it over a network you
    do not control.
    """
    return Database(RemoteDatabase(url, token, timeout=timeout, verify=verify))
