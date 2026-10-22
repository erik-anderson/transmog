"""Generate ABI projections from the existing Rust models, never from a separate IDL."""
from __future__ import annotations

import json
import pprint
import re
from pathlib import Path

ROOTS = ("AppStatus", "ProductState", "ProxyStartRequest", "SessionQueryInput",
         "SessionPage", "TrafficView", "SessionSummary", "SessionDetail", "SessionHint", "AppError")
INPUTS = ("ProductState", "ProxyStartRequest", "SessionQueryInput")
PRIMITIVES = {"bool", "u8", "u16", "u32", "u64", "usize", "i8", "i16", "i32", "i64", "isize", "f32", "f64"}
TEXT = {"String", "str", "PathBuf", "SocketAddr"}
CONTAINERS = {"Option", "Vec", "BTreeMap", "HashMap", "BTreeSet", "Arc", "Box"}


def snake(name):
    return re.sub(r"(?<=[a-z0-9])(?=[A-Z])|(?<=[A-Z])(?=[A-Z][a-z])", "_", name).lower()


def pascal(name):
    return "".join(part[:1].upper() + part[1:] for part in name.split("_"))


class Models:
    def __init__(self, schema):
        self.types, self.modules = schema["types"], schema["modules"]
        self.selected = {}
        self.input_types = set()
        self.public_paths = {}
        for path, item in self.types.items():
            if item["public"] and self.modules[item["module"]]["public"]:
                self.public_paths[path] = path
        for module, item in self.modules.items():
            if not item["public"]:
                continue
            for use in item["imports"]:
                if use["public"] and use["name"] != "*":
                    try:
                        destination = self.resolve(use["path"], module)
                    except ValueError:
                        continue
                    if destination in self.types:
                        candidate = module + "::" + use["name"]
                        if len(candidate) < len(self.public_paths.get(destination, " " * 10000)):
                            self.public_paths[destination] = candidate
        for root in ROOTS:
            self.visit(self.resolve(["transmog_app", root], "transmog_app"), root in INPUTS)

    def resolve(self, parts, module, seen=frozenset()):
        key = (tuple(parts), module)
        if key in seen:
            raise ValueError(f"Import cycle: {key}")
        seen = seen | {key}
        if not parts:
            return module
        if parts[0] == "crate":
            return self.resolve([module.split("::")[0], *parts[1:]], module, seen)
        if parts[0] == "self":
            return self.resolve_absolute(module, parts[1:], seen)
        if parts[0] == "super":
            return self.resolve(parts[1:], module.rsplit("::", 1)[0], seen)
        if parts[0] in self.modules:
            return self.resolve_absolute(parts[0], parts[1:], seen)
        direct = module + "::" + parts[0]
        if direct in self.types or direct in self.modules:
            return self.resolve_absolute(direct, parts[1:], seen)
        for use in self.modules.get(module, {}).get("imports", []):
            if use["name"] == parts[0]:
                target = self.resolve(use["path"], module, seen)
                return self.resolve_absolute(target, parts[1:], seen)
        for use in self.modules.get(module, {}).get("imports", []):
            if use["name"] == "*":
                try:
                    return self.resolve_absolute(self.resolve(use["path"], module, seen), parts, seen)
                except ValueError:
                    pass
        raise ValueError(f"Cannot resolve {'::'.join(parts)} in {module}")

    def resolve_absolute(self, path, rest, seen):
        if not rest:
            return path
        return self.resolve(rest, path, seen)

    def type(self, raw, module):
        if "reference" in raw:
            return self.type(raw["reference"], module)
        if "array" in raw:
            return ("array", self.type(raw["array"], module), int(raw["length"]))
        if "slice" in raw:
            return ("Vec", self.type(raw["slice"], module))
        if "tuple" in raw:
            return ("tuple", *(self.type(t, module) for t in raw["tuple"]))
        if "path" not in raw:
            raise ValueError(f"Unsupported Rust type: {raw}")
        name = raw["path"][-1]
        if name in PRIMITIVES | TEXT:
            return (name,)
        if name in CONTAINERS:
            return (name, *(self.type(t, module) for t in raw["types"]))
        path = self.resolve(raw["path"], module)
        item = self.types[path]
        if item.get("conditional") or item.get("generic"):
            raise ValueError(f"Conditional/generic SDK model needs explicit support: {path}")
        if item["kind"] == "alias":
            return self.type(item["type"], item["module"])
        return ("model", path)

    def visit(self, path, input):
        if path in self.selected and (not input or path in self.input_types):
            return
        item = self.types[path]
        if item.get("conditional") or item.get("generic"):
            raise ValueError(f"Conditional/generic SDK model needs explicit support: {path}")
        if input:
            self.input_types.add(path)
        self.selected[path] = item
        groups = [item] if item["kind"] == "struct" else item["variants"]
        for group in groups:
            if group.get("conditional"):
                raise ValueError(f"Conditional SDK variant needs explicit support: {path}")
            for field in group["fields"]:
                if field.get("conditional") or (item["kind"] == "struct" and not field["public"]):
                    raise ValueError(f"Conditional/private SDK field needs explicit support: {path}::{field['name']}")
                field["resolved"] = self.type(field["type"], item["module"])
                def follow(t):
                    if t[0] == "model":
                        self.visit(t[1], input)
                    else:
                        for part in t[1:]:
                            if isinstance(part, tuple):
                                follow(part)
                follow(field["resolved"])

    def describe(self):
        for path, item in sorted(self.selected.items()):
            print(path, "input" if path in self.input_types else "output", self.public_paths.get(path, "NEEDS PUBLIC EXPORT"))
            groups = [item] if item["kind"] == "struct" else item["variants"]
            for group in groups:
                for field in group["fields"]:
                    if not field["public"] and item["kind"] == "struct":
                        print("  PRIVATE:", field["name"], field["resolved"])


