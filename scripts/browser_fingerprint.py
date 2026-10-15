"""Decode locally recorded public wire data; no fingerprint database or network IO.

JA4 follows FoxIO's published TLS specification. HTTP and TCP inputs are reported
directly rather than bundling the separately licensed JA4H/JA4T implementations.
"""
import hashlib
import html
import json
import socket
import threading
from pathlib import Path


class Reader:
    def __init__(self, data):
        self.data = bytes(data)
        self.offset = 0

    def take(self, length):
        if length < 0 or self.offset + length > len(self.data):
            raise ValueError("Truncated wire vector")
        value = self.data[self.offset:self.offset + length]
        self.offset += length
        return value

    def uint(self, size):
        return int.from_bytes(self.take(size), "big")

    def vector(self, size):
        return self.take(self.uint(size))

    def done(self):
        if self.offset != len(self.data):
            raise ValueError("Trailing wire bytes")

    def remaining(self):
        return len(self.data) - self.offset


def grease(value):
    return value & 0x0f0f == 0x0a0a and value >> 8 == value & 255


def codes(data, width=2):
    r = Reader(data)
    result = []
    while r.remaining():
        result.append(r.uint(width))
    return result


def normalized_codes(values):
    return ["GREASE" if grease(v) else f"{v:04x}" for v in values]


def digest(value):
    return hashlib.sha256(value.encode()).hexdigest()[:12]


def ja4(ciphers, extensions, signatures, versions, legacy, alpn, transport):
    """JA4, including signature order; empty hash sections are explicitly zero."""
    real_ciphers = [v for v in ciphers if not grease(v)]
    real_extensions = [v for v in extensions if not grease(v)]
    version = max((v for v in versions if not grease(v)), default=legacy)
    version = {0x0304: "13", 0x0303: "12", 0x0302: "11", 0x0301: "10"}.get(version, "00")
    first = alpn[0] if alpn else b""
    token = "00"
    if first:
        ascii_alnum = lambda b: 48 <= b <= 57 or 65 <= b <= 90 or 97 <= b <= 122
        token = chr(first[0]) + chr(first[-1]) if ascii_alnum(first[0]) and ascii_alnum(first[-1]) else first.hex()[0] + first.hex()[-1]
    a = f"{transport}{version}{'d' if 0 in extensions else 'i'}{min(len(real_ciphers), 99):02}{min(len(real_extensions), 99):02}{token}"
    cipher_list = ",".join(f"{v:04x}" for v in sorted(real_ciphers))
    extension_list = ",".join(f"{v:04x}" for v in sorted(real_extensions) if v not in (0, 16))
    sigs = ",".join(f"{v:04x}" for v in signatures if not grease(v))
    extension_input = extension_list + ("_" + sigs if sigs else "")
    return a + "_" + (digest(cipher_list) if cipher_list else "0" * 12) + "_" + (digest(extension_input) if extension_list else "0" * 12)


def varint(r):
    first = r.uint(1)
    size = 1 << (first >> 6)
    return ((first & 63) << (8 * (size - 1))) | r.uint(size - 1)


def quic_parameters(data):
    r = Reader(data)
    raw, normalized, seen = [], {}, set()
    while r.remaining():
        key = varint(r)
        value = r.take(varint(r))
        if key in seen:
            raise ValueError("Duplicate QUIC transport parameter")
        seen.add(key)
        raw.append({"id": key, "value": value.hex()})
        if key % 31 == 27:  # RFC 9000 reserved transport parameters.
            continue
        if key in (0, 2, 15, 16):
            normalized[str(key)] = {"length": len(value)}  # random CIDs/reset token
        elif key in (1, 3, 4, 5, 6, 7, 8, 9, 10, 11, 14, 0x20):
            v = Reader(value)
            normalized[str(key)] = varint(v)
            v.done()
        else:
            normalized[str(key)] = value.hex()
    return {"wire_order": raw, "parameters": normalized, "grease_count": sum(v["id"] % 31 == 27 for v in raw)}


