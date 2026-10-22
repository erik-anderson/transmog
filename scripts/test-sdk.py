"""Exercise the shared Application model through C++, C ABI, and .NET."""
from __future__ import annotations
import argparse
import ctypes
import gzip
from http.client import HTTPConnection
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import os
from pathlib import Path
import sys
import tempfile
import threading
import time
from repo_tools import ROOT, command, development_environment, main_guard

sys.path.insert(0, str(ROOT / "sdk/tests"))
import model_support as models

entered = threading.Event()
resume = threading.Event()
BODY = b"native\x00response\n"
STREAM_BODY = bytes(range(256)) * 769
stream_entered, stream_resume = threading.Event(), threading.Event()
managed_stream_output = None

class Origin(BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path == "/hold":
            entered.set()
            resume.wait(10)
        body = STREAM_BODY if self.path in ("/stream", "/managed-stream") else \
            b"" if self.path == "/empty" else gzip.compress(BODY, mtime=0) if self.path == "/gzip" else BODY
        self.send_response(200)
        self.send_header("Content-Type", "application/octet-stream")
        self.send_header("Content-Length", str(100 if self.path == "/broken" else len(body)))
        if self.path == "/gzip": self.send_header("Content-Encoding", "gzip")
        self.end_headers()
        if self.path in ("/stream", "/managed-stream"):
            self.wfile.write(body[:32768])
            self.wfile.flush()
            stream_entered.set()
            if self.path == "/stream":
                if not stream_resume.wait(15): return
            else:
                deadline = time.monotonic() + 15
                while not (managed_stream_output and managed_stream_output.exists()
                           and managed_stream_output.stat().st_size > 0):
                    if time.monotonic() >= deadline: return
                    time.sleep(0.01)
            self.wfile.write(body[32768:])
        else:
            self.wfile.write(body)
    def log_message(self, *_):
        pass

def require(value, message):
    if not value:
        raise ValueError(message)

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--configuration", choices=("Debug", "Release"), default="Debug")
    parser.add_argument("--skip-build", action="store_true")
    args = parser.parse_args()
    env = development_environment()
    compiler = command(["rustc", "-vV"], env=env, capture=True).stdout
    host = next(line.split(": ", 1)[1] for line in compiler.splitlines() if line.startswith("host:"))
    output = ROOT / ".local/sdk" / host / args.configuration
    managed_contract = output / "managed-body-reader"
    if not args.skip_build:
        command([sys.executable, ROOT / "scripts/build-sdk.py", "--configuration", args.configuration, "--test"],
                env=dict(env, CARGO_BUILD_TARGET="wasm32-unknown-unknown"))
        command([sys.executable, ROOT / "scripts/build-dotnet-sample.py", "--configuration", args.configuration,
                 "--sdk-output", output / "install"], env=env)
        command(["dotnet", "build", ROOT / "sdk/tests/managed-body-reader/BodyReaderContract.csproj",
                 "--configuration", args.configuration, "--artifacts-path", managed_contract], env=env)
    command(["dotnet", managed_contract / "bin/BodyReaderContract" / args.configuration.lower() / "BodyReaderContract.dll"], env=env)
    for cmake in (output / "install/lib/cmake/Transmog").glob("*.cmake"):
        text = cmake.read_text(encoding="utf-8")
        require("sdk-crubit" not in text and "rust-target" not in text, "Leaked private build dependency")
    library = output / "install" / ("bin/transmog_sdk.dll" if os.name == "nt" else "lib/libtransmog_sdk.so")
    sdk = ctypes.CDLL(str(library))
    def bind(name, arguments, returns):
        function = getattr(sdk, "transmog_sdk_" + name)
        function.argtypes, function.restype = arguments, returns
        return function
    pointer, size = ctypes.c_void_p, ctypes.c_size_t
    version = bind("abi_version", [], ctypes.c_uint32)
    require(version() == 4, "Expected application SDK ABI 4")
    create = bind("application_create", [], pointer)
    create_with_bodies = bind("application_create_with_body_store", [pointer, size], pointer)
    valid = bind("application_valid", [pointer], ctypes.c_int32)
    free_app = bind("application_free", [pointer], None)
    free_result = bind("result_free", [pointer], None)
    succeeded = bind("result_succeeded", [pointer], ctypes.c_int32)
    output_types = {"status": "AppStatus", "product_state": "ProductState", "stop_proxy": "AppStatus",
                    "shutdown": None, "save_product_state": "ProductState", "start_proxy": "AppStatus",
                    "query_sessions": "SessionPage", "query_traffic_view": "TrafficView",
                    "session_detail": "SessionDetail", "release_traffic_view": None}
    getters = {name: bind("result_as_" + models.kebab(name).replace("-", "_"), [pointer], pointer)
               for name in set(output_types.values()) - {None}}
    getters["SliceSessionSummary"] = bind("result_as_session_rows", [pointer], pointer)
    defaults = {}
    for name, suffix in (("ProxyStartRequest", "proxy_start_request"), ("SessionQueryInput", "session_query_input")):
        getters[name] = bind("result_as_" + suffix, [pointer], pointer)
        defaults[name] = bind("default_" + suffix, [], pointer)
    input_types = {"save_product_state": "ProductState", "start_proxy": "ProxyStartRequest",
                   "query_sessions": "SessionQueryInput", "query_traffic_view": "SessionQueryInput"}
    operations = {}
    for name in ("status", "product_state", "stop_proxy", "shutdown"):
        operations[name] = bind("application_" + name, [pointer], pointer)
    for name in input_types:
        operations[name] = bind("application_" + name, [pointer, pointer], pointer)
    for name in ("session_detail", "release_traffic_view"):
        operations[name] = bind("application_" + name, [pointer, pointer, size], pointer)
    rows_operation = bind("application_traffic_view_rows", [pointer, pointer, size, size, size], pointer)
    callback_type = ctypes.CFUNCTYPE(None, pointer, pointer)
    subscribe = bind("application_subscribe_session_updates", [pointer, callback_type, pointer], pointer)
    valid_updates = bind("session_updates_valid", [pointer], ctypes.c_int32)
    updates_status = bind("session_updates_status", [pointer], pointer)
    free_updates = bind("session_updates_free", [pointer], None)

    class NativeBytes(ctypes.Structure):
        _fields_ = [("data", pointer), ("size", size)]
    class NativeError(ctypes.Structure):
        _fields_ = [("category", ctypes.c_uint32), ("message", NativeBytes), ("retryable", ctypes.c_uint32)]
    class NativeBodyPage(ctypes.Structure):
        _fields_ = [("bytes", NativeBytes), ("offset", ctypes.c_uint64), ("total_bytes", ctypes.c_uint64),
                    ("read_limit", size), ("availability", ctypes.c_uint32), ("length_known", ctypes.c_uint32),
                    ("observed_bytes", ctypes.c_uint64), ("retained_bytes", ctypes.c_uint64),
                    ("has_wire_body_bytes", ctypes.c_uint32), ("wire_body_bytes", ctypes.c_uint64),
                    *[(name, NativeBytes) for name in ("exchange_id", "boundary", "media_type", "charset", "sha256", "reason")],
                    ("content_coding_count", size)]
    body_range = bind("application_read_body_range", [pointer, pointer, size, pointer, size, ctypes.c_uint64, size], pointer)
    body_succeeded = bind("body_page_succeeded", [pointer], ctypes.c_int32)
    body_error = bind("body_page_error", [pointer], NativeError)
    body_view = bind("body_page_get_view", [pointer], NativeBodyPage)
    body_coding = bind("body_page_content_coding", [pointer, size], NativeBytes)
    free_body = bind("body_page_free", [pointer], None)

    def text(value):
        return ctypes.string_at(value.data, value.size).decode() if value.data else None

    def body_handle(app, request):
        session = request["sessionId"]
        boundary = request["boundary"]
        session = session if isinstance(session, bytes) else session.encode()
        boundary = boundary if isinstance(boundary, bytes) else boundary.encode()
        session_input, boundary_input = ctypes.create_string_buffer(session), ctypes.create_string_buffer(boundary)
        return body_range(app, session_input, len(session), boundary_input, len(boundary),
                          int(request["offset"]), request["maxBytes"])

    def read_body(app, request, expected):
        page = body_handle(app, request)
        require(page, "SDK body-page allocation failed")
        try:
            success = bool(body_succeeded(page))
            if success:
                view = body_view(page)
                value = {"bytes": ctypes.string_at(view.bytes.data, view.bytes.size), "offset": view.offset,
                         "totalBytes": view.total_bytes, "readLimit": view.read_limit,
                         "metadata": {"lengthKnown": bool(view.length_known), "exchangeId": text(view.exchange_id),
                                      "boundary": text(view.boundary), "observedBytes": view.observed_bytes,
                                      "retainedBytes": view.retained_bytes,
                                      "wireBodyBytes": view.wire_body_bytes if view.has_wire_body_bytes else None,
                                      "availability": ("unavailable", "capturing", "complete", "truncated", "lost", "evicted", "disabled", "quota-omitted")[view.availability],
                                      "mediaType": text(view.media_type), "charset": text(view.charset),
                                      "sha256": text(view.sha256), "reason": text(view.reason),
                                      "contentCodings": [text(body_coding(page, index)) for index in range(view.content_coding_count)]}}
            else:
                error = body_error(page)
                value = {"category": ("none", "password-required", "invalid-password", "invalid-input", "unsupported", "canceled", "conflict", "limit", "unavailable", "retryable", "internal")[error.category],
                         "message": text(error.message), "retryable": bool(error.retryable)}
            require(success == expected, f"read_body_range: {value}")
            return value
        finally:
            free_body(page)

    get_error = bind("result_error", [pointer], NativeError)
    def read(result, model=None):
        require(result, "SDK wrapper allocation failed")
        try:
            success = bool(succeeded(result))
            if success:
                value = models.read_pointer(model, getters[model](result)) if model else None
            else:
                error = get_error(result)
                value = {"category": models.read("ErrorCategory", error.category),
                         "message": text(error.message), "retryable": bool(error.retryable)}
            return success, value
        finally:
            free_result(result)

    def call(app, name, value=None, expected=True):
        if name == "read_body_range": return read_body(app, value, expected)
        owners = []
        if name in input_types:
            model = input_types[name]
            if model in defaults:
                initial = read(defaults[model](), model)[1]
                initial.update(value)
                value = initial
            source = models.write(model, value, owners)
            result = operations[name](app, ctypes.byref(source))
        elif value is None:
            result = operations[name](app)
        else:
            data = value if isinstance(value, bytes) else value.encode()
            source = ctypes.create_string_buffer(data)
            result = operations[name](app, source, len(data))
        success, value = read(result, output_types[name])
        require(success == expected, f"{name}: {value}")
        return value

    def send(listener, url, expected_body=BODY):
        hostname, port = listener.rsplit(":", 1)
        connection = HTTPConnection(hostname, int(port), timeout=15)
        try:
            connection.request("GET", url)
            response = connection.getresponse()
            body = response.read()
            require(response.status == 200 and body == expected_body, "Proxy changed binary HTTP response")
        finally:
            connection.close()

    def rows(app, view_id, offset=0, limit=100, expected=True):
        data = view_id if isinstance(view_id, bytes) else view_id.encode()
        source = ctypes.create_string_buffer(data)
        success, value = read(rows_operation(app, source, len(data), offset, limit), "SliceSessionSummary")
        require(success == expected, f"traffic_view_rows: {value}")
        return value

    hints, callback_errors, notification = [], [], threading.Event()
    context_value = ctypes.c_int(73)
    @callback_type
    def on_update(context, data):
        try:
            require(ctypes.cast(context, ctypes.POINTER(ctypes.c_int)).contents.value == 73, "Callback context changed")
            hint = models.read_pointer("SessionHint", data)
            require(set(hint) == {"exchangeId", "sequence", "lagged"}, "Expected the shared SessionHint schema")
            hints.append(hint)
            notification.set()
        except Exception as error:
            callback_errors.append(error)
            notification.set()

    server = ThreadingHTTPServer(("127.0.0.1", 0), Origin)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    url = f"http://127.0.0.1:{server.server_port}"
    cache = tempfile.TemporaryDirectory(prefix="body-cache-", dir=output)
    cache_path = str(Path(cache.name) / "bodies").encode()
    cache_input = ctypes.create_string_buffer(cache_path)
    app = create_with_bodies(cache_input, len(cache_path))
    updates = None
    require(app and valid(app), "Application creation failed")
    try:
        require(call(app, "status")["lifecycle"] == "stopped", "Unexpected initial lifecycle")
        default_app = create()
        try:
            disabled = call(default_app, "read_body_range", {
                "sessionId": "0" * 32, "boundary": "client-response", "offset": "0", "maxBytes": 1}, expected=False)
            require(disabled["category"] == "unavailable", "Default application must keep body retention opt-in")
        finally: free_app(default_app)
        state = call(app, "product_state")
        state["preferences"]["decryptHttps"] = False
        state["preferences"]["configureSystemProxy"] = False
        state["privacy"]["bufferLimit"] = {"kind": "Custom", "bytes": 64 * 1024 * 1024}
        saved = call(app, "save_product_state", state)
        require(saved == state, "SDK changed the ProductState schema")
        request = {"caCertificatePath": "", "caPrivateKeyPath": "", "listen": "127.0.0.1:0",
                   "route": "http1", "allowRemoteClients": False, "recordingOnly": False}
        started = call(app, "start_proxy", request)
        listener = started["listener"]
        require(started["lifecycle"] == "running", "Application did not start")
        require(call(app, "start_proxy", request, expected=False)["category"] == "conflict",
                "Start-twice must preserve the application's typed error")
        invalid = subscribe(app, callback_type(), None)
        require(invalid and not valid_updates(invalid), "Null callback must be an inspectable creation error")
        require(read(updates_status(invalid))[1]["category"] == "invalid-input", "Invalid callback error schema")
        free_updates(invalid)
        updates = subscribe(app, on_update, ctypes.byref(context_value))
        require(updates and valid_updates(updates) and read(updates_status(updates))[0], "Subscription creation failed")
        for bad in ({"search": "x" * 262145}, {"limit": 0}):
            failure = call(app, "query_sessions", bad, expected=False)
            require(failure["category"] in ("invalid-input", "limit") and "retryable" in failure,
                    "Expected the shared AppError model")
        require(read(operations["query_sessions"](app, None))[1]["category"] == "invalid-input",
                "Null typed input must return an error")
        for _ in range(3):
            send(listener, url + "/ok")
        deadline = time.monotonic() + 10
        while True:
            page = call(app, "query_sessions", {"terminal": True, "method": "GET", "limit": 10})
            if len(page["sessions"]) == 3:
                break
            require(time.monotonic() < deadline, "Completed traffic did not reach the application catalog")
            time.sleep(0.025)
        session_id = page["sessions"][0]["id"]
        detail = call(app, "session_detail", session_id.encode())
        require(detail["id"] == session_id, "Inspection did not use the catalog's exchange ID")
        body_request = {"sessionId": session_id, "boundary": "client-response", "offset": "0", "maxBytes": 4}
        body_page = call(app, "read_body_range", body_request)
        require(body_page["bytes"] == BODY[:4]
                and body_page["totalBytes"] == len(BODY) and body_page["offset"] == 0
                and body_page["metadata"]["availability"] == "complete", "Body page must preserve the shared model and binary bytes")
        require(body_page["metadata"] == next(body for body in detail["storedBodies"] if body["boundary"] == "client-response"),
                "Typed metadata differs from the desktop serialization")
        for change, category in (({"maxBytes": 0}, "limit"), ({"maxBytes": 16777217}, "limit"),
                                 ({"offset": "18446744073709551615"}, "unavailable"),
                                 ({"sessionId": b"\xff"}, "invalid-input"), ({"boundary": b"\xff"}, "invalid-input"),
                                 ({"sessionId": b"x" * 262145}, "limit"),
                                 ({"boundary": "not-a-boundary"}, "invalid-input")):
            require(call(app, "read_body_range", dict(body_request, **change), expected=False)["category"] == category,
                    "Body range validation differs from the application")
        require(notification.wait(5) and hints and not callback_errors, f"Native callback failed: {callback_errors}")

        query = {"sort": {"column": "request-number", "direction": "descending"}}
        frozen = call(app, "query_traffic_view", query)
        frozen_id = frozen["id"]
        require(len(frozen["ids"]) == 3 and frozen["page"]["totalMatched"] == 3, "Traffic view must include all matching IDs")
        require([row["id"] for row in rows(app, frozen_id, 1, 1)] == frozen["ids"][1:2], "Row window did not follow view order")
        for limit in (0, 513):
            require(rows(app, frozen_id, limit=limit, expected=False)["category"] == "invalid-input", "Shared row limit validation changed")
        require(rows(app, b"\xff", expected=False)["category"] == "invalid-input", "View ID UTF-8 validation")
        require(rows(app, b"x" * 262145, expected=False)["category"] == "limit", "View ID transfer bound")

        # A live request makes the app's reversible drain/resume behavior observable.
        failures = []
        def held_request():
            try:
                send(listener, url + "/hold")
            except Exception as error:
                failures.append(error)
        request_thread = threading.Thread(target=held_request)
        request_thread.start()
        require(entered.wait(10), "Held request was not admitted")
        try:
            deadline = time.monotonic() + 10
            while True:
                live = call(app, "query_traffic_view", query)
                if len(live["ids"]) == 4:
                    break
                call(app, "release_traffic_view", live["id"].encode())
                require(time.monotonic() < deadline, "Active request did not reach the traffic view")
                time.sleep(0.025)
            live_id = live["id"]
            active = rows(app, live_id, 0, 1)[0]
            require(active["path"] == "/hold" and active["terminal"] == "active" and active["status"] is None, "Active row evidence changed")
            require([row["id"] for row in rows(app, frozen_id)] == frozen["ids"], "New traffic changed existing view ordering")
            draining = call(app, "stop_proxy")
            require(draining["lifecycle"] == "draining", "stop_proxy must begin the app's drain")
            resumed = call(app, "start_proxy", request)
            require(resumed["lifecycle"] == "running" and resumed["listener"] == listener,
                    "Start during drain must reuse the same running listener")
        finally:
            resume.set()
            request_thread.join(15)
        require(not request_thread.is_alive() and not failures, f"Drain interrupted admitted traffic: {failures}")
        deadline = time.monotonic() + 10
        while True:
            completed = rows(app, live_id, 0, 1)[0]
            if completed["terminal"] == "completed": break
            require(time.monotonic() < deadline, "Existing view did not refresh row metadata")
            time.sleep(0.025)
        require(completed["id"] == active["id"] and completed["status"] == 200, "Row identity changed on completion")
        for view_id in (frozen_id, live_id):
            require(call(app, "release_traffic_view", view_id.encode()) is None, "Release must return unit")
            require(rows(app, view_id, expected=False)["category"] == "unavailable", "Released view remained readable")
            call(app, "release_traffic_view", view_id.encode())
        # Explicit release must return capacity to the shared application registry.
        views = [call(app, "query_traffic_view", query)["id"] for _ in range(8)]
        require(call(app, "query_traffic_view", query, expected=False)["category"] == "limit", "Expected finite view capacity")
        for view_id in views: call(app, "release_traffic_view", view_id.encode())
        free_updates(updates)
        updates = None
        count = len(hints)
        send(listener, url + "/after-unsubscribe")
        time.sleep(0.15)
        require(len(hints) == count and not callback_errors, "Callback ran after unsubscribe returned")

        # Unsubscribe joins an in-flight callback, and callbacks can safely re-query.
        callback_entered, callback_resume, unsubscribed = threading.Event(), threading.Event(), threading.Event()
        @callback_type
        def blocked_update(_context, _data):
            try:
                call(app, "query_sessions", {"limit": 1})
                callback_entered.set()
                require(callback_resume.wait(10), "Blocked callback was not released")
            except Exception as error:
                callback_errors.append(error)
                callback_entered.set()
        blocked = subscribe(app, blocked_update, None)
        require(blocked and valid_updates(blocked), "Blocked subscription failed")
        unsubscribe_thread = None
        try:
            send(listener, url + "/callback-in-flight")
            require(callback_entered.wait(5) and not callback_errors, "Callback re-query failed")
            def unsubscribe_blocked():
                free_updates(blocked)
                unsubscribed.set()
            unsubscribe_thread = threading.Thread(target=unsubscribe_blocked)
            unsubscribe_thread.start()
            require(not unsubscribed.wait(0.05), "Unsubscribe returned while callback was still in flight")
        finally:
            callback_resume.set()
            if unsubscribe_thread:
                unsubscribe_thread.join(10)
            else:
                free_updates(blocked)
        require(unsubscribed.is_set() and not callback_errors, "Unsubscribe failed to join callback")

        # Releasing from the callback itself must cancel without self-joining.
        self_released = threading.Event()
        self_subscription = []
        @callback_type
        def self_release(_context, _data):
            try:
                free_updates(self_subscription[0])
                self_released.set()
            except Exception as error:
                callback_errors.append(error)
        self_subscription.append(subscribe(app, self_release, None))
        require(self_subscription[0] and valid_updates(self_subscription[0]), "Self-release subscription failed")
        send(listener, url + "/callback-self-release")
        require(self_released.wait(5) and not callback_errors, "Callback could not release itself")

        # The origin withholds its tail until the SDK has read its captured prefix.
        streaming_failures = []
        def streaming_request():
            try: send(listener, url + "/stream", STREAM_BODY)
            except Exception as error: streaming_failures.append(error)
        streaming_thread = threading.Thread(target=streaming_request)
        streaming_thread.start()
        captured = bytearray()
        try:
            require(stream_entered.wait(10), "Streaming origin did not send its prefix")
            deadline = time.monotonic() + 10
            stream_id = None
            while True:
                candidates = call(app, "query_sessions", {"method": "GET", "latest": True, "limit": 100})["sessions"]
                stream_id = next((row["id"] for row in candidates if row["path"] == "/stream"), None)
                if stream_id:
                    stream_detail = call(app, "session_detail", stream_id.encode())
                    metadata = next((body for body in stream_detail["storedBodies"] if body["boundary"] == "client-response"), None)
                    if metadata and metadata["retainedBytes"] >= 32768: break
                require(time.monotonic() < deadline, "Capturing prefix did not become readable")
                time.sleep(0.025)
            while len(captured) < 32768:
                page = call(app, "read_body_range", dict(body_request, sessionId=stream_id, offset=str(len(captured)), maxBytes=4096))
                require(page["metadata"]["availability"] == "capturing", "Expected a live retained body")
                captured.extend(page["bytes"])
            require(captured == STREAM_BODY[:32768] and streaming_thread.is_alive(), "Prefix read waited for body completion")
        finally:
            stream_resume.set()
            streaming_thread.join(15)
        require(not streaming_thread.is_alive() and not streaming_failures, f"Streaming response failed: {streaming_failures}")
        call(app, "shutdown")
        while len(captured) < len(STREAM_BODY):
            page = call(app, "read_body_range", dict(body_request, sessionId=stream_id, offset=str(len(captured)), maxBytes=4096))
            require(page["metadata"]["availability"] == "complete", "Completed body was not retained")
            captured.extend(page["bytes"])
        require(captured == STREAM_BODY, "Chunked body reads changed the captured bytes")
        require(call(app, "status")["lifecycle"] == "stopped", "Final shutdown did not stop the application")
        # Results are independent owners, even after their application is freed.
        retained = operations["product_state"](app)
        retained_body = body_handle(app, dict(body_request, sessionId=stream_id, offset=0, maxBytes=4))
        free_app(app)
        app = None
        require(read(retained, "ProductState")[0], "Result did not survive application destruction")
        try:
            view = body_view(retained_body)
            require(body_succeeded(retained_body) and ctypes.string_at(view.bytes.data, view.bytes.size) == STREAM_BODY[:4],
                    "Typed body page did not survive application destruction")
        finally:
            free_body(retained_body)

        with tempfile.TemporaryDirectory(prefix="application-", dir=output) as temporary:
            directory = Path(temporary)
            consumer = output / "consumer" / ("transmog_cpp_embedding.exe" if os.name == "nt" else "transmog_cpp_embedding")
            command([consumer, "--self-test"], env=env)
            project = ROOT / "examples/dotnet-embedding/Transmog.Embedding.Sample"
            assembly = project / "bin" / args.configuration / "net10.0/Transmog.Embedding.Sample.dll"
            body = directory / "response.bin"
            managed = command(["dotnet", assembly, "--url", url + "/ok", "--output", body], env=env, capture=True)
            require(body.read_bytes() == BODY and "HTTP 200" in managed.stdout and "retained body boundaries" in managed.stdout
                    and "Session updates received:" in managed.stdout and "SHA-256 matches host response" in managed.stdout,
                    ".NET did not capture and inspect host HTTP traffic")
            global managed_stream_output
            managed_stream_output = directory / "streamed.bin"
            streamed = command(["dotnet", assembly, "--url", url + "/managed-stream", "--output", managed_stream_output], env=env, capture=True)
            require(managed_stream_output.read_bytes() == STREAM_BODY and "Native body streaming started." in streamed.stdout,
                    ".NET must read native captured bytes before the origin completes")
            for path, expected in (("empty", b""), ("gzip", gzip.compress(BODY, mtime=0))):
                destination = directory / (path + ".bin")
                command(["dotnet", assembly, "--url", url + "/" + path, "--output", destination], env=env, capture=True)
                require(destination.read_bytes() == expected, "Body streaming changed empty or content-encoded bytes")
            broken = command(["dotnet", assembly, "--url", url + "/broken", "--output", directory / "broken.bin"], env=env, capture=True, check=False)
            require(broken.returncode == 1 and "SHA-256 matches host response" not in broken.stdout,
                    "Incomplete bodies must not report a successful complete stream")
            failure = command(["dotnet", assembly, "--url", url + "/ok", "--output", body],
                              env=env, capture=True, check=False)
            require(failure.returncode == 1 and body.read_bytes() == BODY, "Example overwrote an existing output")
        require(not valid(None) and not succeeded(None), "Null handles must be safe")
        free_app(None)
        free_result(None)
        free_updates(None)
        require(not body_succeeded(None) and not body_view(None).bytes.data, "Empty typed page must be safe")
        free_body(None)
    finally:
        resume.set()
        stream_resume.set()
        if updates: free_updates(updates)
        if app:
            free_app(app)
        server.shutdown()
        server.server_close()
        thread.join()
        cache.cleanup()
    print("Application SDK passed: body ranges/live streaming/binary integrity, traffic views/live rows/release, native callbacks/unsubscribe, shared models/errors, drain/resume, shutdown, ownership, C++, C and .NET")

if __name__ == "__main__":
    main_guard(main)