CS_PRIMITIVES = {"bool":"bool", "u8":"byte", "u16":"ushort", "u32":"uint", "u64":"ulong", "usize":"nuint", "i8":"sbyte", "i16":"short", "i32":"int", "i64":"long", "isize":"nint", "f32":"float", "f64":"double"}
C_PRIMITIVES = {**{k:f"{k[0] == 'u' and 'u' or ''}int{k[1:]}_t" for k in ("u8","u16","u32","u64","i8","i16","i32","i64")}, "bool":"uint32_t", "usize":"size_t", "isize":"intptr_t", "f32":"float", "f64":"double"}


class Generator:
    def __init__(self, models):
        self.m = models
        self.definitions = {}
        self.order = []
        self.names = {p:i["name"] for p,i in models.selected.items()}
        if len(set(self.names.values())) != len(self.names):
            raise ValueError("Ambiguous model names require qualification")
        for path in models.selected:
            if path not in models.public_paths:
                raise ValueError(f"Expose this existing public model from its crate: {path}")
        for path in models.selected:
            self.register(("model", path))
        self.register(("Vec", ("model", models.resolve(["transmog_app", "SessionSummary"], "transmog_app"))))

    def name(self, t):
        kind = t[0]
        if kind in TEXT:
            return "Text"
        if kind in {"Arc", "Box"}:
            return self.name(t[1])
        if kind == "model":
            return self.names[t[1]]
        if kind in PRIMITIVES:
            return pascal(kind)
        if kind == "Option":
            return "Optional" + self.name(t[1])
        if kind in {"Vec", "BTreeSet"}:
            return "Slice" + self.name(t[1])
        if kind in {"BTreeMap", "HashMap"}:
            if t[1:] != (("String",), ("String",)):
                raise ValueError(f"Unsupported map: {t}")
            return "SliceTextEntry"
        raise ValueError(f"Unsupported projection: {t}")

    def c(self, t):
        return C_PRIMITIVES[t[0]] if t[0] in PRIMITIVES else "transmog_sdk_" + snake(self.name(t))

    def rust(self, t):
        k = t[0]
        if k in PRIMITIVES:
            return "u32" if k == "bool" else k
        if k in TEXT:
            return "Text"
        if k in {"Arc", "Box"}:
            return self.rust(t[1])
        if k == "Option":
            return f"Optional<{self.rust(t[1])}>"
        if k in {"Vec", "BTreeSet"}:
            return f"Slice<{self.rust(t[1])}>"
        if k in {"BTreeMap", "HashMap"}:
            return "Slice<TextEntry>"
        if k == "model":
            item = self.m.selected[t[1]]
            if item["kind"] == "enum" and not any(v["fields"] for v in item["variants"]):
                return "u32"
            return "View" + self.name(t)
        raise ValueError(t)

    def cs(self, t, native=False):
        k=t[0]
        if k in PRIMITIVES:
            return "uint" if native and k == "bool" else CS_PRIMITIVES[k]
        if k in TEXT:
            return "NativeText" if native else "string"
        if k in {"Arc", "Box"}:
            return self.cs(t[1], native)
        if k == "Option":
            return "Native"+self.name(t) if native else self.cs(t[1])+"?"
        if k in {"Vec", "BTreeSet"}:
            return "NativeSlice" if native else self.cs(t[1])+"[]"
        if k in {"BTreeMap", "HashMap"}:
            return "NativeSlice" if native else "KeyValuePair<string, string>[]"
        if k == "model":
            item=self.m.selected[t[1]]
            if item["kind"]=="enum" and not any(v["fields"] for v in item["variants"]):
                return self.name(t)
            return ("Native" if native else "")+self.name(t)
        raise ValueError(t)

    def register(self, t):
        if t[0] in PRIMITIVES | TEXT:
            return
        if t[0] in {"Arc", "Box"}:
            return self.register(t[1])
        name = self.name(t)
        if name in self.definitions:
            return
        self.definitions[name] = t
        if t[0] == "model":
            item=self.m.selected[t[1]]
            for group in ([item] if item["kind"]=="struct" else item["variants"]):
                for f in group["fields"]:
                    self.register(f["resolved"])
        elif t[0] == "Option" or t[0] in {"Vec", "BTreeSet"}:
            self.register(t[1])
        self.order.append(name)

    @staticmethod
    def field(f):
        return "item"+f["name"] if f["name"].isdigit() else f["name"]

    def enum_start(self, item):
        # Zero is reserved for an absent error/body state at the ABI boundary.
        return 1 if item["name"] in {"ErrorCategory", "BodyAvailability"} else 0

    @classmethod
    def cfield(cls, field):
        name = cls.field(field)
        reserved = {'alignas', 'alignof', 'and', 'asm', 'auto', 'bool', 'break', 'case',
                    'catch', 'char', 'class', 'const', 'constexpr', 'continue', 'default',
                    'delete', 'do', 'double', 'else', 'enum', 'explicit', 'export', 'extern',
                    'false', 'float', 'for', 'friend', 'goto', 'if', 'inline', 'int', 'long',
                    'mutable', 'namespace', 'new', 'noexcept', 'not', 'nullptr', 'operator',
                    'or', 'private', 'protected', 'public', 'register', 'requires', 'return',
                    'short', 'signed', 'sizeof', 'static', 'struct', 'switch', 'template',
                    'this', 'thread_local', 'throw', 'true', 'try', 'typedef', 'typename',
                    'union', 'unsigned', 'using', 'virtual', 'void', 'volatile', 'while', 'xor'}
        return name + '_' if name in reserved else name

    def enum_header(self, name, variants, start=0):
        c="transmog_sdk_"+snake(name)
        cpp=[f"  {v} = {i+start}," for i,v in enumerate(variants)]
        plain=[f"  TRANSMOG_{snake(name).upper()}_{snake(v).upper()} = {i+start}," for i,v in enumerate(variants)]
        if start:
            cpp.insert(0,"  None = 0,")
            plain.insert(0,f"  TRANSMOG_{snake(name).upper()}_NONE = 0,")
        return ["#ifdef __cplusplus",f"enum class {c} : uint32_t {{",*cpp,"};","#else",f"typedef uint32_t {c};","enum {",*plain,"};","#endif"]

    def header(self):
        out=["// Generated by scripts/generate-sdk-models.py from canonical Rust declarations.","// Do not edit. All text, arrays and nested pointers are borrowed from their result owner.","#pragma once",'#include "transmog/model_support.h"']
        for name in self.order:
            t=self.definitions[name];k=t[0];c="transmog_sdk_"+snake(name)
            if k=="model":
                item=self.m.selected[t[1]]
                out.extend("// "+line for line in item["docs"])
                if item["kind"]=="enum":
                    variants=item["variants"]; payload=any(v["fields"] for v in variants)
                    out+=self.enum_header(name+("Kind" if payload else ""),[v["name"] for v in variants],self.enum_start(item))
                    if not payload:
                        continue
                    for v in variants:
                        if v["fields"]:
                            vn=c+"_"+snake(v["name"])
                            out += [f"typedef struct {vn} {{",*[f"  {self.c(f['resolved'])} {self.cfield(f)};" for f in v["fields"]],f"}} {vn};"]
                    out += [f"typedef struct {c} {{",f"  {c}_kind kind;",*[f"  {c}_{snake(v['name'])} {snake(v['name'])};" for v in variants if v["fields"]],"#ifdef __cplusplus"]
                    for v in variants:
                        args=", ".join(f"{self.c(f['resolved'])} {self.cfield(f)}" for f in v["fields"])
                        assign="" if not v["fields"] else " value."+snake(v["name"])+" = {"+", ".join(self.cfield(f) for f in v["fields"])+"};"
                        out.append(f"  static {c} {v['name']}({args}) noexcept {{ {c} value{{}}; value.kind = {c}_kind::{v['name']};{assign} return value; }}")
                    out += ["#endif",f"}} {c};"]
                else:
                    out += [f"typedef struct {c} {{"]
                    for f in item["fields"]:
                        out.extend("  // "+line for line in f["docs"])
                        out.append(f"  {self.c(f['resolved'])} {self.cfield(f)};")
                    out += [f"}} {c};"]
            elif k=="Option":
                out += [f"typedef struct {c} {{", "  uint32_t has_value;",f"  {self.c(t[1])} value;",f"}} {c};"]
            else:
                element="transmog_sdk_text_entry" if k in {"BTreeMap","HashMap"} else self.c(t[1])
                out += [f"typedef struct {c} {{",f"  const {element} *data;", "  size_t size;", "#ifdef __cplusplus",f"  const {element} &operator[](size_t index) const noexcept {{ return data[index]; }}", f"  const {element} *begin() const noexcept {{ return data; }}",f"  const {element} *end() const noexcept {{ return size ? data + size : data; }}", "#endif",f"}} {c};"]
        out += ["#ifdef __cplusplus", "namespace transmog::models {", "using Text = transmog_sdk_text;"]
        for name in self.order:
            out.append(f"using {name} = transmog_sdk_{snake(name)};")
            t=self.definitions[name]
            if t[0]=="model":
                i=self.m.selected[t[1]]
                if i["kind"]=="enum" and any(v["fields"] for v in i["variants"]):
                    out.append(f"using {name}Kind = transmog_sdk_{snake(name)}_kind;")
        out += ["}", "namespace transmog::detail {", "template<class T> struct model_kind;"]
        for i,name in enumerate(ROOTS,1):
            out.append(f"template<> struct model_kind<models::{name}> {{ static constexpr uint32_t value = {i}; }};")
        out.append(f"template<> struct model_kind<models::SliceSessionSummary> {{ static constexpr uint32_t value = {len(ROOTS)+1}; }};")
        out += ["}", "#endif", ""]
        return "\n".join(out)

    def rust_code(self):
        out=["// Generated from the canonical Rust models. Do not edit.","use super::abi::*;", "use transmog_app::AppError;"]
        for name in self.order:
            t=self.definitions[name]
            if t[0]!="model":
                continue
            item=self.m.selected[t[1]];path=self.m.public_paths[t[1]];view=self.rust(t)
            payload=item["kind"]=="enum" and any(v["fields"] for v in item["variants"])
            if item["kind"]=="struct":
                out += ["#[repr(C)]", "#[derive(Clone, Copy, Default)]",f"pub struct {view} {{",*[f"    pub {self.field(f)}: {self.rust(f['resolved'])}," for f in item["fields"]],"}"]
                project=f"Ok({view} {{ "+", ".join(f"{self.field(f)}: self.{f['name']}.project(arena)?" for f in item["fields"])+" })"
                read=f"Ok(Self {{ "+", ".join(f"{f['name']}: unsafe {{ FromAbi::read(&value.{self.field(f)}, budget) }}?" for f in item["fields"])+" })"
                if item.get("tuple"):
                    raise ValueError("Tuple structs need an explicit projection")
            else:
                if payload:
                    for v in item["variants"]:
                        if v["fields"]:
                            out += ["#[repr(C)]", "#[derive(Clone, Copy, Default)]",f"pub struct {view}{v['name']} {{",*[f"    pub {self.field(f)}: {self.rust(f['resolved'])}," for f in v["fields"]],"}"]
                    out += ["#[repr(C)]", "#[derive(Clone, Copy, Default)]",f"pub struct {view} {{", "    pub kind: u32,",*[f"    pub {snake(v['name'])}: {view}{v['name']}," for v in item["variants"] if v["fields"]],"}"]
                arms=[];reads=[]
                for index,v in enumerate(item["variants"],self.enum_start(item)):
                    fields=v["fields"];vn=v["name"]
                    bindings=", ".join((self.field(f) if v.get("tuple") else f["name"]) for f in fields)
                    pattern=f"Self::{vn}"+((f"({bindings})" if v.get("tuple") else f" {{ {bindings} }}") if fields else "")
                    if payload:
                        defaults = "..Default::default()" if not fields or sum(bool(v['fields']) for v in item['variants']) > 1 else ""
                        projected_fields = ", ".join(f"{self.field(f)}: {self.field(f)}.project(arena)?" for f in fields)
                        active_payload = f"{snake(vn)}: {view}{vn} {{ {projected_fields} }}, " if fields else ""
                        init = f"{view} {{ kind: {index}, {active_payload}{defaults} }}"
                    else:
                        init = str(index)
                    arms.append(f"{pattern} => {init},")
                    values=", ".join(("" if v.get("tuple") else f["name"]+": ")+f"unsafe {{ FromAbi::read(&value.{snake(vn)}.{self.field(f)}, budget) }}?" for f in fields)
                    construct=f"Self::{vn}"+((f"({values})" if v.get("tuple") else f" {{ {values} }}") if fields else "")
                    reads.append(f"{index} => {construct},")
                if payload:
                    project="Ok(match self { "+" ".join(arms)+" })"
                else:
                    number = snake(name) + "_number"
                    out += [f"pub fn {number}(value: &{path}) -> u32 {{",
                            "    match value { " + " ".join(arms).replace("Self::", path + "::") + " }", "}"]
                    project = f"Ok({number}(self))"
                read="Ok(match "+("value.kind" if payload else "*value")+" { "+" ".join(reads)+f' _ => return Err(invalid("Invalid {name} discriminant")), '+"})"
            out += [f"impl Project for {path} {{", f"    type View = {view};", "    fn project(&self, arena: &mut Arena) -> Result<Self::View, AppError> {", "        let _ = &arena;", "        "+project, "    }", "}"]
            if t[1] in self.m.input_types:
                out += [f"impl FromAbi for {path} {{", "    unsafe fn read(value: &Self::View, budget: &mut InputBudget) -> Result<Self, AppError> {", "        let _ = &budget;", "        "+read, "    }", "}"]
        for i,name in enumerate(ROOTS,1):
            path=self.m.resolve(["transmog_app",name],"transmog_app")
            out.append(f"impl Model for {self.m.public_paths[path]} {{ const KIND: u32 = {i}; }}")
        out.append(f"impl Model for Vec<transmog_app::SessionSummary> {{ const KIND: u32 = {len(ROOTS)+1}; }}")
        out += ["pub fn layout(index: usize) -> usize {", "    match index {"]
        # Used by the native wrapper to reject a layout mismatch before exposing a pointer.
        layouts=[self.rust(self.definitions[n]) for n in self.order]
        for i,t in enumerate(layouts):
            out += [f"        {2*i} => std::mem::size_of::<{t}>(),",f"        {2*i+1} => std::mem::align_of::<{t}>(),"]
        out += ["        _ => 0,", "    }", "}", ""]
        return "\n".join(out)

    def layout_cpp(self):
        return "// Generated layout checks.\n"+"\n".join(f"if (transmog_sdk_rust::model_layout({2*i}) != sizeof(transmog_sdk_{snake(n)}) || transmog_sdk_rust::model_layout({2*i+1}) != alignof(transmog_sdk_{snake(n)})) return false;" for i,n in enumerate(self.order))+"\n"

    def test_layout_cpp(self):
        return "// Generated native test fixture layouts.\n"+"\n".join(f"case {2*i}: return sizeof(transmog_sdk_{snake(n)});\ncase {2*i+1}: return alignof(transmog_sdk_{snake(n)});" for i,n in enumerate(self.order))+"\n"

    def python_fixture(self):
        # Test-only descriptors for ctypes; no independently authored model/schema.
        definitions = {}
        def typename(t):
            return t[0] if t[0] in PRIMITIVES else self.name(t)
        def fields(group):
            return [(self.field(f), typename(f['resolved'])) for f in group['fields']]
        for name in self.order:
            t=self.definitions[name]; k=t[0]
            if k=='model':
                item=self.m.selected[t[1]]
                definitions[name] = ('struct', fields(item)) if item['kind']=='struct' else ('enum', self.enum_start(item), [(v['name'], fields(v)) for v in item['variants']])
            elif k=='Option':
                definitions[name] = ('optional', typename(t[1]))
            else:
                definitions[name] = ('slice', 'TextEntry' if k in {'BTreeMap','HashMap'} else typename(t[1]))
        return '# Generated test-only ctypes descriptors from the Rust models.\nDEFINITIONS = '+pprint.pformat(definitions, width=110, sort_dicts=False)+'\n'

    def cs_read(self, t, expression):
        k=t[0]
        if k in PRIMITIVES:
            return expression+" != 0" if k=="bool" else expression
        if k in TEXT:
            return f"ModelMemory.Text({expression})"
        if k in {"Arc","Box"}:
            return self.cs_read(t[1],expression)
        if k=="Option":
            return f"({expression}.HasValue != 0 ? {self.cs_read(t[1],expression+'.Value')} : null)"
        if k in {"Vec","BTreeSet"}:
            return f"ModelMemory.Array<{self.cs(t[1],True)}, {self.cs(t[1])}>({expression}, static item => {self.cs_read(t[1],'item')})"
        if k in {"BTreeMap","HashMap"}:
            return f"ModelMemory.Array<NativeTextEntry, KeyValuePair<string, string>>({expression}, static item => new(ModelMemory.Text(item.Key), ModelMemory.Text(item.Value)))"
        item=self.m.selected[t[1]]
        if item["kind"]=="enum" and not any(v["fields"] for v in item["variants"]):
            return expression
        return f"Read{self.name(t)}({expression})"

    def cs_write(self,t,e):
        k=t[0]
        if k in PRIMITIVES:
            return f"({e} ? 1u : 0u)" if k=="bool" else e
        if k in TEXT:
            return f"arena.Text({e})"
        if k=="Option":
            inner=t[1];item=self.m.selected[inner[1]] if inner[0]=="model" else None
            is_value=inner[0] in PRIMITIVES or (item and item["kind"]=="enum" and not any(v["fields"] for v in item["variants"]))
            value=e+".Value" if is_value else e
            return f"({e} is null ? default : new Native{self.name(t)} {{ HasValue = 1, Value = {self.cs_write(inner,value)} }})"
        if k in {"Vec","BTreeSet"}:
            return f"arena.Array<{self.cs(t[1])}, {self.cs(t[1],True)}>({e}, item => {self.cs_write(t[1],'item')})"
        if k in {"BTreeMap","HashMap"}:
            return f"arena.Array<KeyValuePair<string, string>, NativeTextEntry>({e}, item => new() {{ Key = arena.Text(item.Key), Value = arena.Text(item.Value) }})"
        if k in {"Arc","Box"}:
            return self.cs_write(t[1],e)
        item=self.m.selected[t[1]]
        if item["kind"]=="enum" and not any(v["fields"] for v in item["variants"]):
            return e
        return f"Write{self.name(t)}({e}, arena)"

    def csharp(self):
        out=["// <auto-generated />", "// Canonical Rust model projections; regenerate with scripts/generate-sdk-models.py.", "#nullable enable", "using System.Runtime.InteropServices;", "namespace Transmog.Sdk.Models;", ""]
        for name in self.order:
            t=self.definitions[name];k=t[0]
            if k=="model":
                item=self.m.selected[t[1]]
                out.extend("// "+d for d in item["docs"])
                if item["kind"]=="enum":
                    payload=any(v["fields"] for v in item["variants"])
                    if not payload:
                        out += [f"public enum {name} : uint", "{"]
                        if self.enum_start(item):
                            out.append("    None = 0,")
                        out += [f"    {v['name']} = {i}," for i,v in enumerate(item["variants"],self.enum_start(item))]+["}"]
                        continue
                    out += [f"public abstract record {name}","{"]
                    for v in item["variants"]:
                        args=", ".join(f"{self.cs(f['resolved'])} {pascal(self.field(f))}" for f in v["fields"])
                        out.append(f"    public sealed record {v['name']}({args}) : {name};")
                    out += ["}"]
                    for v in item["variants"]:
                        if v["fields"]:
                            out += ["[StructLayout(LayoutKind.Sequential)]",f"internal struct Native{name}{v['name']}","{",*[f"    internal {self.cs(f['resolved'],True)} {pascal(self.field(f))};" for f in v["fields"]],"}"]
                    out += ["[StructLayout(LayoutKind.Sequential)]",f"internal struct Native{name}","{","    internal uint Kind;",*[f"    internal Native{name}{v['name']} {v['name']};" for v in item["variants"] if v["fields"]],"}"]
                else:
                    args=",\n    ".join(f"{self.cs(f['resolved'])} {pascal(self.field(f))}" for f in item["fields"])
                    out.append(f"public sealed record {name}(\n    {args});")
                    out += ["[StructLayout(LayoutKind.Sequential)]",f"internal struct Native{name}","{",*[f"    internal {self.cs(f['resolved'],True)} {pascal(self.field(f))};" for f in item["fields"]],"}"]
            elif k=="Option":
                out += ["[StructLayout(LayoutKind.Sequential)]",f"internal struct Native{name}","{","    internal uint HasValue;",f"    internal {self.cs(t[1],True)} Value;","}"]
        out += ["internal static unsafe class ModelMarshal", "{"]
        for name in self.order:
            t=self.definitions[name]
            if t[0]!="model":
                continue
            item=self.m.selected[t[1]]
            if item["kind"]=="enum" and not any(v["fields"] for v in item["variants"]):
                continue
            out.append(f"    internal static {name} Read{name}(Native{name} value) =>")
            if item["kind"]=="struct":
                out += ["        new("+",\n            ".join(self.cs_read(f["resolved"],"value."+pascal(self.field(f))) for f in item["fields"])+");"]
            else:
                out += ["        value.Kind switch", "        {"]
                for i,v in enumerate(item["variants"],self.enum_start(item)):
                    args=", ".join(self.cs_read(f["resolved"],"value."+v["name"]+"."+pascal(self.field(f))) for f in v["fields"])
                    out.append(f"            {i} => new {name}.{v['name']}({args}),")
                out += [f'            _ => throw new InvalidOperationException("Unknown native {name} discriminant."),', "        };"]
            if t[1] in self.m.input_types:
                out.append(f"    internal static Native{name} Write{name}({name} value, InputArena arena) =>")
                if item["kind"]=="struct":
                    out += ["        new()", "        {",*[f"            {pascal(self.field(f))} = {self.cs_write(f['resolved'],'value.'+pascal(self.field(f)))}," for f in item["fields"]],"        };"]
                else:
                    out += ["        value switch", "        {"]
                    for i,v in enumerate(item["variants"],self.enum_start(item)):
                        fields=", ".join(f"{pascal(self.field(f))} = {self.cs_write(f['resolved'],'option.'+pascal(self.field(f)))}" for f in v["fields"])
                        payload=f", {v['name']} = new Native{name}{v['name']} {{ {fields} }}" if v["fields"] else ""
                        out.append(f"            {name}.{v['name']} option => new() {{ Kind = {i}{payload} }},")
                    out += [f'            _ => throw new ArgumentException("Invalid {name}.", nameof(value)),',"        };"]
        for name in ROOTS:
            out += [f'    [DllImport("transmog_sdk", CallingConvention = CallingConvention.Cdecl, ExactSpelling = true, EntryPoint = "transmog_sdk_result_as_{snake(name)}")]',f"    private static extern nint Native{name}Result(nint result);",f"    internal static {name} Read{name}Result(nint result) => Read{name}(ModelMemory.Value<Native{name}>(Native{name}Result(result)));"]
        out += ['    [DllImport("transmog_sdk", CallingConvention = CallingConvention.Cdecl, ExactSpelling = true, EntryPoint = "transmog_sdk_result_as_session_rows")]',"    private static extern nint NativeSessionRowsResult(nint result);", "    internal static SessionSummary[] ReadSessionRowsResult(nint result) => ModelMemory.Array<NativeSessionSummary, SessionSummary>(ModelMemory.Value<NativeSlice>(NativeSessionRowsResult(result)), ReadSessionSummary);", "}", ""]
        return "\n".join(out)


if __name__ == "__main__":
    import sys
    Models(json.loads(Path(sys.argv[1]).read_text(encoding="utf-8-sig"))).describe()