def client_hello(data, transport="t"):
    r = Reader(data)
    # BoringSSL supplies the body, not a TLS record or handshake header.
    legacy = r.uint(2)
    r.take(32)
    session_length = len(r.vector(1))
    ciphers = codes(r.vector(2))
    compression = list(r.vector(1))
    extension_reader = Reader(r.vector(2))
    r.done()
    extensions = {}
    while extension_reader.remaining():
        kind = extension_reader.uint(2)
        if kind in extensions:
            raise ValueError("Duplicate TLS extension")
        extensions[kind] = extension_reader.vector(2)

    def vector(kind, size=2):
        if kind not in extensions:
            return b""
        v = Reader(extensions[kind])
        result = v.vector(size)
        v.done()
        return result

    groups = codes(vector(10))
    signatures = codes(vector(13))
    versions = codes(vector(43, 1))
    alpn_reader = Reader(vector(16))
    alpn = []
    while alpn_reader.remaining():
        alpn.append(alpn_reader.vector(1))
    shares_reader = Reader(vector(51))
    shares = []
    while shares_reader.remaining():
        group = shares_reader.uint(2)
        key = shares_reader.vector(2)
        shares.append({"group": "GREASE" if grease(group) else f"{group:04x}", "length": len(key)})
    names_reader = Reader(vector(0))
    names = []
    while names_reader.remaining():
        name_type = names_reader.uint(1)
        names.append({"type": name_type, "name": names_reader.vector(2).decode("ascii")})
    dynamic = {0, 10, 13, 16, 21, 27, 41, 43, 51, 57, 0xfe0d}
    normalized = {
        "legacy_version": f"{legacy:04x}", "session_id_length": session_length,
        "ciphers": normalized_codes(ciphers), "compression": compression,
        "extensions": sorted(normalized_codes(extensions)), "sni": names,
        "alpn": [p.hex() for p in alpn], "groups": normalized_codes(groups),
        "key_shares": shares, "versions": normalized_codes(versions),
        "signature_algorithms": normalized_codes(signatures),
        "certificate_compression": codes(vector(27, 1)),
        "grease_extension_lengths": sorted(len(v) for k, v in extensions.items() if grease(k)),
        "padding_length": len(extensions[21]) if 21 in extensions else None,
        "extension_payloads": {f"{k:04x}": v.hex() for k, v in extensions.items() if k not in dynamic and not grease(k)},
    }
    if 0xfe0d in extensions:
        ech = Reader(extensions[0xfe0d])
        mode = ech.uint(1)
        normalized["ech"] = {"type": mode}
        if mode == 0:
            normalized["ech"].update(kdf=ech.uint(2), aead=ech.uint(2))
            ech.take(1)  # randomized GREASE config identifier
            normalized["ech"].update(enc_length=len(ech.vector(2)), payload_length=len(ech.vector(2)))
            ech.done()
        elif mode == 1:
            ech.done()
        else:
            raise ValueError("Invalid ECH ClientHello type")
    quic = quic_parameters(extensions[57]) if 57 in extensions else None
    if quic is not None:
        normalized["quic_transport"] = quic["parameters"]
        normalized["quic_grease_count"] = quic["grease_count"]
    return {"ja4": ja4(ciphers, extensions, signatures, versions, legacy, alpn, transport),
            "normalized": normalized, "alpn_names": [p.decode("ascii", "backslashreplace") for p in alpn],
            "extension_order": normalized_codes(extensions), "extension_lengths": {f"{k:04x}": len(v) for k, v in extensions.items()},
            "quic": quic, "wire_sha256": hashlib.sha256(data).hexdigest()}


PREFACE = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n"


