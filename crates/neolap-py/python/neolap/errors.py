"""Exceptions of NeOlap.

The local backend (Rust) and the remote backend (HTTP) raise the *same* classes, so code that
catches `neolap.ConflictError` works with either. The classes are defined in the native module so
that the Rust side can raise them; the remote backend imports them from there.
"""

from neolap._native import (  # noqa: F401  (re-exported)
    ConflictError,
    InputError,
    LockedError,
    NeolapError,
    NotFoundError,
    PermissionError,
    StorageError,
)

__all__ = [
    "NeolapError",
    "NotFoundError",
    "ConflictError",
    "PermissionError",
    "InputError",
    "LockedError",
    "StorageError",
    "ConnectionFailed",
    "AuthenticationError",
    "TooManyRequests",
]


class ConnectionFailed(NeolapError):
    """The server could not be reached (remote backend)."""


class AuthenticationError(NeolapError):
    """The server did not accept the token (remote backend)."""


class TooManyRequests(NeolapError):
    """The server temporarily refuses a client after too many failed logins (remote backend). Wait and try again."""
