#!/usr/bin/env python3
"""One-shot locale updater for the image-understanding UX batch.

Adds the new keys of this feature batch to every locale file. English is the
source of truth; zh-CN / zh-TW / ja carry real translations, the remaining
locales get the English text (the upstream pattern for keys awaiting
translation).
"""
import copy
import json
import os

LOCALES = os.path.join(os.path.dirname(__file__), "..", "src", "locales")

EN = {
    "sidebar": {"ai_search": "AI Search"},
    "aiSearch": {
        "title": "AI Search",
        "placeholder": "Describe the photos you are looking for",
        "vl_group": "Image Understanding",
        "vl_annotated": "VL annotated",
        "vl_unannotated": "VL unannotated",
        "empty_no_annotations": "No photos have been understood yet",
        "empty_no_match": "No semantic match among the understood photos",
        "mode": {"all": "All", "precise": "Precise", "semantic": "Semantic"},
        "hint": {
            "all": "Full text index and image descriptions",
            "precise": "BM25 over names, metadata, tags, people and descriptions",
            "semantic": "Matches the stored image descriptions by meaning",
        },
    },
    "settings": {"vl": {
        "protocol": "Gateway protocol",
        "protocol_hint": "Request/response dialect of the gateway",
        "models_title": "Cloud models",
        "embed_title": "Embedding models",
        "refresh_models": "Refresh",
        "models_auth_required": "The model list requires authentication; you can still type a model name manually.",
        "model_unsupported": "The gateway lists this model as text-only",
        "model_unknown": "The gateway does not report whether this model accepts images",
        "model_context": "Context window",
        "runs_title": "Task history",
        "runs_empty": "No image understanding runs yet",
    }},
    "vl": {
        "cancel_run": "Cancel",
        "failed": "failed",
        "skipped": "skipped",
        "run_title": "AI understanding (cloud)",
        "run_count": "{count} photo(s) will be sent to the configured gateway",
        "run_template": "Template",
        "run_no_template": "No enabled template",
        "force_rerun": "Re-understand already annotated photos",
        "run_configure_first": "Configure the gateway or enable a template first",
        "run_start": "Start",
        "run_started": "Understanding {count} photo(s) in the background",
        "run_progress": "Understanding {done}/{total}",
        "run_finished": "Done: {ok} succeeded, {fail} failed, {skip} skipped",
    },
    "menu": {"file": {
        "vl_analyze": "AI understanding (cloud)",
        "vl_analyze_selected": "AI understanding (cloud)",
    }},
    "file_info": {
        "vl_understanding": "Image understanding",
        "vl_loading": "Loading annotations...",
        "vl_empty": "No understanding results for this photo",
        "vl_copy": "Copy description",
        "vl_copied": "Description copied",
    },
}

ZH = {
    "sidebar": {"ai_search": "AI 搜索"},
    "aiSearch": {
        "title": "AI 搜索",
        "placeholder": "描述你想找的照片",
        "vl_group": "图片理解",
        "vl_annotated": "已理解",
        "vl_unannotated": "未理解",
        "empty_no_annotations": "还没有照片被理解过",
        "empty_no_match": "已理解的照片中没有语义匹配",
        "mode": {"all": "全部", "precise": "精确", "semantic": "语义"},
        "hint": {
            "all": "全文索引 + 图片描述",
            "precise": "按文件名、元数据、标签、人名与描述做精确检索",
            "semantic": "按图片描述的语义相似度匹配",
        },
    },
    "settings": {"vl": {
        "protocol": "网关协议",
        "protocol_hint": "网关的请求/响应协议",
        "models_title": "云端模型",
        "embed_title": "嵌入模型",
        "refresh_models": "刷新",
        "models_auth_required": "模型列表需要鉴权；仍可手动输入模型名。",
        "model_unsupported": "网关标注该模型仅支持文本",
        "model_unknown": "网关未说明该模型是否支持图片",
        "model_context": "上下文窗口",
        "runs_title": "任务历史",
        "runs_empty": "还没有图片理解任务",
    }},
    "vl": {
        "cancel_run": "取消",
        "failed": "失败",
        "skipped": "跳过",
        "run_title": "AI 理解（云端）",
        "run_count": "将把 {count} 张照片发送到已配置的网关",
        "run_template": "模板",
        "run_no_template": "没有可用模板",
        "force_rerun": "重新理解已标注的照片",
        "run_configure_first": "请先配置网关或启用模板",
        "run_start": "开始",
        "run_started": "正在后台理解 {count} 张照片",
        "run_progress": "理解进度 {done}/{total}",
        "run_finished": "完成：成功 {ok}，失败 {fail}，跳过 {skip}",
    },
    "menu": {"file": {
        "vl_analyze": "AI 理解（云端）",
        "vl_analyze_selected": "AI 理解（云端）",
    }},
    "file_info": {
        "vl_understanding": "图片理解",
        "vl_loading": "正在载入理解结果…",
        "vl_empty": "这张照片还没有理解结果",
        "vl_copy": "复制描述",
        "vl_copied": "已复制描述",
    },
}

