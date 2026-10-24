"""Create binary SAZ evidence, including independent reference-serializer outputs."""
import gzip
import json
from pathlib import Path
import subprocess
import sys
import zipfile
import zlib


def main():
    output = Path(sys.argv[1])
    protobuf = bytes.fromhex("089601120568656c6c6f1a03089601")
    bond = bytes.fromhex("2903332e3000")
    compressor = zlib.compressobj(wbits=-15)
    raw_deflate = compressor.compress(bond) + compressor.flush()
    limited_versions = bytes([0x85, 0x34]) + bytes([10, 1, 0]) * 2220 + bytes([0])
    cases = [
        ("protobuf", "application/protobuf", protobuf, ""),
        ("protobuf-gzip", "application/x-protobuf", gzip.compress(protobuf, mtime=0), "gzip"),
        ("bond", "application/bond-compact-binary", bond, ""),
        ("bond-deflate", "application/bond-compact-binary", raw_deflate, "deflate"),
        ("bond-v2", "application/bond-compact-binary", bytes.fromhex("43420200082903332e30420100"), ""),
        ("bond-ambiguous", "application/bond-compact-binary", bytes.fromhex("020100"), ""),
        ("bond-framing", "application/bond-compact-binary", bytes.fromhex("43420200062903332e3000"), ""),
        ("protobuf-malformed", "application/protobuf", bytes.fromhex("0801120561"), ""),
        ("protobuf-negative-zero", "application/protobuf", bytes.fromhex("0d00000080110000000000000080"), ""),
        ("bond-negative-zero", "application/bond-compact-binary", bytes.fromhex("43420100070000008028000000000000008000"), ""),
        ("protobuf-duplicate-identity", "application/protobuf", protobuf, ("identity", "identity")),
        ("protobuf-invalid-identity", "application/protobuf", protobuf, "identity\xa0"),
        ("bond-version-limited", "application/bond-compact-binary", limited_versions, ""),
        ("bond-framing-limited", "application/bond-compact-binary", bytes.fromhex("43420200") + limited_versions, ""),
    ]
    reference_script = Path(__file__).with_name('binary-reference-fixtures.mjs')
    references = json.loads(subprocess.check_output(['node', str(reference_script)], text=True))
    cases.extend((case['name'], case['contentType'], bytes.fromhex(case['bodyHex']), '') for case in references)
    output.parent.mkdir(parents=True, exist_ok=True)
    with zipfile.ZipFile(output, "w", compression=zipfile.ZIP_DEFLATED) as archive:
        for number, (name, content_type, body, coding) in enumerate(cases, 1):
            headers = f"Content-Type: {content_type}\r\nContent-Length: {len(body)}\r\n"
            if coding:
                for value in coding if isinstance(coding, tuple) else (coding,):
                    headers += f"Content-Encoding: {value}\r\n"
            request = f"POST https://binary.invalid/{name} HTTP/1.1\r\n{headers}\r\n".encode("latin1") + body
            response = f"HTTP/1.1 200 OK\r\n{headers}\r\n".encode("latin1") + body
            archive.writestr(f"raw/{number}_c.txt", request)
            archive.writestr(f"raw/{number}_s.txt", response)
    print(f"Wrote {len(cases)} synthetic binary exchanges: {output}")


if __name__ == "__main__":
    main()
