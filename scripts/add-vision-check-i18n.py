"""Insert the gateway vision self-check keys into every locale.

Repo convention: at least en + zh-CN carry real text; the other locales may
ship English placeholders until a translator picks them up, but the keys must
exist everywhere so the UI never falls back to a raw key path.
"""
import json
import os

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
LOCALES_DIR = os.path.join(ROOT, "src-vite", "src", "locales")

EN = {
    "vision_check": "Check vision",
    "vision_check_hint": "Sends a red/blue probe image to confirm the selected model really sees pixels (some gateways label text-only models as vision-capable).",
    "vision_check_ok": "Vision works: {answer}",
    "vision_check_mismatch": "Could not confirm vision (answer: {answer}). Consider a vision model such as zhipu/glm-4.6.",
    "vision_check_failed": "Check failed: {error}",
    "vision_check_no_answer": "(empty answer)",
}

ZH_CN = {
    "vision_check": "检测视觉能力",
    "vision_check_hint": "发送一张左红右蓝的测试图，确认所选模型真的能看图（部分网关会把纯文本模型标成支持视觉）。",
    "vision_check_ok": "视觉正常：{answer}",
    "vision_check_mismatch": "未能确认视觉能力（回答：{answer}）。建议换用 zhipu/glm-4.6 之类的视觉模型。",
    "vision_check_failed": "检测失败：{error}",
    "vision_check_no_answer": "（空回答）",
}

ZH_TW = {
    "vision_check": "檢測視覺能力",
    "vision_check_hint": "傳送一張左紅右藍的測試圖，確認所選模型真的能看圖（部分閘道會把純文字模型標成支援視覺）。",
    "vision_check_ok": "視覺正常：{answer}",
    "vision_check_mismatch": "未能確認視覺能力（回答：{answer}）。建議改用 zhipu/glm-4.6 之類的視覺模型。",
    "vision_check_failed": "檢測失敗：{error}",
    "vision_check_no_answer": "（空回答）",
}

for name in sorted(os.listdir(LOCALES_DIR)):
    if not name.endswith(".json"):
        continue
    path = os.path.join(LOCALES_DIR, name)
    with open(path, encoding="utf-8") as fh:
        data = json.load(fh)
    vl = data.setdefault("settings", {}).setdefault("vl", {})
    if name == "zh-CN.json":
        strings = ZH_CN
    elif name == "zh-TW.json":
        strings = ZH_TW
    else:
        strings = EN
    for key, value in strings.items():
        vl[key] = value
    text = json.dumps(data, ensure_ascii=False, indent=2).replace("\n", "\r\n") + "\r\n"
    with open(path, "w", encoding="utf-8", newline="") as fh:
        fh.write(text)
    print(f"{name}: +{len(strings)} keys")
print("done")