def h2_frames(data):
    r = Reader(data)
    if r.take(24) != PREFACE:
        raise ValueError("Missing HTTP/2 preface")
    frames, block, headers_stream, complete = [], bytearray(), None, False
    while r.remaining() and not complete:
        length, kind, flags, stream = r.uint(3), r.uint(1), r.uint(1), r.uint(4) & 0x7fffffff
        payload = r.take(length)
        if not frames and (kind != 4 or flags & 1):
            raise ValueError("First HTTP/2 frame must be non-ACK SETTINGS")
        if headers_stream is not None and kind != 9:
            raise ValueError("Interleaved HTTP/2 continuation")
        frame = {"type": kind, "flags": flags, "stream": stream, "length": length}
        if kind == 4:
            if stream or (flags & 1 and payload) or len(payload) % 6:
                raise ValueError("Invalid HTTP/2 SETTINGS")
            settings = Reader(payload)
            frame["settings"] = []
            while settings.remaining():
                frame["settings"].append([settings.uint(2), settings.uint(4)])
        if kind == 8:
            window = Reader(payload)
            frame["window_increment"] = window.uint(4) & 0x7fffffff
            window.done()
            if not frame["window_increment"]:
                raise ValueError("Zero HTTP/2 window increment")
        if kind in (1, 9):
            part = Reader(payload)
            if kind == 1:
                if not stream or headers_stream is not None:
                    raise ValueError("Invalid HTTP/2 HEADERS stream")
                padding = part.uint(1) if flags & 8 else 0
                if flags & 32:
                    dependency = part.uint(4)
                    frame["priority"] = {"dependency": dependency & 0x7fffffff, "exclusive": bool(dependency >> 31), "weight": part.uint(1) + 1}
                if padding > part.remaining():
                    raise ValueError("Invalid HTTP/2 padding")
                block.extend(part.take(part.remaining() - padding))
                part.take(padding)
                headers_stream = stream
            else:
                if headers_stream != stream:
                    raise ValueError("Wrong HTTP/2 continuation stream")
                block.extend(payload)
            complete = bool(flags & 4)
        frames.append(frame)
    if not complete:
        raise ValueError("Incomplete first HTTP/2 request headers")
    return frames, bytes(block)


def http_signals(protocol, headers, method, target, authority):
    normal = [(name.lower(), value) for name, value in headers]
    # Dynamic listener ports are fixture routing, not a fingerprint difference.
    normal = [(name, "localhost:PORT" if name in ("host", ":authority") and value == authority else value) for name, value in normal]
    cookies = [v for n, v in normal if n == "cookie"]
    fields = {"protocol": protocol, "method": method, "target": target, "headers": normal,
              "header_order": [n for n, _ in normal], "header_count": len(normal),
              "cookie_present": bool(cookies), "referer_present": any(n == "referer" for n, _ in normal),
              "accept_language": [v for n, v in normal if n == "accept-language"],
              "cookie_names": sorted(p.strip().split("=", 1)[0] for c in cookies for p in c.split(";") if p.strip())}
    if protocol == "h1":
        fields["header_casing"] = [name for name, _ in headers]
    return {"normalized": fields, "signal_sha256": hashlib.sha256(json.dumps(fields, sort_keys=True).encode()).hexdigest(),
            "wire_headers": headers, "scheme": "transmog-http-signals-v1 (not JA4H)"}


def application(record):
    protocol = record["protocol"]
    authority = "localhost:" + record["local"].rsplit(":", 1)[1]
    frames = None
    if protocol == "h3":
        headers = record["headers"]
    elif protocol == "h2":
        from hpack import Decoder  # Tooling-only distro package in the isolated image.
        frames, block = h2_frames(bytes.fromhex(record["application_bytes"]))
        headers = Decoder(max_header_list_size=65536).decode(block)
    else:
        data = bytes.fromhex(record["application_bytes"])
        head, delimiter, _ = data.partition(b"\r\n\r\n")
        if not delimiter:
            raise ValueError("Incomplete HTTP/1 request")
        lines = head.decode("iso-8859-1").split("\r\n")
        method, target, version = lines.pop(0).split(" ")
        if version != "HTTP/1.1":
            raise ValueError("Expected HTTP/1.1")
        headers = []
        for line in lines:
            name, colon, value = line.partition(":")
            if not colon:
                raise ValueError("Malformed HTTP/1 header")
            headers.append((name, value.lstrip()))
        return {"http": http_signals(protocol, headers, method, target, authority)}
    pairs = dict(headers)
    result = {"http": http_signals(protocol, headers, pairs[":method"], pairs[":path"], authority)}
    if frames is not None:
        result["h2"] = {"frames": frames, "settings": [setting for frame in frames if frame["type"] == 4 and not frame["flags"] & 1 for setting in frame["settings"]],
                        "connection_window_increments": [f["window_increment"] for f in frames if f["type"] == 8 and f["stream"] == 0]}
    else:
        if record["settings"] is None:
            raise ValueError("Missing HTTP/3 peer SETTINGS")
        result["h3"] = {"settings": record["settings"]}
    return result


