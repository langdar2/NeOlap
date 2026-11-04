"""Remote backend: the same operations as the native backend, over the REST API of a server.

Standard library only (`urllib`), so installing NeOlap pulls in no dependency. The token is sent in
the `Authorization` header and is never part of a `repr`, an exception message or a log line.
"""

from __future__ import annotations

import csv
import http.client
import io
import json
import socket
import ssl
import urllib.error
import urllib.parse
import urllib.request

from .errors import (
    AuthenticationError,
    ConflictError,
    ConnectionFailed,
    InputError,
    NotFoundError,
    PermissionError,
    StorageError,
    TooManyRequests,
)

_API = "/api/v1"


def _raise_for(status: int, body: dict | None) -> None:
    message = (body or {}).get("error") or f"HTTP {status}"
    code = (body or {}).get("code")
    if status == 401:
        raise AuthenticationError(message)
    if status == 403:
        raise PermissionError(message)
    if status == 404:
        raise NotFoundError(message)
    if status == 409:
        details = (body or {}).get("details") or {}
        exc = ConflictError(message)
        exc.user = details.get("user")
        exc.cell = details.get("cell")
        exc.value = details.get("value")
        raise exc
    if status == 429:
        raise TooManyRequests(message)
    if 400 <= status < 500:
        raise InputError(message)
    if status in (502, 503, 504):
        raise ConnectionFailed(f"the server is not available right now ({status}): {message}")
    raise StorageError(message)


class _Columns:
    """A long-format table as columns, built from CSV text (same shape as the native `Columns`).

    Which columns are numbers is decided per *column*, from the position: the dimension columns
    come first and are names, whatever they look like (a member called "2026" stays text); the
    columns after them are values. In a long table that is the last column, in a wide table every
    column after the dimensions.
    """

    def __init__(self, headers, rows, n_dimensions):
        self.headers = headers
        self.n_rows = len(rows)
        self._rows = rows
        self.numeric = [i >= n_dimensions for i in range(len(headers))]

    def value(self, row, i):
        text = self._rows[row][i]
        if not self.numeric[i]:
            return text
        return float(text) if text != "" else None

    def __len__(self):
        return self.n_rows


