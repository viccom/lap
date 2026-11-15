#!/usr/bin/env python3
"""Add the compact `aiSearch.rerank_short` header label to every locale.

The cloud-rerank switch moved into the panel header, where the long
`aiSearch.rerank` label no longer fits; the header uses a short chip label
while the tooltip keeps the full hint. The old long key is removed because
nothing references it anymore.
"""
import json
import os

LOCALES = os.path.join(os.path.dirname(__file__), "..", "src", "locales")

EN = {"rerank_short": "Rerank"}
ZH = {"rerank_short": "重排"}
ZH_TW = {"rerank_short": "重排"}
JA = {"rerank_short": "再ランク"}


def apply_locale(path, pack):
    with open(path, encoding="utf-8") as f:
        data = json.load(f)
    ai = data.setdefault("aiSearch", {})
    ai.pop("rerank", None)
    ai.update(pack)
    with open(path, "w", encoding="utf-8", newline="\n") as f:
        json.dump(data, f, ensure_ascii=False, indent=2)
        f.write("\n")


def main():
    files = sorted(f for f in os.listdir(LOCALES) if f.endswith(".json"))
    assert len(files) == 17, files
    overrides = {
        "zh-CN.json": ZH,
        "zh-TW.json": ZH_TW,
        "ja.json": JA,
    }
    for name in files:
        apply_locale(os.path.join(LOCALES, name), overrides.get(name, EN))
        print("updated", name)


if __name__ == "__main__":
    main()
