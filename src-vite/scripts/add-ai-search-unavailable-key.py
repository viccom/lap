#!/usr/bin/env python3
"""One-shot locale updater for the AI-search semantic-unavailable hint.

Adds `aiSearch.empty_semantic_unavailable` to every locale file. English is
the source of truth; zh-CN / zh-TW / ja carry real translations, the
remaining locales get the English text (the upstream pattern for keys
awaiting translation).
"""
import json
import os

LOCALES = os.path.join(os.path.dirname(__file__), "..", "src", "locales")

EN = {
    "aiSearch": {"empty_semantic_unavailable": "The semantic search could not reach the embedding model. Check the embedding settings and retry."},
}
ZH = {
    "aiSearch": {"empty_semantic_unavailable": "语义搜索无法调用嵌入模型，请检查嵌入设置后重试。"},
}
ZH_TW = {
    "aiSearch": {"empty_semantic_unavailable": "語義搜尋無法呼叫嵌入模型，請檢查嵌入設定後重試。"},
}
JA = {
    "aiSearch": {"empty_semantic_unavailable": "セマンティック検索で埋め込みモデルに接続できませんでした。埋め込み設定を確認して再試行してください。"},
}


def apply_locale(path, pack):
    with open(path, encoding="utf-8") as f:
        data = json.load(f)
    data.setdefault("aiSearch", {}).update(pack["aiSearch"])
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