class RemoteDatabase:
    def __init__(self, url: str, token: str, *, timeout: float = 60.0, verify: bool = True):
        parsed = urllib.parse.urlparse(url)
        if parsed.scheme not in ("http", "https") or not parsed.netloc:
            raise InputError(f"'{url}' is not an http(s) URL")
        if parsed.scheme == "http" and parsed.hostname not in ("localhost", "127.0.0.1", "::1"):
            # a token over plain HTTP is readable by everybody on the way
            raise InputError("a token must not be sent over plain http to another machine; use https")
        self._base = f"{parsed.scheme}://{parsed.netloc}{_API}"
        # a token is printable ASCII without blanks: anything else cannot be a header value (and a
        # line break in it would let the caller smuggle in headers). Refused before anything is sent.
        if not isinstance(token, str) or not token or not token.isascii() or not token.isprintable() or any(c.isspace() for c in token):
            raise InputError("the token must be a non-empty string of printable ASCII characters without blanks (as printed by `neolap user token`)")
        self._token = token
        self._timeout = timeout
        self._ctx = None
        if parsed.scheme == "https" and not verify:
            self._ctx = ssl.create_default_context()
            self._ctx.check_hostname = False
            self._ctx.verify_mode = ssl.CERT_NONE
        # dimensions per cube, so a read is one request instead of two; dropped when the server
        # says "not found" (the model may have changed) and read once more
        self._dims: dict[str, list[str]] = {}

    def __repr__(self) -> str:
        return f"RemoteDatabase({self._base!r})"  # never the token

    def close(self) -> None:
        """Nothing to release: every call is its own HTTP request."""

    # ---- transport ----

    def _send(self, req):
        """Sends a request and returns the answer's body. Every way this can fail ends in a
        `neolap` exception: callers catch `NeolapError`, never urllib's or http.client's types."""
        try:
            with urllib.request.urlopen(req, timeout=self._timeout, context=self._ctx) as res:
                return res.read()
        except urllib.error.HTTPError as e:
            try:
                parsed = json.loads(e.read() or b"null")
            except (ValueError, http.client.HTTPException, OSError):
                parsed = None
            _raise_for(e.code, parsed if isinstance(parsed, dict) else None)
        except urllib.error.URLError as e:
            raise ConnectionFailed(f"cannot reach {self._base}: {e.reason}") from None
        except (TimeoutError, socket.timeout):
            raise ConnectionFailed(f"{self._base} did not answer within {self._timeout} s") from None
        except (http.client.HTTPException, ConnectionError, ssl.SSLError, OSError) as e:
            # a cut-off answer, a hang-up, something that is not HTTP at all
            raise ConnectionFailed(f"the answer of {self._base} could not be read: {type(e).__name__}") from None

    def _request(self, method, path, body=None, *, raw=False):
        data = None
        headers = {"Authorization": f"Bearer {self._token}", "Accept": "application/json"}
        if body is not None:
            data = json.dumps(body).encode()
            headers["Content-Type"] = "application/json"
        payload = self._send(urllib.request.Request(self._base + path, data=data, method=method, headers=headers))
        if raw:
            return payload
        if not payload:
            return None
        try:
            return json.loads(payload)
        except ValueError:
            raise ConnectionFailed(f"{self._base} answered with something that is not JSON") from None

    @staticmethod
    def _q(**params):
        items = {k: v for k, v in params.items() if v not in (None, "")}
        return ("?" + urllib.parse.urlencode(items)) if items else ""

    @staticmethod
    def _enc(name):
        return urllib.parse.quote(name, safe="")

    # ---- the operations of the backend interface ----

    def _cube_info(self, cube):
        self._model = self._request("GET", "/model")
        for c in self._model["cubes"]:
            if c["name"].lower() == cube.lower():
                return c
        raise NotFoundError(f"cube '{cube}' not found")

    def cubes(self):
        return [c["name"] for c in self._request("GET", "/model")["cubes"]]

    def dimensions(self, cube):
        dims = list(self._cube_info(cube)["dimensions"])
        self._dims[cube.lower()] = dims
        return dims

    def _dimensions_cached(self, cube):
        dims = self._dims.get(cube.lower())
        return dims if dims is not None else self.dimensions(cube)

    def generation(self, cube):
        return int(self._cube_info(cube)["generation"])

    def get(self, cube, members, rules=True):
        for attempt in (0, 1):
            dims = self._dimensions_cached(cube) if attempt == 0 else self.dimensions(cube)
            if len(members) != len(dims):
                if attempt == 0 and cube.lower() in self._dims:
                    continue  # the cached shape may be stale: look once more before refusing
                raise InputError(f"cube '{cube}' has {len(dims)} dimensions, got {len(members)} members")
            params = dict(zip(dims, members))
            if not rules:
                params["opt.rules"] = "false"  # prefixed, so it can never be taken for a dimension
            try:
                r = self._request("GET", f"/cubes/{self._enc(cube)}/cell{self._q(**params)}")
            except NotFoundError:
                if attempt == 0:
                    self._dims.pop(cube.lower(), None)
                    continue
                raise
            break
        v = r["value"]
        if isinstance(v, str):
            raise InputError(f"the cell is {v}" if v.startswith("#") else f"the cell holds the text '{v}'")
        return v

    def write(self, cube, members, value, expected_generation=None):
        body = {"cell": list(members), "value": value}
        if expected_generation is not None:
            body["generation"] = expected_generation
        r = self._request("POST", f"/cubes/{self._enc(cube)}/write", body)
        return r["cells_changed"], r["generation"]

    def undo(self, cube):
        r = self._request("POST", f"/cubes/{self._enc(cube)}/undo", {})
        return r["cells_changed"], r["generation"]

    def area(self, cube, area, aggregates=False, rules=False, wide=None):
        a = ",".join(f"{d}={'|'.join(m)}" for d, m in area)
        q = self._q(area=a, aggregates="true" if aggregates else None, rules="true" if rules else None, wide=wide, format="csv")
        text = self._request("GET", f"/cubes/{self._enc(cube)}/export{q}", raw=True).decode("utf-8")
        rows = list(csv.reader(io.StringIO(text)))
        if not rows:
            return _Columns([], [], 0)
        # long: every dimension is a column and one value column follows; wide: the dimensions
        # except the wide one, then one column per member of the wide dimension
        n_dims = len(self.dimensions(cube)) - (1 if wide else 0)
        return _Columns(rows[0], rows[1:], n_dims)

    def load(self, spec, data, kind="csv", dry_run=False):
        import time
        boundary = "----neolap" + str(time.time_ns())
        name = "data.xlsx" if kind == "xlsx" else "data.csv"
        parts = []
        for field, value in (("spec", spec), ("dry_run", "true" if dry_run else "false")):
            parts.append(f'--{boundary}\r\nContent-Disposition: form-data; name="{field}"\r\n\r\n{value}\r\n'.encode())
        parts.append(f'--{boundary}\r\nContent-Disposition: form-data; name="file"; filename="{name}"\r\nContent-Type: application/octet-stream\r\n\r\n'.encode() + data + b"\r\n")
        parts.append(f"--{boundary}--\r\n".encode())
        req = urllib.request.Request(
            self._base + "/imports", data=b"".join(parts), method="POST",
            headers={"Authorization": f"Bearer {self._token}", "Content-Type": f"multipart/form-data; boundary={boundary}"},
        )
        try:
            job = json.loads(self._send(req))["job"]
        except (ValueError, KeyError, TypeError):
            raise ConnectionFailed(f"{self._base} did not start the import properly") from None
        delay = 0.05
        for _ in range(2000):
            j = self._request("GET", f"/jobs/{job}")
            if j["status"] != "running":
                report = j.get("result") or {}
                if not report:
                    raise InputError(j.get("error") or "the import failed")
                report["rejected"] = [(r["line"], r["reason"]) for r in report.get("rejected", [])]
                return report
            time.sleep(delay)
            delay = min(delay * 1.5, 1.0)
        raise ConnectionFailed("the import did not finish in time")

    def rules(self, cube):
        return self._request("GET", f"/cubes/{self._enc(cube)}/rules")["text"]

    def backup(self, target):
        raise InputError("a backup of a remote database is made on the server (Admin: POST /api/v1/admin/backup)")
