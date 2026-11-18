#!/usr/bin/env python3
"""Generate the lapd command bridge dispatch table (`bridge_gen.rs`).

`gen-command-inventory.py` stays the single source of truth: this script
importlib-loads it and reuses `parse_handler` (the main.rs registration
list), `parse_signatures` (shell wrapper signatures), `classify` (A/B/C)
and `exposure_of` (read/write/admin/internal). What it adds is the
bridge-specific layer:

  * wrapper fidelity scan — async flag, return type, and a pass-through
    check that every bridged command's shell body is exactly one call to
    `lappp_core::<module>::<name>(...)`;
  * parameter parsing — comment-stripped, string-aware top-level comma
    split, `name: Type` extraction, `&str` / `Option<&str>` normalization;
  * the bridge whitelist — class A commands whose exposure is NOT
    "internal" (the curated 不暴露 entries stay desktop-only per §3.6).

Re-run after adding commands:

    python -I docs/dev/gen-lapd-bridge.py

Outputs `src-tauri/crates/lappp-server/src/bridge_gen.rs` (deterministic,
registration order). The Rust-side completeness guard test pins the B/C/
internal skip list by hand, so a newly added command stays red until the
generator is re-run AND the skip list is reviewed.
"""
from __future__ import annotations

import importlib.util
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
INVENTORY = ROOT / "docs" / "dev" / "gen-command-inventory.py"
SRC_DIR = ROOT / "src-tauri" / "src"
OUT = ROOT / "src-tauri" / "crates" / "lappp-server" / "src" / "bridge_gen.rs"

PRIMITIVES = {
    "String", "bool", "char", "f32", "f64", "i8", "i16", "i32", "i64",
    "isize", "u8", "u16", "u32", "u64", "usize",
}

# Non-primitive parameter types that may reach the bridge, mapped to their
# lappp_core paths. Anything else fails the generation loudly — add the type
# here only after checking it derives Deserialize.
TYPE_IMPORTS = {
    "HashMap<String, String>": "std::collections::HashMap",
    "ACollectionOrder": "lappp_core::t_sqlite::ACollectionOrder",
    "AlbumDisplayOrder": "lappp_core::t_sqlite::AlbumDisplayOrder",
    "BatchDeleteFile": "lappp_core::t_cmds::BatchDeleteFile",
    "BatchFileMetadataUpdate": "lappp_core::t_write::BatchFileMetadataUpdate",
    "DedupDeleteRequest": "lappp_core::t_dedup::DedupDeleteRequest",
    "EditParams": "lappp_core::t_image::EditParams",
    "ImageSearchParams": "lappp_core::t_sqlite::ImageSearchParams",
    "KeepRules": "lappp_core::t_dedup::KeepRules",
    "LibraryState": "lappp_core::t_config::LibraryState",
    "MontageParams": "lappp_core::t_montage::MontageParams",
    "PathBuf": "std::path::PathBuf",
    "PersonPageRequest": "lappp_core::t_sqlite::PersonPageRequest",
    "QueryParams": "lappp_core::t_sqlite::QueryParams",
    "RawDisplayOptions": "lappp_core::t_raw_display::RawDisplayOptions",
    "RenameItem": "lappp_core::t_batch_rename::RenameItem",
    "RenameOptions": "lappp_core::t_batch_rename::RenameOptions",
    "RestoreSelection": "lappp_core::t_storage::RestoreSelection",
    "SmartQueryParams": "lappp_core::t_sqlite::SmartQueryParams",
    "ThumbRequest": "lappp_core::t_cmds::ThumbRequest",
}

QUALIFIED_TYPE = re.compile(r"(?:lappp_core|std)::[A-Za-z0-9_:]+|t_[a-z_0-9]+::[A-Za-z0-9_:]+")

RUST_KEYWORDS = {
    "as", "box", "dyn", "fn", "in", "let", "loop", "match", "mod", "move",
    "mut", "ref", "type", "use", "where",
}


def load_inventory():
    spec = importlib.util.spec_from_file_location("gen_command_inventory", INVENTORY)
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


def balance(text: str, start: int, open_ch: str, close_ch: str) -> int:
    """Index of the `close_ch` matching `text[start]` (string/comment aware)."""
    depth = 0
    i = start
    n = len(text)
    while i < n:
        ch = text[i]
        if ch in "\"'":
            quote = ch
            i += 1
            while i < n and text[i] != quote:
                i += 2 if text[i] == "\\" else 1
            i += 1
            continue
        if text.startswith("//", i):
            i = text.index("\n", i) + 1 if "\n" in text[i:] else n
            continue
        if text.startswith("/*", i):
            i = text.index("*/", i) + 2
            continue
        if ch == open_ch:
            depth += 1
        elif ch == close_ch:
            depth -= 1
            if depth == 0:
                return i
        i += 1
    raise ValueError(f"unbalanced {open_ch}{close_ch} at {start}")