ZH_TW = copy.deepcopy(ZH)
ZH_TW["sidebar"]["ai_search"] = "AI 搜尋"
ZH_TW["aiSearch"]["title"] = "AI 搜尋"
ZH_TW["aiSearch"]["placeholder"] = "描述你想找的照片"
ZH_TW["aiSearch"]["vl_group"] = "圖片理解"
ZH_TW["aiSearch"]["vl_annotated"] = "已理解"
ZH_TW["aiSearch"]["vl_unannotated"] = "未理解"
ZH_TW["aiSearch"]["empty_no_annotations"] = "還沒有照片被理解過"
ZH_TW["aiSearch"]["empty_no_match"] = "已理解的照片中沒有語意匹配"
ZH_TW["aiSearch"]["mode"] = {"all": "全部", "precise": "精確", "semantic": "語意"}
ZH_TW["aiSearch"]["hint"] = {
    "all": "全文索引 + 圖片描述",
    "precise": "按檔名、元資料、標籤、人名與描述做精確檢索",
    "semantic": "按圖片描述的語意相似度匹配",
}
ZH_TW["settings"]["vl"]["protocol"] = "閘道協定"
ZH_TW["settings"]["vl"]["protocol_hint"] = "閘道的請求/回應協定"
ZH_TW["settings"]["vl"]["models_title"] = "雲端模型"
ZH_TW["settings"]["vl"]["embed_title"] = "嵌入模型"
ZH_TW["settings"]["vl"]["refresh_models"] = "重新整理"
ZH_TW["settings"]["vl"]["models_auth_required"] = "模型列表需要鑑權；仍可手動輸入模型名。"
ZH_TW["settings"]["vl"]["model_unsupported"] = "閘道標註該模型僅支援文字"
ZH_TW["settings"]["vl"]["model_unknown"] = "閘道未說明該模型是否支援圖片"
ZH_TW["settings"]["vl"]["model_context"] = "上下文視窗"
ZH_TW["settings"]["vl"]["runs_title"] = "任務歷史"
ZH_TW["settings"]["vl"]["runs_empty"] = "還沒有圖片理解任務"
ZH_TW["vl"]["run_title"] = "AI 理解（雲端）"
ZH_TW["vl"]["run_count"] = "將把 {count} 張照片傳送到已設定的閘道"
ZH_TW["vl"]["run_template"] = "範本"
ZH_TW["vl"]["run_no_template"] = "沒有可用範本"
ZH_TW["vl"]["force_rerun"] = "重新理解已標註的照片"
ZH_TW["vl"]["run_configure_first"] = "請先設定閘道或啟用範本"
ZH_TW["vl"]["run_start"] = "開始"
ZH_TW["vl"]["cancel_run"] = "取消"
ZH_TW["vl"]["failed"] = "失敗"
ZH_TW["vl"]["skipped"] = "跳過"
ZH_TW["vl"]["run_started"] = "正在背景理解 {count} 張照片"
ZH_TW["vl"]["run_progress"] = "理解進度 {done}/{total}"
ZH_TW["vl"]["run_finished"] = "完成：成功 {ok}，失敗 {fail}，跳過 {skip}"
ZH_TW["menu"]["file"]["vl_analyze"] = "AI 理解（雲端）"
ZH_TW["menu"]["file"]["vl_analyze_selected"] = "AI 理解（雲端）"
ZH_TW["file_info"]["vl_understanding"] = "圖片理解"
ZH_TW["file_info"]["vl_loading"] = "正在載入理解結果…"
ZH_TW["file_info"]["vl_empty"] = "這張照片還沒有理解結果"
ZH_TW["file_info"]["vl_copy"] = "複製描述"
ZH_TW["file_info"]["vl_copied"] = "已複製描述"

JA = copy.deepcopy(EN)
JA["sidebar"] = {"ai_search": "AI 検索"}
JA["aiSearch"] = {
    "title": "AI 検索",
    "placeholder": "探したい写真を言葉で説明してください",
    "vl_group": "画像理解",
    "vl_annotated": "理解済み",
    "vl_unannotated": "未理解",
    "empty_no_annotations": "まだ理解された写真はありません",
    "empty_no_match": "理解済みの写真に意味の一致はありません",
    "mode": {"all": "すべて", "precise": "精密", "semantic": "意味"},
    "hint": {
        "all": "全文索引と画像の説明",
        "precise": "ファイル名・メタデータ・タグ・人物・説明を対象に検索",
        "semantic": "画像の説明と意味で照合",
    },
}


def merge(base, override):
    out = copy.deepcopy(base)
    for key, value in override.items():
        if isinstance(value, dict):
            out[key] = merge(out.get(key, {}), value)
        else:
            out[key] = value
    return out


def apply_locale(path, pack):
    with open(path, encoding="utf-8") as f:
        data = json.load(f)
    data.setdefault("settings", {}).setdefault("vl", {}).update(pack["settings"]["vl"])
    data.setdefault("menu", {}).setdefault("file", {}).update(pack["menu"]["file"])
    data.setdefault("sidebar", {})["ai_search"] = pack["sidebar"]["ai_search"]
    data.setdefault("vl", {}).update(pack["vl"])
    data["file_info"] = merge(data.get("file_info", {}), pack["file_info"])
    data["aiSearch"] = merge(data.get("aiSearch", {}), pack["aiSearch"])
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
