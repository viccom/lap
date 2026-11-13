# Read-only probe: full model list shape from the VL gateway.
# Never prints the API key.
import json, urllib.request, urllib.error, pathlib, collections

APP = pathlib.Path(r"C:\Users\viccom\AppData\Local\com.julyx10.lap.debug")
key = (APP / "vl-api-key").read_text(encoding="utf-8").strip()
cfg = json.loads((APP / "app-config.json").read_text(encoding="utf-8"))
base = ((cfg.get("vl") or {}).get("gateway_url") or "http://192.168.199.8:3425").rstrip("/")


def fetch(path, with_key=True):
    req = urllib.request.Request(base + path, method="GET")
    if with_key:
        req.add_header("x-api-key", key)
        req.add_header("anthropic-version", "2023-06-01")
    try:
        with urllib.request.urlopen(req, timeout=10) as resp:
            return resp.status, resp.read().decode("utf-8", "replace")
    except urllib.error.HTTPError as e:
        return e.code, e.read(500).decode("utf-8", "replace")
    except Exception as e:
        return None, f"{type(e).__name__}: {e}"


print("== key required? ==")
for with_key in (True, False):
    st, body = fetch("/v1/models", with_key)
    print(f"  with_key={with_key}: status={st} len={len(body)}")
print()

st, body = fetch("/v1/models")
data = json.loads(body)
models = data.get("data", [])
print(f"== total models: {len(models)} ==")
print("top-level keys:", sorted(data.keys()))
print("per-model keys:", sorted(models[0].keys()) if models else None)
print()

# field summary
print("== id / display_name / modalities / ctx / max_out ==")
vision, text_only, embeds = [], [], []
for m in models:
    mid = m.get("id", "")
    mods = (m.get("modalities") or {}).get("input") or []
    row = (
        f"{mid:52s} in={','.join(mods):12s} "
        f"ctx={m.get('context_length')} out={m.get('max_output_tokens')} "
        f"reasoning={m.get('reasoning')}"
    )
    if "embed" in mid.lower() or "embedding" in mid.lower():
        embeds.append(mid)
    elif "image" in mods or "vision" in mid.lower():
        vision.append(mid)
    else:
        text_only.append(mid)
    print(" ", row)
print()
print(f"vision-capable: {len(vision)}  text-only: {len(text_only)}  embedding-ish: {len(embeds)}")
print()
print("== rerank / embedding model ids present? ==")
for m in models:
    mid = m.get("id", "")
    if any(k in mid.lower() for k in ("embed", "rerank", "bge")):
        print(f"  {mid}  modalities={(m.get('modalities') or {}).get('input')}  ctx={m.get('context_length')} out={m.get('max_output_tokens')}")
print()
print("== owned_by distribution ==", collections.Counter(m.get("owned_by") for m in models))
