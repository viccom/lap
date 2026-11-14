#!/usr/bin/env python3
"""One-shot locale updater for two keys that were referenced but never added.

`tooltip.album.remove` (Montage.vue) and `video.errors.playback_failed`
(Video.vue) resolve to nothing in every locale, so the UI shows the raw key
(and the `|| 'Playback failed'` fallback never runs because vue-i18n returns
the key itself). English is the source of truth; zh-CN / zh-TW / ja carry real
translations, the remaining locales get the English text.
"""
import json
import os

LOCALES = os.path.join(os.path.dirname(__file__), "..", "src", "locales")

EN = {
    "tooltip": {"album": {"remove": "Remove from album"}},
    "video": {"errors": {"playback_failed": "Video playback failed"}},
}
ZH = {
    "tooltip": {"album": {"remove": "从相册移除"}},
    "video": {"errors": {"playback_failed": "视频播放失败"}},
}
ZH_TW = {
    "tooltip": {"album": {"remove": "從相簿移除"}},
    "video": {"errors": {"playback_failed": "影片播放失敗"}},
}
JA = {
    "tooltip": {"album": {"remove": "アルバムから削除"}},
    "video": {"errors": {"playback_failed": "動画の再生に失敗しました"}},
}


def apply_locale(path, pack):
    with open(path, encoding="utf-8") as f:
        data = json.load(f)
    data.setdefault("tooltip", {}).setdefault("album", {}).update(pack["tooltip"]["album"])
    data.setdefault("video", {}).setdefault("errors", {}).update(pack["video"]["errors"])
    with open(path, "w", encoding="utf-8", newline="\n") as f:
        json.dump(data, f, ensure_ascii=False, indent=2)
        f.write("\n")


def main():
    files = sorted(f for f in os.listdir(LOCALES) if f.endswith(".json"))
    assert len(files) == 17, files
    overrides = {"zh-CN.json": ZH, "zh-TW.json": ZH_TW, "ja.json": JA}
    for name in files:
        apply_locale(os.path.join(LOCALES, name), overrides.get(name, EN))
        print("updated", name)


if __name__ == "__main__":
    main()
