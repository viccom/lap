#!/usr/bin/env python3
"""Add the AI-search cloud-rerank keys to every locale.

Keys land under `aiSearch`: the panel switch label + hint, and the two
runtime toasts (failure keeps the local ranking; nothing matched the
descriptions). English is the source of truth; zh-CN / zh-TW / ja carry real
translations.
"""
import json
import os

LOCALES = os.path.join(os.path.dirname(__file__), "..", "src", "locales")

EN = {
    "rerank": "Cloud rerank",
    "rerank_hint": "Off by default. When on, the query and the candidate photo descriptions are sent to the rerank text model (no images, ~1–2 s); failures keep the local ranking. Requires the gateway to be configured.",
    "rerank_failed": "Cloud rerank failed; the local ranking was kept",
    "rerank_no_match": "No photo description matched the query",
}
ZH = {
    "rerank": "云端重排",
    "rerank_hint": "默认关闭。开启后把查询与候选照片的图片描述发给重排文本模型（不发送图片，约 1–2 秒）；失败时保留本地排序。需要网关已配置。",
    "rerank_failed": "云端重排失败，已保留本地排序",
    "rerank_no_match": "没有与描述相符的照片",
}
ZH_TW = {
    "rerank": "雲端重排",
    "rerank_hint": "預設關閉。開啟後把查詢與候選照片的圖片描述送給重排文字模型（不傳送圖片，約 1–2 秒）；失敗時保留本地排序。需要閘道已設定。",
    "rerank_failed": "雲端重排失敗，已保留本地排序",
    "rerank_no_match": "沒有與描述相符的照片",
}
JA = {
    "rerank": "クラウド再ランク",
    "rerank_hint": "既定ではオフ。オンにすると検索語と候補写真の説明を再ランク用テキストモデルへ送ります（画像は送らず、約 1〜2 秒）。失敗時はローカル順位を維持します。ゲートウェイの設定が必要です。",
    "rerank_failed": "クラウド再ランクに失敗しました。ローカル順位を維持します",
    "rerank_no_match": "説明が検索語に合う写真はありません",
}


def apply_locale(path, pack):
    with open(path, encoding="utf-8") as f:
        data = json.load(f)
    data.setdefault("aiSearch", {}).update(pack)
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
