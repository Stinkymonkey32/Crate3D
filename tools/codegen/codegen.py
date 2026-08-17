#!/usr/bin/env python3
"""BloxVM API code generator.

Parses the Roblox Creator Documentation engine reference (YAML) and emits
Rust source files into the crate's `generated/` directory:

  classes.rs     - class registry (inheritance + member metadata)
  datatypes.rs   - value-type structs + datatype registry
  enums.rs       - 525 Rust enums + enum registry
  globals.rs     - Lua / Roblox globals registry
  libraries.rs   - library (task, string, ...) registry

Usage:
  python tools/codegen/codegen.py [--docs DIR] [--out DIR]

Regenerate whenever the Creator Docs snapshot is updated.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import sys
import yaml

RUST_KEYWORDS = {
    "as", "break", "const", "continue", "crate", "else", "enum", "extern",
    "false", "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod",
    "move", "mut", "pub", "ref", "return", "self", "Self", "static", "struct",
    "super", "trait", "true", "type", "unsafe", "use", "where", "while",
    "async", "await", "dyn", "abstract", "become", "box", "do", "final",
    "macro", "override", "priv", "typeof", "unsized", "virtual", "yield",
    "try", "gen",
}

IDENT_RE = re.compile(r"^[A-Za-z_][A-Za-z0-9_]*$")

# Datatype name -> concrete Rust struct field table. Curated: which properties
# from the docs are stored fields (vs computed), and their Rust types.
FIELD_TABLE: dict[str, dict] = {
    "Vector2": dict(
        fields=[("x", "f64"), ("y", "f64")],
        consts={"ZERO": "(0.0, 0.0)", "ONE": "(1.0, 1.0)", "X_AXIS": "(1.0, 0.0)", "Y_AXIS": "(0.0, 1.0)"},
    ),
    "Vector3": dict(
        fields=[("x", "f64"), ("y", "f64"), ("z", "f64")],
        consts={"ZERO": "(0.0, 0.0, 0.0)", "ONE": "(1.0, 1.0, 1.0)", "X_AXIS": "(1.0, 0.0, 0.0)", "Y_AXIS": "(0.0, 1.0, 0.0)", "Z_AXIS": "(0.0, 0.0, 1.0)"},
    ),
    "Vector2int16": dict(fields=[("x", "i16"), ("y", "i16")], consts={}),
    "Vector3int16": dict(fields=[("x", "i16"), ("y", "i16"), ("z", "i16")], consts={}),
    "Color3": dict(fields=[("r", "f32"), ("g", "f32"), ("b", "f32")], consts={"WHITE": "(1.0, 1.0, 1.0)", "BLACK": "(0.0, 0.0, 0.0)"}),
    "UDim": dict(fields=[("scale", "f64"), ("offset", "i32")], consts={}),
    "UDim2": dict(fields=[("x", "UDim"), ("y", "UDim")], consts={}),
    "CFrame": dict(
        fields=[("r00", "f64"), ("r01", "f64"), ("r02", "f64"), ("r10", "f64"), ("r11", "f64"), ("r12", "f64"), ("r20", "f64"), ("r21", "f64"), ("r22", "f64"), ("x", "f64"), ("y", "f64"), ("z", "f64")],
        consts={"IDENTITY": "(1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0)"},
    ),
    "NumberRange": dict(fields=[("min", "f64"), ("max", "f64")], consts={}),
    "Rect": dict(fields=[("min", "Vector2"), ("max", "Vector2")], consts={}),
    "Region3int16": dict(fields=[("min", "Vector3int16"), ("max", "Vector3int16")], consts={}),
    "Ray": dict(fields=[("origin", "Vector3"), ("direction", "Vector3")], consts={}),
    "PhysicalProperties": dict(fields=[("density", "f64"), ("friction", "f64"), ("elasticity", "f64"), ("friction_weight", "f64"), ("elasticity_weight", "f64"), ("acoustic_absorption", "f64")], consts={}),
    "BrickColor": dict(fields=[("number", "i32")], consts={}),
    "DateTime": dict(fields=[("unix_timestamp", "i64"), ("unix_timestamp_millis", "i64")], consts={}),
    "Axes": dict(fields=[("x", "bool"), ("y", "bool"), ("z", "bool")], consts={}),
    "Faces": dict(fields=[("top", "bool"), ("bottom", "bool"), ("left", "bool"), ("right", "bool"), ("back", "bool"), ("front", "bool")], consts={}),
    "NumberSequenceKeypoint": dict(fields=[("time", "f64"), ("value", "f64"), ("envelope", "f64")], consts={}),
    "ColorSequenceKeypoint": dict(fields=[("time", "f64"), ("value", "Color3")], consts={}),
    "FloatCurveKey": dict(fields=[("interpolation", "KeyInterpolationMode"), ("time", "f64"), ("value", "f64"), ("right_tangent", "f64"), ("left_tangent", "f64")], consts={}),
    "PathWaypoint": dict(fields=[("action", "PathWaypointAction"), ("position", "Vector3"), ("label", "String")], consts={}),
    "TweenInfo": dict(fields=[("time", "f64"), ("delay_time", "f64"), ("repeat_count", "i32"), ("easing_style", "EasingStyle"), ("easing_direction", "EasingDirection"), ("reverses", "bool")], consts={}),
    "User": dict(fields=[("id", "i64"), ("domain_type", "DomainType"), ("domain_id", "i64")], consts={}),
    "DockWidgetPluginGuiInfo": dict(fields=[("initial_enabled", "bool"), ("initial_enabled_should_override_restore", "bool"), ("floating_x_size", "i32"), ("floating_y_size", "i32"), ("min_width", "i32"), ("min_height", "i32")], consts={}),
}


def rust_str(s) -> str:
    """Escape a string for a Rust string literal."""
    out = []
    for ch in s:
        o = ord(ch)
        if ch == "\\":
            out.append("\\\\")
        elif ch == '"':
            out.append('\\"')
        elif ch == "\n":
            out.append("\\n")
        elif ch == "\r":
            out.append("\\r")
        elif ch == "\t":
            out.append("\\t")
        elif o < 0x20:
            out.append("\\u{%x}" % o)
        else:
            out.append(ch)
    return '"' + "".join(out) + '"'


def rust_ident(name: str) -> str:
    """Mangle a Roblox identifier into a valid, non-keyword Rust identifier."""
    ident = name
    if ident in RUST_KEYWORDS:
        ident += "_"
    return ident


PREFIX_RE = re.compile(r"^[A-Za-z_][A-Za-z0-9_]*[.:]")


def bare_name(name: str) -> str:
    """Strip the `<Type>.` / `<Type>:` prefix from a member name.

    Leaves names like `...` (varargs) and unprefixed names untouched.
    """
    return PREFIX_RE.sub("", name)


def load_docs(docs_dir: str) -> dict:
    """Load all YAML engine reference files, keyed by (type, name).

    Some names exist under multiple types (e.g. `Instance` is both a class and
    a datatype), so the type is part of the key.
    """
    docs: dict = {}
    errors = []
    for root, _dirs, files in os.walk(docs_dir):
        for f in sorted(files):
            if not f.endswith(".yaml"):
                continue
            path = os.path.join(root, f)
            try:
                doc = yaml.safe_load(open(path, encoding="utf-8"))
            except Exception as exc:
                errors.append(f"{os.path.relpath(path, docs_dir)}: {exc}")
                continue
            if not isinstance(doc, dict) or "name" not in doc:
                errors.append(f"{os.path.relpath(path, docs_dir)}: missing 'name'")
                continue
            docs[(doc.get("type", "?"), doc["name"])] = doc
    if errors:
        print("WARN: %d file(s) failed to parse:" % len(errors), file=sys.stderr)
        for e in errors[:20]:
            print("  " + e, file=sys.stderr)
    return docs


def of_type(docs: dict, type_: str):
    """All docs of the given type, sorted by name."""
    return [docs[k] for k in sorted(k for k in docs if k[0] == type_)]


def tags_list(tags) -> str:
    if not tags:
        return "&[]"
    return "&[" + ", ".join(rust_str(t) for t in tags) + "]"


def deprecated_of(tags) -> bool:
    return bool(tags) and "Deprecated" in tags


def param_def(p) -> str:
    default = p.get("default") or ""
    default_s = f", default: Some({rust_str(str(default))})" if default else ", default: None"
    return f"ParamDef {{ name: {rust_str(bare_name(p.get('name', '')))}, type_name: {rust_str(p.get('type', ''))}{default_s} }}"


SECURITY_VARIANTS = {
    "": "Security::None",
    "None": "Security::None",
    "PluginSecurity": "Security::PluginSecurity",
    "RobloxScriptSecurity": "Security::RobloxScriptSecurity",
    "RobloxEngineSecurity": "Security::RobloxEngineSecurity",
    "LocalUserSecurity": "Security::LocalUserSecurity",
    "NotAccessibleSecurity": "Security::NotAccessibleSecurity",
}

THREAD_SAFETY_VARIANTS = {
    "": "ThreadSafety::Unsafe",
    "ReadSafe": "ThreadSafety::ReadSafe",
    "Unsafe": "ThreadSafety::Unsafe",
    "Safe": "ThreadSafety::Safe",
}


def security_expr(s: str) -> str:
    """Direct enum variant expression for a security string (valid in statics)."""
    return SECURITY_VARIANTS.get(s, f"Security::Other({rust_str(s)})")


def thread_safety_expr(s: str) -> str:
    """Direct enum variant expression for a thread-safety string (valid in statics)."""
    return THREAD_SAFETY_VARIANTS.get(s, f"ThreadSafety::Unknown({rust_str(s)})")


def parse_security(s) -> str:
    """Parse a `security` field (string for methods, {read, write} dict for properties)."""
    if isinstance(s, str):
        return security_expr(s)
    return "Security::None"


def parse_sec_read(s) -> str:
    if isinstance(s, dict):
        return security_expr(s.get("read") or "")
    return "Security::None"


def parse_sec_write(s) -> str:
    if isinstance(s, dict):
        return security_expr(s.get("write") or "")
    return "Security::None"


def method_def(m, security_field="security", thread_safety_field="thread_safety") -> str:
    params = ", ".join(param_def(p) for p in m.get("parameters", []))
    returns = ", ".join(param_def(p) for p in m.get("returns", []))
    sec = parse_security(m.get(security_field))
    ts = m.get(thread_safety_field) or ""
    tags = tags_list(m.get("tags", []))
    dep = deprecated_of(m.get("tags", []))
    return (
        f"MethodDef {{ name: {rust_str(bare_name(m.get('name', '')))}, "
        f"params: &[{params}], returns: &[{returns}], security: {sec}, "
        f"thread_safety: {thread_safety_expr(ts)}, tags: {tags}, deprecated: {str(dep).lower()} }}"
    )


def event_def(m) -> str:
    params = ", ".join(param_def(p) for p in m.get("parameters", []))
    sec = m.get("security") or ""
    tags = tags_list(m.get("tags", []))
    dep = deprecated_of(m.get("tags", []))
    return (
        f"EventDef {{ name: {rust_str(bare_name(m.get('name', '')))}, "
        f"params: &[{params}], security: {security_expr(sec)}, "
        f"tags: {tags}, deprecated: {str(dep).lower()} }}"
    )


def prop_def(m) -> str:
    name = rust_str(bare_name(m.get("name", "")))
    type_name = rust_str(m.get("type", ""))
    r_sec = parse_sec_read(m.get("security"))
    w_sec = parse_sec_write(m.get("security"))
    ts = m.get("thread_safety") or ""
    tags = tags_list(m.get("tags", []))
    ser = m.get("serialization") or {}
    can_load = bool(ser.get("can_load", False))
    can_save = bool(ser.get("can_save", False))
    cat = m.get("category") or ""
    dep = deprecated_of(m.get("tags", []))
    return (
        f"PropDef {{ name: {name}, type_name: {type_name}, "
        f"read_security: {r_sec}, write_security: {w_sec}, "
        f"thread_safety: {thread_safety_expr(ts)}, tags: {tags}, "
        f"serialization: Serialization {{ can_load: {str(can_load).lower()}, can_save: {str(can_save).lower()} }}, "
        f"category: {rust_str(cat)}, deprecated: {str(dep).lower()} }}"
    )


def dt_prop_def(m) -> str:
    return f"DtPropDef {{ name: {rust_str(bare_name(m.get('name', '')))}, type_name: {rust_str(m.get('type', ''))} }}"


def dt_method_def(m) -> str:
    params = ", ".join(param_def(p) for p in m.get("parameters", []))
    returns = ", ".join(param_def(p) for p in m.get("returns", []))
    tags = tags_list(m.get("tags", []))
    dep = deprecated_of(m.get("tags", []))
    return (
        f"DtMethodDef {{ name: {rust_str(bare_name(m.get('name', '')))}, "
        f"params: &[{params}], returns: &[{returns}], tags: {tags}, deprecated: {str(dep).lower()} }}"
    )


def math_op_def(m) -> str:
    dep = deprecated_of(m.get("tags", []))
    return (
        f"MathOpDef {{ operation: {rust_str(m.get('operation', ''))}, "
        f"type_a: {rust_str(m.get('type_a', ''))}, type_b: {rust_str(m.get('type_b', ''))}, "
        f"return_type: {rust_str(m.get('return_type', ''))}, deprecated: {str(dep).lower()} }}"
    )


def gen_enums(docs, out) -> str:
    enum_docs = of_type(docs, "enum")
    lines = [
        "// Generated by tools/codegen/codegen.py. DO NOT EDIT.",
        "#![allow(clippy::all, non_camel_case_types, non_snake_case)]",
        "",
        "use crate::reflection::{EnumDef, EnumItemDef};",
        "",
        "use std::fmt;",
        "",
    ]
    enum_names = []
    fallback = []

    for doc in enum_docs:
        ename = doc["name"]
        items = doc.get("items", [])
        enum_names.append(ename)

        # Dedupe item names; keep first occurrence.
        seen = set()
        dedup = []
        for it in items:
            nm = it["name"]
            if nm in seen:
                fallback.append((ename, f"duplicate item name {nm}"))
                continue
            seen.add(nm)
            dedup.append(it)

        valid = bool(items) and all(IDENT_RE.match(it["name"]) for it in dedup)
        if not valid:
            fallback.append((ename, "no items or invalid identifiers"))
            continue

        variants = ", ".join(f"{rust_ident(it['name'])} = {it['value']}" for it in dedup)
        lines.append(f"#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]")
        lines.append(f"pub enum {rust_ident(ename)} {{ {variants} }}")
        lines.append("")

        # value() -> i64
        lines.append(f"impl {rust_ident(ename)} {{")
        lines.append(f"    #[inline]")
        lines.append(f"    pub const fn value(self) -> i64 {{ self as i64 }}")
        lines.append(f"    #[inline]")
        lines.append(f"    pub const fn name(self) -> &'static str {{")
        lines.append(f"        match self {{")
        for it in dedup:
            lines.append(f"            Self::{rust_ident(it['name'])} => {rust_str(it['name'])},")
        lines.append(f"        }}")
        lines.append(f"    }}")
        lines.append(f"    #[inline]")
        lines.append(f"    pub fn from_name(name: &str) -> Option<Self> {{")
        lines.append(f"        match name {{")
        for it in dedup:
            lines.append(f"            {rust_str(it['name'])} => Some(Self::{rust_ident(it['name'])}),")
        lines.append(f"            _ => None,")
        lines.append(f"        }}")
        lines.append(f"    }}")
        lines.append(f"}}")
        lines.append("")
        lines.append(f"impl TryFrom<i64> for {rust_ident(ename)} {{")
        lines.append(f"    type Error = ();")
        lines.append(f"    fn try_from(value: i64) -> Result<Self, ()> {{")
        lines.append(f"        match value {{")
        for it in dedup:
            lines.append(f"            v if v == {it['value']} => Ok(Self::{rust_ident(it['name'])}),")
        lines.append(f"            _ => Err(()),")
        lines.append(f"        }}")
        lines.append(f"    }}")
        lines.append(f"}}")
        lines.append("")
        lines.append(f"impl From<{rust_ident(ename)}> for i64 {{")
        lines.append(f"    fn from(v: {rust_ident(ename)}) -> i64 {{ v.value() }}")
        lines.append(f"}}")
        lines.append("")
        lines.append(f"impl fmt::Display for {rust_ident(ename)} {{")
        lines.append(f"    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {{")
        lines.append(f"        f.write_str(self.name())")
        lines.append(f"    }}")
        lines.append(f"}}")
        lines.append("")

    # Registry: every enum, including ones without a valid Rust type.
    lines.append("pub static ALL_ENUMS: &[EnumDef] = &[")
    for doc in enum_docs:
        items = doc.get("items", [])
        entries = []
        seen = set()
        for it in items:
            nm = it["name"]
            if nm in seen:
                continue
            seen.add(nm)
            entries.append(
                f"EnumItemDef {{ name: {rust_str(nm)}, value: {it['value']}, deprecated: {str(deprecated_of(it.get('tags', []))).lower()} }}"
            )
        lines.append(f"    EnumDef {{ name: {rust_str(doc['name'])}, items: &[{', '.join(entries)}] }},")
    lines.append("];")
    lines.append("")
    lines.append("pub fn enum_def(name: &str) -> Option<&'static EnumDef> {")
    lines.append("    ALL_ENUMS.iter().find(|e| e.name == name)")
    lines.append("}")
    lines.append("")

    for ename, reason in fallback:
        print(f"  WARN enum {ename}: no Rust type generated ({reason})")

    write(out, "enums.rs", "\n".join(lines))
    return len(enum_docs)


def gen_classes(docs, out) -> None:
    class_docs = of_type(docs, "class")
    valid_names = {c["name"] for c in class_docs}

    lines = [
        "// Generated by tools/codegen/codegen.py. DO NOT EDIT.",
        "#![allow(clippy::all, non_camel_case_types, non_snake_case)]",
        "",
        "use crate::reflection::{",
        "    ClassDef, EventDef, MethodDef, ParamDef, PropDef, Security, Serialization, ThreadSafety,",
        "};",
        "",
    ]

    chunk_size = 110
    chunks = [class_docs[i : i + chunk_size] for i in range(0, len(class_docs), chunk_size)]

    for ci, chunk in enumerate(chunks):
        chunk_lines = [f"pub static CLASSES_{ci}: &[ClassDef] = &["]
        for doc in chunk:
            props = ", ".join(prop_def(m) for m in doc.get("properties", []))
            methods = ", ".join(method_def(m) for m in doc.get("methods", []))
            events = ", ".join(event_def(m) for m in doc.get("events", []))
            callbacks = ", ".join(method_def(m) for m in doc.get("callbacks", []))
            inh = doc.get("inherits") or []
            inherits = inh if isinstance(inh, str) else (inh[0] if inh else "")
            if inherits and inherits not in valid_names:
                print(f"  WARN class {doc['name']}: inherits unknown parent {inherits}")
            tags = tags_list(doc.get("tags", []))
            chunk_lines.append(
                f"    ClassDef {{ name: {rust_str(doc['name'])}, inherits: {rust_str(inherits)}, "
                f"tags: {tags}, properties: &[{props}], methods: &[{methods}], "
                f"events: &[{events}], callbacks: &[{callbacks}] }},"
            )
        chunk_lines.append("];")
        chunk_lines.append("")
        lines.append("\n".join(chunk_lines))

    lines.append("pub static CLASS_CHUNKS: &[&[ClassDef]] = &[")
    for ci in range(len(chunks)):
        lines.append(f"    CLASSES_{ci},")
    lines.append("];")
    lines.append("")
    lines.append("pub fn class_def(name: &str) -> Option<&'static ClassDef> {")
    lines.append("    CLASS_CHUNKS.iter().flat_map(|c| c.iter()).find(|c| c.name == name)")
    lines.append("}")
    lines.append("")
    lines.append("pub fn all_classes() -> impl Iterator<Item = &'static ClassDef> {")
    lines.append("    CLASS_CHUNKS.iter().flat_map(|c| c.iter())")
    lines.append("}")
    lines.append("")

    write(out, "classes.rs", "\n".join(lines))
    return len(class_docs)


def gen_datatypes(docs, out) -> int:
    dt_docs = of_type(docs, "datatype")

    lines = [
        "// Generated by tools/codegen/codegen.py. DO NOT EDIT.",
        "#![allow(clippy::all, non_camel_case_types, non_snake_case)]",
        "",
        "use crate::reflection::{",
        "    DatatypeDef, DtMethodDef, DtPropDef, MathOpDef, ParamDef,",
        "};",
        "use super::enums::*;",
        "",
    ]

    # ---- concrete value structs ----------------------------------------
    lines.append("// Concrete value types. Field tables are curated in tools/codegen/codegen.py.")
    lines.append("")
    enum_type_names = {name for (type_, name) in docs if type_ == "enum"}
    for name, cfg in FIELD_TABLE.items():
        fields = cfg["fields"]
        field_types = [t for _, t in fields]
        has_string = "String" in field_types
        has_enum = any(t in enum_type_names for t in field_types)
        is_copy = not has_string

        if is_copy:
            lines.append("#[derive(Debug, Clone, Copy, PartialEq)]")
        else:
            lines.append("#[derive(Debug, Clone, PartialEq)]")
        if not has_string and not has_enum:
            lines.append("#[derive(Default)]")
        lines.append(f"pub struct {name} {{")
        for fname, ftype in fields:
            lines.append(f"    pub {fname}: {ftype},")
        lines.append("}")
        lines.append("")
        args = ", ".join(f"{fname}: {ftype}" for fname, ftype in fields)
        body = ", ".join(f"{fname}" for fname, _ in fields)
        const_fn = "const " if is_copy else ""
        lines.append(f"impl {name} {{")
        lines.append(f"    pub {const_fn}fn new({args}) -> Self {{")
        lines.append(f"        Self {{ {body} }}")
        lines.append(f"    }}")
        for cname, cval in cfg.get("consts", {}).items():
            lines.append(f"    pub const {cname}: Self = Self::new{cval};")
        lines.append(f"}}")
        lines.append("")

    # ---- datatype registry ---------------------------------------------
    lines.append("pub static ALL_DATATYPES: &[DatatypeDef] = &[")
    for doc in dt_docs:
        consts = ", ".join(dt_prop_def(m) for m in doc.get("constants", []))
        props = ", ".join(dt_prop_def(m) for m in doc.get("properties", []))
        ctors = ", ".join(dt_method_def(m) for m in doc.get("constructors", []))
        methods = ", ".join(dt_method_def(m) for m in doc.get("methods", []))
        fns = ", ".join(dt_method_def(m) for m in doc.get("functions", []))
        ops = ", ".join(math_op_def(m) for m in doc.get("math_operations", []))
        lines.append(
            f"    DatatypeDef {{ name: {rust_str(doc['name'])}, constants: &[{consts}], "
            f"properties: &[{props}], constructors: &[{ctors}], methods: &[{methods}], "
            f"functions: &[{fns}], math_operations: &[{ops}] }},"
        )
    lines.append("];")
    lines.append("")
    lines.append("pub fn datatype_def(name: &str) -> Option<&'static DatatypeDef> {")
    lines.append("    ALL_DATATYPES.iter().find(|d| d.name == name)")
    lines.append("}")
    lines.append("")

    write(out, "datatypes.rs", "\n".join(lines))
    return len(dt_docs)


def gen_libraries(docs, out) -> int:
    lib_docs = of_type(docs, "library")
    lines = [
        "// Generated by tools/codegen/codegen.py. DO NOT EDIT.",
        "#![allow(clippy::all, non_camel_case_types, non_snake_case)]",
        "",
        "use crate::reflection::{DtPropDef, LibraryDef, MethodDef, ParamDef, Security, ThreadSafety};",
        "",
    ]
    lines.append("pub static ALL_LIBRARIES: &[LibraryDef] = &[")
    for doc in lib_docs:
        props = ", ".join(dt_prop_def(m) for m in doc.get("properties", []))
        fns = ", ".join(method_def(m) for m in doc.get("functions", []))
        lines.append(
            f"    LibraryDef {{ name: {rust_str(doc['name'])}, properties: &[{props}], "
            f"functions: &[{fns}] }},"
        )
    lines.append("];")
    lines.append("")
    lines.append("pub fn library_def(name: &str) -> Option<&'static LibraryDef> {")
    lines.append("    ALL_LIBRARIES.iter().find(|l| l.name == name)")
    lines.append("}")
    lines.append("")
    write(out, "libraries.rs", "\n".join(lines))
    return len(lib_docs)


def gen_globals(docs, out) -> int:
    glob_docs = of_type(docs, "global")
    lines = [
        "// Generated by tools/codegen/codegen.py. DO NOT EDIT.",
        "#![allow(clippy::all, non_camel_case_types, non_snake_case)]",
        "",
        "use crate::reflection::{DtPropDef, GlobalDef, MethodDef, ParamDef, Security, ThreadSafety};",
        "",
    ]
    lines.append("pub static ALL_GLOBALS: &[GlobalDef] = &[")
    for doc in glob_docs:
        props = ", ".join(dt_prop_def(m) for m in doc.get("properties", []))
        fns = ", ".join(method_def(m) for m in doc.get("functions", []))
        lines.append(
            f"    GlobalDef {{ name: {rust_str(doc['name'])}, properties: &[{props}], "
            f"functions: &[{fns}] }},"
        )
    lines.append("];")
    lines.append("")
    lines.append("pub fn global_def(name: &str) -> Option<&'static GlobalDef> {")
    lines.append("    ALL_GLOBALS.iter().find(|g| g.name == name)")
    lines.append("}")
    lines.append("")
    write(out, "globals.rs", "\n".join(lines))
    return len(glob_docs)


def gen_mod(out) -> None:
    mod = (
        "// Generated by tools/codegen/codegen.py. DO NOT EDIT.\n"
        "\n"
        "pub mod classes;\n"
        "pub mod datatypes;\n"
        "pub mod enums;\n"
        "pub mod globals;\n"
        "pub mod libraries;\n"
        "\n"
        "pub use classes::*;\n"
        "pub use datatypes::*;\n"
        "pub use enums::*;\n"
        "pub use globals::*;\n"
        "pub use libraries::*;\n"
    )
    write(out, "mod.rs", mod)


def write(out_dir: str, filename: str, content: str) -> None:
    os.makedirs(out_dir, exist_ok=True)
    path = os.path.join(out_dir, filename)
    existing = None
    if os.path.exists(path):
        with open(path, encoding="utf-8") as f:
            existing = f.read()
    if existing != content:
        with open(path, "w", encoding="utf-8", newline="\n") as f:
            f.write(content)
        print(f"wrote {os.path.relpath(path)} ({len(content.splitlines())} lines)")
    else:
        print(f"unchanged {os.path.relpath(path)}")


def main() -> None:
    here = os.path.dirname(os.path.abspath(__file__))
    repo_root = os.path.abspath(os.path.join(here, "..", ".."))

    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--docs",
        default=os.path.join(repo_root, "Documentation", "creator-docs-main", "content", "en-us", "reference", "engine"),
        help="Path to the engine reference YAML directory",
    )
    parser.add_argument(
        "--out",
        default=os.path.join(repo_root, "crates", "bloxvm", "src", "generated"),
        help="Output directory for generated Rust sources",
    )
    args = parser.parse_args()

    docs = load_docs(args.docs)
    print(f"loaded {len(docs)} YAML definitions")

    counts = {}
    counts["classes"] = gen_classes(docs, args.out)
    counts["datatypes"] = gen_datatypes(docs, args.out)
    counts["enums"] = gen_enums(docs, args.out)
    counts["libraries"] = gen_libraries(docs, args.out)
    counts["globals"] = gen_globals(docs, args.out)
    gen_mod(args.out)

    print("summary: " + ", ".join(f"{k}={v}" for k, v in counts.items()))


if __name__ == "__main__":
    main()