def scan_wrappers() -> dict[str, dict]:
    """Shell `#[tauri::command]` wrappers: async flag, return type, body."""
    wrappers: dict[str, dict] = {}
    for path in sorted(SRC_DIR.glob("t_*.rs")):
        text = path.read_text(encoding="utf-8")
        for m in re.finditer(r"#\[tauri::command\]", text):
            tail = text[m.end(): m.end() + 6000]
            fm = re.search(r"\bfn\s+([a-z_0-9]+)\s*\(", tail)
            if not fm:
                continue
            name = fm.group(1)
            is_async = bool(re.search(rf"\basync\s+fn\s+{name}\b", tail))
            open_idx = m.end() + fm.end() - 1
            close_idx = balance(text, open_idx, "(", ")")
            brace_idx = text.index("{", close_idx)
            between = text[close_idx + 1: brace_idx].strip()
            ret = between[2:].strip() if between.startswith("->") else "()"
            body_close = balance(text, brace_idx, "{", "}")
            body = text[brace_idx + 1: body_close]
            wrappers[name] = {
                "async": is_async,
                "ret": ret,
                "body": body,
                "params_text": text[open_idx + 1: close_idx],
                "file": path.name,
            }
    return wrappers


def split_params(params_text: str) -> list[str]:
    """Top-level comma split; `//` line comments stripped first."""
    lines = [re.sub(r"//.*$", "", ln) for ln in params_text.splitlines()]
    text = "\n".join(lines)
    parts: list[str] = []
    cur: list[str] = []
    depth = 0
    i = 0
    n = len(text)
    while i < n:
        ch = text[i]
        if ch in "\"'":
            quote = ch
            cur.append(ch)
            i += 1
            while i < n and text[i] != quote:
                cur.append(text[i])
                i += 2 if text[i] == "\\" else 1
            cur.append(quote)
            i += 1
            continue
        if ch in "(<[":
            depth += 1
        elif ch in ")>]":
            depth -= 1
        elif ch == "," and depth == 0:
            parts.append("".join(cur))
            cur = []
            i += 1
            continue
        cur.append(ch)
        i += 1
    parts.append("".join(cur))
    return [p.strip() for p in parts if p.strip()]


def parse_param(raw: str) -> tuple[str, str]:
    """`name: Type` — the type side may contain `::` paths."""
    name, sep, type_ = raw.partition(":")
    if not sep:
        raise ValueError(f"cannot parse parameter: {raw!r}")
    return name.strip(), " ".join(type_.split())


def camel(name: str) -> str:
    parts = name.split("_")
    return parts[0] + "".join(p[:1].upper() + p[1:] for p in parts[1:])


def rust_local(name: str) -> str:
    return f"r#{name}" if name in RUST_KEYWORDS else name


def unwrap_outer(type_: str, wrapper: str) -> str | None:
    if type_.startswith(f"{wrapper}<") and type_.endswith(">"):
        return type_[len(wrapper) + 1: -1].strip()
    return None


def collect_type_imports(type_: str, imports: set[str], modules: set[str], unknown: set[str]) -> None:
    for wrapper in ("Option", "Vec"):
        inner = unwrap_outer(type_, wrapper)
        if inner is not None:
            collect_type_imports(inner, imports, modules, unknown)
            return
    if type_ == "&str" or type_ in PRIMITIVES:
        return
    if type_.startswith("lappp_core::") or type_.startswith("std::"):
        return  # fully qualified — usable inline
    if QUALIFIED_TYPE.fullmatch(type_):
        # module-relative (`t_sqlite::QueryParams`) — needs the module import
        modules.add(type_.split("::", 1)[0])
        return
    if type_ not in TYPE_IMPORTS:
        unknown.add(type_)
        return
    imports.add(TYPE_IMPORTS[type_])


def render_binding(name: str, type_: str, unknown: set[str]) -> tuple[str, str]:
    """(binding line, call-site expression) for one parameter."""
    local = rust_local(name)
    key = camel(name)
    if type_ == "&str":
        return f'let {local}: String = arg(body, "{key}")?;', f"&{local}"
    if type_ == "Option<&str>":
        return f'let {local}: Option<String> = arg_opt(body, "{key}")?;', f"{local}.as_deref()"
    if unwrap_outer(type_, "Option") is not None:
        collect_type_imports(type_, _imports, _modules, unknown)
        return f'let {local}: {type_} = arg_opt(body, "{key}")?;', local
    collect_type_imports(type_, _imports, _modules, unknown)
    return f'let {local}: {type_} = arg(body, "{key}")?;', local


