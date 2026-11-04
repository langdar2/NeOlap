"""What only one backend can do or has to guard against."""

import gc
import threading
import time

import pytest

import neolap


# ---------------------------------------------------------------- local file

def test_a_file_is_exclusive_until_it_is_closed(tmp_path, binary, local):
    # `local` holds the file open: a second open must be refused with a clear exception
    path = next(tmp_path.glob("*.neolap"))
    with pytest.raises(neolap.LockedError, match="in use by another process"):
        neolap.open(path)
    local.close()
    again = neolap.open(path)  # and works once the first one is closed
    assert again.cube("sales")["Year", "Total", "All", "Sales"] == 165600.0
    again.close()


def test_a_closed_database_fails_clearly_instead_of_crashing(tmp_path, binary):
    path = tmp_path / "c.neolap"
    db = neolap.create(path)
    db.close()
    with pytest.raises(neolap.InputError, match="closed"):
        db.cubes
    db.close()  # closing twice is harmless


def test_with_block_releases_the_file(tmp_path, binary):
    path = tmp_path / "w.neolap"
    with neolap.create(path) as db:
        assert db.cubes == []
    with neolap.open(path) as db:  # not locked any more
        assert db.cubes == []


def test_a_cube_object_of_a_closed_database_fails_clearly(local):
    c = local.cube("sales")
    local.close()
    with pytest.raises(neolap.InputError, match="closed"):
        c["Year", "Total", "All", "Sales"]
    with pytest.raises(neolap.InputError, match="closed"):
        c["Jan", "North", "sedan", "Sales"] = 1
    with pytest.raises(neolap.InputError, match="closed"):
        c.area(years="Jan").to_pandas()


def test_dropping_the_object_without_close_also_frees_the_file(tmp_path, binary):
    path = tmp_path / "d.neolap"
    db = neolap.create(path)
    del db
    gc.collect()
    neolap.open(path).close()


def test_missing_and_foreign_files_are_reported_not_crashed(tmp_path, binary):
    with pytest.raises(neolap.NotFoundError):
        neolap.open(tmp_path / "nothing.neolap")
    junk = tmp_path / "junk.neolap"
    junk.write_text("this is not a database")
    with pytest.raises(neolap.NeolapError):
        neolap.open(junk)
    with pytest.raises(neolap.StorageError, match="already exists"):
        neolap.create(junk)


def test_the_journal_user_is_the_one_given(tmp_path, binary):
    path = tmp_path / "j.neolap"
    import subprocess
    subprocess.run([str(binary), "demo", "tiny", str(path)], check=True, capture_output=True)
    with neolap.open(path, user="claudia") as db:
        db.cube("sales")["Jan", "North", "sedan", "Sales"] = 5
    out = subprocess.run([str(binary), "journal", str(path), "--by", "claudia", "--json"], capture_output=True, text=True, check=True).stdout
    assert '"user":"claudia"' in out and '"op":"splash"' in out


def test_the_gil_is_released_so_other_threads_keep_running(big_cube_file):
    # Reading 120,000 cells takes a fraction of a second inside the engine. If the interpreter lock
    # were held meanwhile, a second Python thread would stand still for that long: it makes about
    # 8 of ~3,000 possible ticks (measured with the release removed), against ~2,300 with it.
    c = neolap.open(big_cube_file).cube("plan")
    ticks = []
    stop = threading.Event()

    def tick():
        while not stop.is_set():
            ticks.append(time.perf_counter())
            time.sleep(0.0005)

    t = threading.Thread(target=tick)
    t.start()
    time.sleep(0.05)
    start = time.perf_counter()
    n = sum(len(c.area()) for _ in range(6))  # len() makes the area read; the engine does the work
    elapsed = time.perf_counter() - start
    stop.set()
    t.join()
    assert n == 6 * 120_000
    assert elapsed > 0.3, f"the work was too short ({elapsed:.2f} s) to tell anything"
    during = [x for x in ticks if start <= x <= start + elapsed]
    gaps = [b - a for a, b in zip(during, during[1:])]
    possible = elapsed / 0.0006
    assert len(during) > possible * 0.4, f"the other thread got {len(during)} of ~{possible:.0f} ticks: the GIL is held"
    assert max(gaps) < 0.1, f"the interpreter stood still for {max(gaps) * 1000:.0f} ms"


def test_several_threads_may_use_one_database(local):
    c = local.cube("sales")
    errors = []

    def work(i):
        try:
            for k in range(50):
                c[f"Jan", "North", "sedan", "Sales"] = float(i * 100 + k + 1)
                assert c["Jan", "North", "sedan", "Sales"] is not None
                c.area(years="Q1", regions="North").to_pandas()
        except Exception as e:  # noqa: BLE001
            errors.append(repr(e))

    threads = [threading.Thread(target=work, args=(i,)) for i in range(6)]
    [t.start() for t in threads]
    [t.join() for t in threads]
    assert errors == []


