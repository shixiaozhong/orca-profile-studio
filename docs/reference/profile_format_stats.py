"""Simulate Orca's system-profile loading rules over resources/profiles and print statistics.

Reference implementation for docs/profile_format_spec.md.
Usage: python3 -I docs/profile_format_stats.py resources/profiles
"""
import collections
import json
import os
import sys

ROOT = sys.argv[1]
LIB = "OrcaFilamentLibrary"
SECTIONS = [("machine_model_list", "machine_model"), ("process_list", "process"),
            ("filament_list", "filament"), ("machine_list", "machine")]
# Keys that ConfigBase::load_from_json() separates from config options
META = {"version", "is_custom_defined", "name", "url", "type", "setting_id", "filament_id",
        "from", "description", "instantiation", "inherits", "renamed_from"}


def load(p):
    # nlohmann::json skips a UTF-8 BOM, so the loader must tolerate it too
    with open(p, encoding="utf-8-sig") as f:
        return json.load(f)


def lib_manifest(vdir):
    """Mirror append_json_subfiles(): OrcaFilamentLibrary ships an empty manifest."""
    fdir = os.path.join(vdir, "filament")

    def prio(stem):
        if stem == "fdm_filament_common":
            return 0
        if stem.endswith("@base"):
            return 1
        if stem.endswith("_common"):
            return 2
        return 3

    def collect(d, rec):
        out = []
        if not os.path.isdir(d):
            return out
        if rec:
            for r, _, fs in os.walk(d):
                out += [os.path.join(r, f) for f in fs if f.endswith(".json")]
        else:
            out = [os.path.join(d, f) for f in os.listdir(d)
                   if f.endswith(".json") and os.path.isfile(os.path.join(d, f))]
        out.sort(key=lambda p: (prio(os.path.splitext(os.path.basename(p))[0]), p.replace(os.sep, "/")))
        return [(os.path.splitext(os.path.basename(p))[0], os.path.relpath(p, vdir)) for p in out]

    items = collect(os.path.join(fdir, "base"), True) + collect(fdir, False)
    for sub in sorted(os.listdir(fdir)):
        if sub != "base" and os.path.isdir(os.path.join(fdir, sub)):
            items += collect(os.path.join(fdir, sub), True)
    return items


vendors = sorted(f[:-5] for f in os.listdir(ROOT) if f.endswith(".json") and f != "blacklist.json")
vendors.remove(LIB)
vendors.insert(0, LIB)

st = collections.Counter()
issues = collections.defaultdict(list)
depths = collections.Counter()
keys_by_type = collections.defaultdict(set)
value_shapes = collections.Counter()
from_values = collections.Counter()
model_keys = collections.Counter()
lib_maps = {}               # library filament name -> depth (the only cross-vendor inherit source)
all_printers = set()
compat_refs = []            # (vendor, preset, printer)
global_names = collections.defaultdict(list)
listed_paths = set()

