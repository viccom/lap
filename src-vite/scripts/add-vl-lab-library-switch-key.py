#!/usr/bin/env python3
"""One-shot locale updater for the lab library-switch reset.

Adds `vl.lab.library_switched` to every locale file. English is the source
of truth; zh-CN / zh-TW / ja carry real translations, the remaining locales
get the English text (the upstream pattern for keys awaiting translation).
"""
import json
import os

LOCALES = os.path.join(os.path.dirname(__file__), "..", "src", "locales")

EN = {
    "vl": {"lab": {"library_switched": "The photo library was switched in the main window. Reload the file list before running understanding."}},
}
ZH = {
    "vl": {"lab": {"library_switched": "主窗口已切换图库，请重新加载文件列表后再发起理解。"}},
}
ZH_TW = {
    "vl": {"lab": {"library_switched": "主視窗已切換圖庫，請重新載入檔案清單後再發起理解。"}},
}
JA = {
    "vl": {"lab": {"library_switched": "メインウィンドウでライブラリが切り替わりました。ファイル一覧を再読み込みしてから理解を実行してください。"}},
}


def apply_locale(path, pack):
    with open(path, encoding="utf-8") as f:
        data = json.load(f)
    data.setdefault("vl", {}).setdefault("lab", {}).update(pack["vl"]["lab"])
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