def tcp_syn(packet):
    """Linux loopback Ethernet/IP/TCP SYN; keep options and their actual order."""
    if len(packet) < 54 or packet[12:14] != b"\x08\x00":
        return None
    ip = packet[14:]
    ihl = (ip[0] & 15) * 4
    if ip[0] >> 4 != 4 or ihl < 20 or ip[9] != 6 or len(ip) < ihl + 20 or int.from_bytes(ip[6:8], "big") & 0x1fff:
        return None
    tcp = ip[ihl:]
    offset = (tcp[12] >> 4) * 4
    if tcp[13] & 0x12 != 2 or offset < 20 or len(tcp) < offset:
        return None
    r = Reader(tcp[20:offset])
    options, option_values, mss, scale = [], [], None, None
    while r.remaining():
        kind = r.uint(1)
        options.append(kind)
        if kind == 0:
            break
        if kind == 1:
            continue
        size = r.uint(1)
        if size < 2:
            raise ValueError("Invalid TCP option length")
        value = r.take(size - 2)
        option_values.append({"kind":kind, "value":value.hex()})
        if kind == 2:
            if len(value) != 2: raise ValueError("Invalid MSS option")
            mss = int.from_bytes(value, "big")
        if kind == 3:
            if len(value) != 1: raise ValueError("Invalid window scale option")
            scale = value[0]
    return {"peer": f"{socket.inet_ntoa(ip[12:16])}:{int.from_bytes(tcp[:2], 'big')}",
            "local": f"{socket.inet_ntoa(ip[16:20])}:{int.from_bytes(tcp[2:4], 'big')}",
            "normalized": {"window": int.from_bytes(tcp[14:16], "big"), "option_order": options, "mss": mss, "window_scale": scale, "ip_ttl": ip[8]},
            "option_values":option_values, "wire_packet":packet.hex(),
            "scheme": "TCP SYN inputs (not JA4T)"}


class SynCapture:
    def __init__(self):
        self.socket = socket.socket(socket.AF_PACKET, socket.SOCK_RAW, socket.htons(3))
        self.socket.bind(("lo", 0))
        self.socket.settimeout(.1)
        self.stopped = threading.Event()
        self.records, self.errors = [], []
        self.thread = threading.Thread(target=self._run, daemon=True)

    def _run(self):
        while not self.stopped.is_set():
            try:
                packet = self.socket.recv(65535)
                record = tcp_syn(packet)
                if record and len(self.records) < 1024:
                    if not any(r["peer"] == record["peer"] and r["local"] == record["local"] for r in self.records):
                        self.records.append(record)
            except TimeoutError:
                pass
            except Exception as error:
                self.errors.append(str(error))
                return

    def __enter__(self):
        self.thread.start()
        return self

    def __exit__(self, *_):
        self.stopped.set()
        self.thread.join(timeout=2)
        self.socket.close()
        if self.errors:
            raise RuntimeError("TCP SYN capture failed: " + "; ".join(self.errors))

    def find(self, record):
        if record["protocol"] == "h3":
            return {"status": "not-applicable", "reason": "QUIC uses UDP"}
        matches = [r for r in self.records if r["peer"] == record["peer"] and r["local"] == record["local"]]
        if len(matches) != 1:
            raise ValueError("Missing unambiguous origin TCP SYN")
        return matches[0]