for v in vendors:
    vdir = os.path.join(ROOT, v)
    man = load(os.path.join(ROOT, v + ".json"))
    models = {}
    for sec, typ in SECTIONS:
        items = [(e["name"], e["sub_path"]) for e in man.get(sec, [])]
        if v == LIB and sec == "filament_list" and not items:
            items = lib_manifest(vdir)
        files = {}
        for name, sub in items:
            p = os.path.join(vdir, sub)
            listed_paths.add(os.path.normpath(p))
            if os.path.isfile(p):
                files[sub] = load(p)
            else:
                issues["listed_but_missing"].append(f"{v}/{sub}")
        pending = {d.get("name") for d in files.values()}
        local = {}          # per-vendor, per-section map: preset name (from file) -> depth
        resolved_pm = {}    # machine name -> resolved printer_model / printer_variant
        for name, sub in items:
            d = files.get(sub)
            if d is None:
                continue
            st[f"files_{typ}"] += 1
            if typ == "machine_model":
                model_keys.update(d.keys())
                models[name] = d
                continue
            if d.get("name") != name:
                issues["list_name_vs_file_name"].append(f"{v}/{sub}: list={name} file={d.get('name')}")
            if d.get("type") not in (None, typ):
                issues["type_mismatch"].append(f"{v}/{sub}: section={typ} type={d.get('type')}")
            from_values[d.get("from", "<missing>")] += 1
            inst = d.get("instantiation")
            if inst not in ("true", "false"):
                issues["bad_instantiation"].append(f"{v}/{sub}: {inst!r}")
            for k, val in d.items():
                if k in META:
                    continue
                keys_by_type[typ].add(k)
                if isinstance(val, str):
                    value_shapes["string"] += 1
                elif isinstance(val, list):
                    value_shapes["array_len1" if len(val) == 1 else "array_lenN"] += 1
                    if any(not isinstance(x, str) for x in val):
                        value_shapes["array_non_string_item"] += 1
                else:
                    value_shapes["non_string:" + type(val).__name__] += 1
            par = d.get("inherits", "")
            if par:
                if par in local:
                    depth = local[par] + 1
                elif par in lib_maps:
                    depth = lib_maps[par] + 1
                    if v != LIB:
                        st["inherits_from_library"] += 1
                else:
                    kind = "inherits_parent_after_child" if par in pending else "inherits_not_found"
                    issues[kind].append(f"{v}/{sub}: inherits {par!r}")
                    continue
            else:
                depth = 0
            local[d.get("name")] = depth
            if typ == "filament" and v == LIB:
                lib_maps[d.get("name")] = depth
            # printer_model / printer_variant are usually inherited, so resolve them along the chain
            if typ == "machine":
                inherited = resolved_pm.get(par, {}) if par else {}
                resolved_pm[d.get("name")] = {k: d.get(k, inherited.get(k)) for k in ("printer_model", "printer_variant")}
            st[f"{typ}_inst_{inst}"] += 1
            if inst != "true":
                continue
            depths[depth] += 1
            global_names[(typ, d.get("name"))].append(v)
            if typ == "machine":
                all_printers.add(d.get("name"))
                pm, pv = resolved_pm[d.get("name")]["printer_model"], resolved_pm[d.get("name")]["printer_variant"]
                if not pm or not pv:
                    issues["printer_model_or_variant_empty"].append(f"{v}/{sub}")
                elif pm not in models:
                    issues["printer_model_unknown"].append(f"{v}/{sub}: {pm}")
                elif pv not in models[pm].get("nozzle_diameter", "").split(";"):
                    issues["printer_variant_unknown"].append(f"{v}/{sub}: {pm} / {pv}")
            for pr in d.get("compatible_printers", []) or []:
                compat_refs.append((v, d.get("name"), pr))
            if d.get("compatible_printers_condition"):
                st[f"{typ}_with_condition"] += 1

bom = []
for v in vendors:
    for r, _, fs in os.walk(os.path.join(ROOT, v)):
        for f in fs:
            p = os.path.normpath(os.path.join(r, f))
            if not f.endswith(".json"):
                continue
            with open(p, "rb") as fh:
                if fh.read(3) == b"\xef\xbb\xbf":
                    bom.append(os.path.relpath(p, ROOT))
            if p not in listed_paths:
                issues["orphan_json_not_in_manifest"].append(os.path.relpath(p, ROOT))

dangling = [(v, n, p) for v, n, p in compat_refs if p not in all_printers]
dups = {k: vs for k, vs in global_names.items() if len(vs) > 1}

print("vendors", len(vendors))
print(dict(st))
print("from values", dict(from_values))
print("inheritance depth of instantiable presets", dict(sorted(depths.items())))
print("value shapes", dict(value_shapes))
print("distinct keys per type", {k: len(s) for k, s in keys_by_type.items()})
print("machine_model keys", dict(model_keys))
print("BOM files", bom)
print("compatible_printers refs", len(compat_refs), "dangling", len(dangling))
print("  dangling by vendor", collections.Counter(v for v, _, _ in dangling).most_common(10))
print("cross-vendor duplicate names", len(dups))
for (typ, name), vs in dups.items():
    print(f"  {typ} | {name} | {vs}")
print("orphans by vendor",
      collections.Counter(x.split(os.sep)[0] for x in issues["orphan_json_not_in_manifest"]).most_common(10))
for k, lst in sorted(issues.items()):
    print(f"{k}: {len(lst)}")
    for x in lst[:6]:
        print("   ", x)
