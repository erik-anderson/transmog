"""Conservatively repack ICU CmnD data without changing retained resources.

The offset table format is defined by ICU's source/common/ucmndata.h. Only the
directory and padding are regenerated; retained resource bytes remain identical.
"""
from __future__ import annotations

from pathlib import PurePosixPath
import struct


def read_package(data):
    if (len(data) < 24 or data[2:4] != b"\xda\x27" or data[8:11] != b"\0\0\2"
            or data[12:17] != b"CmnD\1"):
        raise ValueError("Expected little-endian ICU CmnD version 1 data")
    header_size = struct.unpack_from("<H", data)[0]
    if header_size < 24 or header_size % 16 or header_size + 4 > len(data):
        raise ValueError("Invalid ICU package header")
    count = struct.unpack_from("<I", data, header_size)[0]
    table_end = 4 + count * 8
    size = len(data) - header_size
    if not 0 < count <= 65536 or table_end >= size:
        raise ValueError("Invalid ICU package directory")
    offsets = [struct.unpack_from("<II", data, header_size + 4 + index * 8) for index in range(count)]
    starts = sorted({offset for _, offset in offsets})
    if starts[0] <= table_end or starts[-1] >= size or any(offset % 16 for offset in starts):
        raise ValueError("Invalid ICU resource offsets")
    ends = dict(zip(starts, starts[1:] + [size]))
    resources = {}
    previous = ""
    prefix = None
    for name_offset, offset in offsets:
        if not table_end <= name_offset < starts[0]:
            raise ValueError("Invalid ICU resource name offset")
        try:
            end = data.index(b"\0", header_size + name_offset, header_size + starts[0])
            name = data[header_size + name_offset:end].decode("ascii")
        except (ValueError, UnicodeDecodeError) as error:
            raise ValueError("Invalid ICU resource name") from error
        path = PurePosixPath(name)
        if (len(path.parts) < 2 or name.startswith("/") or "\\" in name or ":" in name
                or ".." in path.parts or name <= previous):
            raise ValueError("Invalid or unsorted ICU resource names")
        prefix = prefix or path.parts[0]
        if path.parts[0] != prefix:
            raise ValueError("Mixed ICU package namespaces")
        previous = name
        resources[name] = data[header_size + offset:header_size + ends[offset]]
    return data[:header_size], resources


def write_package(header, resources):
    body = bytearray(4 + 8 * len(resources))
    struct.pack_into("<I", body, 0, len(resources))
    names = []
    for name in sorted(resources):
        names.append((name, len(body)))
        body.extend(name.encode("ascii") + b"\0")
    body.extend(b"\xaa" * (-len(body) % 16))
    for index, (name, name_offset) in enumerate(names):
        struct.pack_into("<II", body, 4 + index * 8, name_offset, len(body))
        body.extend(resources[name])
        body.extend(b"\xaa" * (-len(body) % 16))
    return header + body


def filter_package(data, policy):
    if set(policy) != {"excludedSuffixes", "excludedFiles"}:
        raise ValueError("Unknown ICU data filter policy")
    header, resources = read_package(data)
    suffixes = tuple(policy["excludedSuffixes"])
    filenames = set(policy["excludedFiles"])
    retained = {name: content for name, content in resources.items()
                if not name.endswith(suffixes) and PurePosixPath(name).name not in filenames}
    if not retained:
        raise ValueError("ICU data filter removed every resource")
    result = write_package(header, retained)
    if read_package(result)[1] != retained:
        raise ValueError("ICU data filter changed retained resource bytes")
    return result
