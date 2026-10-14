"""Rebuild the bundled crs.pb offline from pinned, public Chromium source.

This deliberately small textproto encoder supports the fields in the checked-in
root_store.proto. Unknown fields or syntax fail rather than silently dropping
policy. No Google component package is downloaded or redistributed.
"""
import ast
import base64
import hashlib
import json
from pathlib import Path
import re

ROOT = Path(__file__).resolve().parents[1]
DATA = ROOT / "third_party/chromium-root-store"
REVISION = "6baa586063d26e723f468ec8f213069fdb2d120c"
CONSTRAINTS = {
    "sct_not_after_sec": (1, "int"), "sct_all_after_sec": (2, "int"),
    "min_version": (3, "string"), "max_version_exclusive": (4, "string"),
    "permitted_dns_names": (5, "string"), "serial_not_after": (8, "int"),
    "serial_after": (9, "int"), "validity_starts_not_after_sec": (10, "int"),
    "validity_starts_after_sec": (11, "int"),
}
ANCHOR = {
    "der": (1, "bytes"), "sha256_hex": (2, "string"),
    "ev_policy_oids": (3, "string"), "constraints": (4, CONSTRAINTS),
    "display_name": (5, "string"), "eutl": (6, "bool"),
    "enforce_anchor_expiry": (8, "bool"), "enforce_anchor_constraints": (9, "bool"),
    "tls_trust_anchor": (10, "bool"), "trust_anchor_id": (11, "bytes"),
    "crs_root_id": (12, "int"),
}
STORE = {"trust_anchors": (1, ANCHOR), "version_major": (2, "int"),
         "additional_certs": (3, ANCHOR)}
TOKEN = re.compile(r'\s+|#[^\n]*|"(?:[^"\\]|\\.)*"|[A-Za-z_][\w]*|-?\d+|[{}:]', re.S)


def parse(text):
    tokens = []
    offset = 0
    while offset < len(text):
        match = TOKEN.match(text, offset)
        if not match:
            raise ValueError(f"Unsupported textproto syntax at {offset}")
        token = match.group()
        offset = match.end()
        if not token.isspace() and not token.startswith("#"):
            tokens.append(token)
    cursor = 0

    def message(nested=False):
        nonlocal cursor
        result = []
        while cursor < len(tokens) and tokens[cursor] != "}":
            name = tokens[cursor]
            cursor += 1
            if tokens[cursor] == ":":
                cursor += 1
            if tokens[cursor] == "{":
                cursor += 1
                value = message(True)
            else:
                value = tokens[cursor]
                cursor += 1
            result.append((name, value))
        if nested:
            if cursor >= len(tokens):
                raise ValueError("Unclosed message")
            cursor += 1
        return result

    result = message()
    if cursor != len(tokens):
        raise ValueError("Unexpected closing brace")
    return result


def varint(value):
    value &= (1 << 64) - 1
    result = bytearray()
    while value >= 128:
        result.append((value & 127) | 128)
        value >>= 7
    result.append(value)
    return bytes(result)


def encode(fields, schema, certificates):
    result = bytearray()
    for name, value in fields:
        number, kind = schema[name]  # Fail closed on newly added fields.
        if name == "sha256_hex":
            number, kind, payload = 1, "bytes", certificates[ast.literal_eval(value).lower()]
        elif isinstance(kind, dict):
            payload = encode(value, kind, certificates)
        elif kind in ("string", "bytes"):
            payload = ast.literal_eval(value).encode("utf-8" if kind == "string" else "latin1")
        elif kind == "bool":
            if value not in ("true", "false"):
                raise ValueError("Invalid boolean")
            payload = int(value == "true")
        else:
            payload = int(value)
        if isinstance(payload, int):
            result += varint(number << 3) + varint(payload)
        else:
            result += varint((number << 3) | 2) + varint(len(payload)) + payload
    return bytes(result)


def main():
    certificates = {}
    for name in ("root_store.certs", "additional.certs"):
        text = (DATA / name).read_text(encoding="utf-8")
        for pem in re.findall(r"-----BEGIN CERTIFICATE-----(.*?)-----END CERTIFICATE-----", text, re.S):
            der = base64.b64decode(re.sub(r"\s", "", pem), validate=True)
            certificates[hashlib.sha256(der).hexdigest()] = der
    fields = parse((DATA / "root_store.textproto").read_text(encoding="utf-8"))
    output = encode(fields, STORE, certificates)
    (DATA / "crs.pb").write_bytes(output)
    inputs = {name: hashlib.sha256((DATA / name).read_bytes()).hexdigest() for name in
              ("root_store.proto", "root_store.textproto", "root_store.certs", "additional.certs", "LICENSE")}
    receipt = {"sourceRevision": REVISION, "rootStoreVersion": int(dict(fields)["version_major"]),
               "source": f"https://chromium.googlesource.com/chromium/src/+/{REVISION}/",
               "inputSha256": inputs, "protobufSha256": hashlib.sha256(output).hexdigest(),
               "protobufBytes": len(output)}
    (DATA / "snapshot.json").write_text(json.dumps(receipt, indent=2) + "\n", encoding="utf-8", newline="\n")
    print(json.dumps(receipt, indent=2))


if __name__ == "__main__":
    main()