# ---------------------------------------------------------------- remote

def test_the_token_is_never_part_of_repr_str_or_an_error(server):
    db = neolap.connect(server.url, token=server.token)
    assert server.token not in repr(db) and server.token not in str(db)
    assert server.token not in repr(db._backend) and server.token not in repr(vars(db._backend).keys())
    bad = neolap.connect(server.url, token=server.token + "x")
    with pytest.raises(neolap.AuthenticationError) as e:
        bad.cubes
    assert server.token not in str(e.value)


def test_a_wrong_token_is_an_authentication_error_for_every_call(server):
    db = neolap.connect(server.url, token="nlp_000000000000_" + "0" * 48)
    for call in (lambda: db.cubes, lambda: db.cube("sales")):
        with pytest.raises(neolap.AuthenticationError):
            call()


def test_connecting_does_not_contact_the_server_but_the_first_call_does(tmp_path):
    db = neolap.connect("http://127.0.0.1:9", token="nlp_x_y", timeout=2)  # nothing listens there
    with pytest.raises(neolap.ConnectionFailed, match="cannot reach"):
        db.cubes


def test_a_token_is_not_sent_over_plain_http_to_another_machine():
    with pytest.raises(neolap.InputError, match="plain http"):
        neolap.connect("http://example.org:8080", token="nlp_x_y")
    for ok in ("http://localhost:1", "http://127.0.0.1:1", "http://[::1]:1", "https://example.org"):
        neolap.connect(ok, token="nlp_x_y")  # allowed (nothing is sent yet)
    for bad in ("ftp://x", "olap.internal:8080", "", "http://", "file:///etc/passwd"):
        with pytest.raises(neolap.InputError, match="http"):
            neolap.connect(bad, token="nlp_x_y")


def test_a_slow_server_is_a_timeout_not_a_hang(tmp_path):
    import socket

    s = socket.socket()
    s.bind(("127.0.0.1", 0))
    s.listen(1)  # accepts the connection (in the kernel) but never answers
    port = s.getsockname()[1]
    db = neolap.connect(f"http://127.0.0.1:{port}", token="nlp_x_y", timeout=0.5)
    start = time.perf_counter()
    with pytest.raises(neolap.ConnectionFailed):
        db.cubes
    assert time.perf_counter() - start < 3
    s.close()


def test_member_names_with_special_characters_survive_the_url(remote, local):
    # names with &, =, #, %, spaces, umlauts and slashes must reach the server as they are
    for db in (remote,):
        c = db.cube("sales")
        with pytest.raises(neolap.NotFoundError, match="a&b=c#d%e f/ü"):
            c["Jan", "a&b=c#d%e f/ü", "sedan", "Sales"]


def test_the_cached_dimensions_survive_a_model_change_on_the_server(server):
    db = neolap.connect(server.url, token=server.token)
    c = db.cube("sales")
    assert c["Year", "Total", "All", "Sales"] == 165600.0  # fills the cache
    # a modeler renames a member through the API; the client must not keep answering from stale knowledge
    import json, urllib.request
    req = urllib.request.Request(server.url + "/api/v1/dimensions/regions", method="PATCH", data=json.dumps({"ops": [{"op": "rename", "name": "North", "to": "Nord"}]}).encode(),
                                 headers={"Authorization": f"Bearer {server.token}", "Content-Type": "application/json"})
    urllib.request.urlopen(req).read()
    assert c["Jan", "Nord", "sedan", "Sales"] == 1000.0
    with pytest.raises(neolap.NotFoundError):
        c["Jan", "North", "sedan", "Sales"]


def test_a_reader_token_can_read_but_not_write(tmp_path, binary):
    import os, socket, subprocess
    data = tmp_path / "d"
    data.mkdir()
    path = data / "neolap.neolap"
    subprocess.run([str(binary), "demo", "tiny", str(path)], check=True, capture_output=True)
    env = dict(os.environ, NEOLAP_PASSWORD="a very long secret")
    subprocess.run([str(binary), "user", "add", str(path), "rita", "--role", "reader"], check=True, capture_output=True, env=env)
    subprocess.run([str(binary), "user", "add", str(path), "root", "--role", "admin"], check=True, capture_output=True, env=env)
    token = subprocess.run([str(binary), "user", "token", str(path), "rita"], check=True, capture_output=True, env=env, text=True).stdout.strip()
    s = socket.socket(); s.bind(("127.0.0.1", 0)); port = s.getsockname()[1]; s.close()
    p = subprocess.Popen([str(binary), "serve", "--listen", f"127.0.0.1:{port}", "--data-dir", str(data)], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, env={k: v for k, v in os.environ.items() if not k.startswith("NEOLAP_")})
    try:
        time.sleep(1.5)
        db = neolap.connect(f"http://127.0.0.1:{port}", token=token)
        assert db.cubes == []  # a reader without a grant sees no cube at all
        with pytest.raises(neolap.NotFoundError):
            db.cube("sales")
    finally:
        p.terminate()
        p.wait(timeout=10)