def main() -> int:
    inv = load_inventory()
    entries = inv.parse_handler()
    signatures = inv.parse_signatures()
    wrappers = scan_wrappers()

    bridge: list[dict] = []
    skipped: list[str] = []
    for entry in entries:
        cls, _ = inv.classify(entry, signatures)
        expo = inv.exposure_of(entry)
        name = entry["name"]
        if cls != "A" or expo == "internal":
            skipped.append(name)
            continue
        params = [parse_param(p) for p in split_params(signatures[name])]
        wrap = wrappers[name]
        call = f"lappp_core::{entry['module']}::{name}("
        if call not in wrap["body"]:
            raise ValueError(
                f"{name}: shell wrapper body is not a pass-through to {call}"
            )
        if wrap["async"] != (".await" in wrap["body"]):
            raise ValueError(f"{name}: async flag and .await disagree in wrapper")
        bridge.append({
            "module": entry["module"],
            "name": name,
            "exposure": expo,
            "params": params,
            "async": wrap["async"],
            "ret": " ".join(wrap["ret"].split()),
        })

    global _imports, _modules
    _imports = set()
    _modules = set()
    unknown: set[str] = set()
    arms: list[str] = []
    for cmd in bridge:
        bindings = []
        args: list[str] = []
        for pname, ptype in cmd["params"]:
            line, expr = render_binding(pname, ptype, unknown)
            bindings.append(line)
            args.append(expr)
        core = f"lappp_core::{cmd['module']}::{cmd['name']}"
        call_args = ", ".join(args)
        is_result = cmd["ret"].startswith("Result<")
        if cmd["async"]:
            if is_result:
                expr = f"ok_res({core}({call_args}).await)"
            else:
                expr = f"ok({core}({call_args}).await)"
        elif is_result:
            expr = f"spawn_sync(move || {core}({call_args})).await"
        else:
            expr = f"spawn_sync_plain(move || {core}({call_args})).await"
        body = "\n".join(bindings) + "\n" + expr if bindings else expr
        arms.append(
            f"        // {cmd['module']}::{cmd['name']} [{cmd['exposure']}]\n"
            f'        "{cmd["name"]}" => {{\n'
            + (body + "\n" if bindings else expr + "\n")
            + "        }"
        )
    if unknown:
        print("unknown non-primitive param types — add to TYPE_IMPORTS:", file=sys.stderr)
        for t in sorted(unknown):
            print(f"  {t}", file=sys.stderr)
        return 1

    total = len(entries)
    names = ",\n    ".join(f'"{c["name"]}"' for c in bridge)
    exposures = ",\n    ".join(f'("{c["name"]}", "{c["exposure"]}")' for c in bridge)
    imports = "\n".join(f"use {path};" for path in sorted(_imports))
    module_imports = "\n".join(f"use lappp_core::{modname};" for modname in sorted(_modules))
    header = f"""\
//! @generated by docs/dev/gen-lapd-bridge.py — DO NOT EDIT BY HAND.
//!
//! Regenerate after adding commands: `python -I docs/dev/gen-lapd-bridge.py`.
//! Whitelist = class A commands with a network exposure level, in main.rs
//! registration order: {len(bridge)} bridged / {total} registered
//! ({len(skipped)} skipped: B desktop-glue, C native-only, A 不暴露).
use crate::bridge::{{
    arg, arg_opt, ok, ok_res, spawn_sync, spawn_sync_plain, BridgeError,
}};
{imports}
{module_imports}

/// The bridge whitelist: command names this server accepts on
/// `POST /api/cmd/{{name}}`. Consumed by the completeness guard test; a
/// future `/api/cmd` listing endpoint will read it at runtime too.
#[allow(dead_code)]
pub const COMMANDS: &[&str] = &[
    {names},
];

/// The §3.6 whitelist axis, aligned with COMMANDS by index: `read`/`write`
/// need any valid token, `admin` needs the admin scope or a loopback source.
#[allow(dead_code)]
pub const EXPOSURES: &[(&str, &str)] = &[
    {exposures},
];

/// Dispatch one bridge call. Argument extraction follows the Tauri IPC
/// contract: top-level keys are matched camelCase-first, snake_case as a
/// fallback; nested payloads deserialize untouched.
pub async fn dispatch(name: &str, body: &serde_json::Value) -> Result<serde_json::Value, BridgeError> {{
    match name {{
{chr(10).join(arms)}
        _ => Err(BridgeError::UnknownCommand(name.to_string())),
    }}
}}
"""
    OUT.write_text(header, encoding="utf-8", newline="\n")

    print(f"bridged: {len(bridge)} / {total} registered, skipped: {len(skipped)}")
    print(f"wrote {OUT}")
    return 0


_imports: set[str] = set()
_modules: set[str] = set()

if __name__ == "__main__":
    sys.exit(main())
