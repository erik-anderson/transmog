"""Snapshot protocol enum data only; never translate parser code or .NET types."""
import argparse
import ast
import json
import re

from mapi_reference import ROOT, ensure_sources


def number(expression, previous):
    tree = ast.parse(re.sub(r"(?<=[0-9a-fA-F])[uUlL]+\b", "", expression), mode="eval")

    def evaluate(node):
        if isinstance(node, ast.Constant) and isinstance(node.value, int):
            return node.value
        if isinstance(node, ast.Name):
            return previous[node.id]
        if isinstance(node, ast.BinOp) and isinstance(node.op, ast.BitOr):
            return evaluate(node.left) | evaluate(node.right)
        raise ValueError(f"Unexpected enum expression: {expression}")

    return evaluate(tree.body)


def render():
    reference = ensure_sources()
    widths = {"byte": "u8", "sbyte": "i8", "ushort": "u16", "short": "i16",
              "uint": "u32", "int": "i32", "ulong": "u64", "long": "i64"}
    enums = {}
    for path in sorted(reference.rglob("*.cs")):
        source = path.read_text(encoding="utf-8-sig")
        source = re.sub(r"/\*.*?\*/|//[^\n]*", "", source, flags=re.S)
        for match in re.finditer(r"\bpublic\s+enum\s+(\w+)\s*(?::\s*(\w+))?\s*\{([^}]+)\}", source):
            name, underlying, body = match.groups()
            values = {}
            current = -1
            for item in body.split(","):
                item = item.strip()
                if not item:
                    continue
                parts = item.split("=", 1)
                current = number(parts[1].strip(), values) if len(parts) == 2 else current + 1
                values[parts[0].strip()] = current
            enums[name] = {"wire": widths[underlying or "int"],
                           "flags": bool(re.search(r"\[Flags\]\s*$", source[:match.start()])),
                           "values": values}
    data = json.dumps(dict(sorted(enums.items())), indent=2, ensure_ascii=False)
    data = re.sub(r'^(\s*)"([A-Za-z_$][\w$]*)":', r"\1\2:", data, flags=re.M)
    # Keep the generated data in the same conventional format as authored TS.
    lines = data.splitlines()
    data = "\n".join(line if index == len(lines) - 1 or line.endswith(("{", ",")) else line + ","
                     for index, line in enumerate(lines))
    return ("// Protocol constants from the pinned Microsoft reference; no executable parser code.\n"
            "// Refresh with python scripts/generate-mapi-constants.py.\n"
            'import type { EnumDefinition } from "./record.js";\n\n'
            "export const enums: Readonly<Record<string, EnumDefinition>> = "
            + data
            + ";\n")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true")
    arguments = parser.parse_args()
    target = ROOT / "extensions/mapi/typescript/constants.ts"
    source = render()
    if arguments.check:
        if target.read_text(encoding="utf-8") != source:
            raise SystemExit("MAPI protocol constants are stale")
    else:
        target.write_text(source, encoding="utf-8", newline="\n")


if __name__ == "__main__":
    main()