# ---------------------------------------------------------------- the client never leaks foreign exception types

def _fake_server(responses):
    """A one-shot HTTP server answering each connection with the next raw response."""
    import socket

    srv = socket.socket()
    srv.bind(("127.0.0.1", 0))
    srv.listen(5)

    def run():
        for raw in responses:
            try:
                c, _ = srv.accept()
            except OSError:
                return
            c.settimeout(1)
            try:
                c.recv(65536)
                c.sendall(raw)
            except OSError:
                pass
            c.close()

    threading.Thread(target=run, daemon=True).start()
    return srv, srv.getsockname()[1]


@pytest.mark.parametrize("token", ["abc\r\nX-Injected: yes", "abc\nx", "abc\x00def", "tab\there", "ü-token", ""])
def test_a_token_that_cannot_be_sent_is_refused_when_connecting(token):
    # at `connect`, not at the first request: the URL is checked there as well, and nothing is sent
    with pytest.raises(neolap.InputError, match="token"):
        neolap.connect("http://127.0.0.1:9", token=token, timeout=1)


def test_a_cut_off_answer_is_a_connection_failure_not_a_foreign_exception():
    srv, port = _fake_server([b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 500\r\n\r\n{\"cubes\":["])
    db = neolap.connect(f"http://127.0.0.1:{port}", token="nlp_x_y", timeout=2)
    with pytest.raises(neolap.ConnectionFailed):
        db.cubes
    srv.close()


def test_garbage_from_the_server_is_not_a_foreign_exception_either():
    srv, port = _fake_server([b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 9\r\n\r\nnot json!", b"this is not http at all\r\n\r\n", b"HTTP/1.1 500 Oops\r\nContent-Length: 4\r\n\r\n<hi>"])
    db = neolap.connect(f"http://127.0.0.1:{port}", token="nlp_x_y", timeout=2)
    for _ in range(3):
        with pytest.raises(neolap.NeolapError):
            db.cubes
    srv.close()


def test_a_server_that_hangs_up_is_a_connection_failure():
    srv, port = _fake_server([b""])  # closes without a byte
    db = neolap.connect(f"http://127.0.0.1:{port}", token="nlp_x_y", timeout=2)
    with pytest.raises(neolap.ConnectionFailed):
        db.cubes
    srv.close()


@pytest.mark.parametrize(
    "status, body, exception",
    [
        (401, '{"error":"authentication required","code":"unauthorized"}', neolap.AuthenticationError),
        (403, '{"error":"no write access to cell [x]","code":"forbidden"}', neolap.PermissionError),
        (404, '{"error":"cube \'x\' not found","code":"not_found"}', neolap.NotFoundError),
        (409, '{"error":"changed","code":"conflict","details":{"user":"ben","cell":"Jan, North","value":5.0}}', neolap.ConflictError),
        (429, '{"error":"too many failed logins","code":"too_many_requests"}', neolap.TooManyRequests),
        (400, '{"error":"bad input","code":"bad_request"}', neolap.InputError),
        (413, "", neolap.InputError),
        (500, '{"error":"internal error","code":"internal"}', neolap.StorageError),
        (502, "", neolap.ConnectionFailed),
        (503, '{"error":"busy","code":"locked"}', neolap.ConnectionFailed),
        (504, "", neolap.ConnectionFailed),
    ],
)
def test_every_http_status_becomes_the_exception_a_user_would_catch(status, body, exception):
    raw = f"HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {len(body)}\r\n\r\n{body}".encode()
    srv, port = _fake_server([raw])
    db = neolap.connect(f"http://127.0.0.1:{port}", token="nlp_x_y", timeout=2)
    with pytest.raises(exception) as e:
        db.cubes
    assert type(e.value) is exception or isinstance(e.value, exception)
    if status == 409:
        assert (e.value.user, e.value.cell, e.value.value) == ("ben", "Jan, North", 5.0)
    srv.close()


def test_all_exceptions_are_neolap_errors_so_one_except_clause_catches_them():
    for cls in (neolap.NotFoundError, neolap.ConflictError, neolap.PermissionError, neolap.InputError, neolap.LockedError,
                neolap.StorageError, neolap.ConnectionFailed, neolap.AuthenticationError, neolap.TooManyRequests):
        assert issubclass(cls, neolap.NeolapError), cls
    assert not issubclass(neolap.PermissionError, OSError), "must not be confused with the builtin PermissionError"


def test_star_import_does_not_hide_the_builtin_permission_error():
    ns = {}
    exec("from neolap import *", ns)
    assert "PermissionError" not in ns or ns["PermissionError"] is PermissionError
    assert ns["NotFoundError"] is neolap.NotFoundError and ns["open"] is neolap.open
    assert neolap.PermissionError is not PermissionError