def measure(record, capture):
    hellos = record["tls"]["client_hellos"]
    if not hellos:
        raise ValueError("No captured ClientHello")
    hello = client_hello(bytes.fromhex(hellos[0]), "q" if record["protocol"] == "h3" else "t")
    result = {"tls": hello, "negotiated": {k: v for k, v in record["tls"].items() if k != "client_hellos"},
              "client_hello_count": len(hellos), **application(record), "tcp": capture.find(record)}
    if record["protocol"] == "h3":
        initial = Reader(bytes.fromhex(record["quic_initial"]))
        initial.uint(1)
        version = initial.uint(4)
        destination, source = initial.vector(1), initial.vector(1)
        result["h3"]["quic_initial"] = {"version": version, "destination_cid_length": len(destination), "source_cid_length": len(source), "datagram_length": len(initial.data)}
    return result


def comparison_fields(measurement):
    result = {"tls": measurement["tls"]["normalized"], "ja4": measurement["tls"]["ja4"],
              "client_hello_count": measurement["client_hello_count"],
              "negotiated": measurement["negotiated"], "http": measurement["http"]["normalized"],
              "tcp": measurement["tcp"].get("normalized", measurement["tcp"])}
    for layer in ("h2", "h3"):
        if layer in measurement:
            result[layer] = measurement[layer]
    if "h3" in result:
        result["h3"] = {**result["h3"], "settings":[["GREASE", "GREASE"] if key >= 0x21 and (key - 0x21) % 31 == 0 else [key, value] for key, value in result["h3"]["settings"]]}
    return result


def flatten(value, prefix=""):
    result = {}
    if isinstance(value, dict):
        for key, child in value.items():
            result.update(flatten(child, prefix + "." + str(key) if prefix else str(key)))
    else:
        result[prefix] = value
    return result


def differences(left, right):
    a, b = flatten(left), flatten(right)
    return [{"field": key, "direct": a.get(key), "proxied": b.get(key)} for key in sorted(a.keys() | b.keys()) if a.get(key) != b.get(key)]


def write_report(directory, result):
    directory = Path(directory)
    (directory / "results.json").write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
    escape = lambda value: html.escape(json.dumps(value, ensure_ascii=False))
    parts = ["<!doctype html><meta charset=utf-8><title>Local Chromium comparison</title><style>body{font:15px system-ui;margin:2rem;max-width:1400px}table{border-collapse:collapse;width:100%}td,th{border:1px solid #aaa;padding:.5rem;text-align:left;vertical-align:top}code,pre{overflow-wrap:anywhere;white-space:pre-wrap}summary{cursor:pointer}</style>",
             "<h1>Local Chromium / Transmog comparison</h1>", "<p>Measured wire differences; a completed run does not imply fingerprint equality.</p>",
             "<pre>" + html.escape(json.dumps(result["environment"], indent=2)) + "</pre>",
             "<p>HTTP and TCP input signals cover the JA4H/JA4T surfaces without importing their separately licensed implementations. WAF scores, browser similarity, public-IP/ASN/geolocation: not applicable to this isolated local comparison.</p>"]
    parts += ["<details><summary>Fields that varied within the same client across samples</summary><pre>" + html.escape(json.dumps(result["within_client_variation"], indent=2)) + "</pre></details>"]
    parts.append("<table><tr><th>Protocol / sample<th>Direct JA4<th>Proxied JA4<th>Differing fields")
    for case in result["comparisons"]:
        parts.append(f"<tr><td>{html.escape(case['protocol'])} / {case['sample']}<td><code>{html.escape(case['direct']['tls']['ja4'])}</code><td><code>{html.escape(case['proxied']['tls']['ja4'])}</code><td>{len(case['differences'])}")
    parts.append("</table>")
    for case in result["comparisons"]:
        parts += [f"<h2>{html.escape(case['protocol'])}, sample {case['sample']}</h2>", "<table><tr><th>Field<th>Direct Chromium<th>Through Transmog"]
        for delta in case["differences"]:
            parts.append("<tr><td>" + html.escape(delta["field"]) + "<td><code>" + escape(delta["direct"]) + "</code><td><code>" + escape(delta["proxied"]) + "</code>")
        if not case["differences"]:
            parts.append("<tr><td colspan=3>No normalized differences observed in this sample.")
        parts += ["</table><details><summary>All measured signals and capture evidence</summary><pre>" + html.escape(json.dumps(case, indent=2)) + "</pre></details>"]
    (directory / "report.html").write_text("\n".join(parts), encoding="utf-8")
