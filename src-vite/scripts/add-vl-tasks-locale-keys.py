#!/usr/bin/env python3
"""Add the missing VL keys of the tasks-monitor batch to every locale.

Covers:
- the two embedding-source options that were referenced but never added
  (settings.vl.embed_source_local / embed_source_cloud)
- the new collapsible model list, rerank picker, max-concurrency and
  VL tasks page keys.

English is the source of truth; zh-CN / zh-TW / ja carry real translations
and the remaining locales receive the English text, matching the pattern of
the earlier locale batch.
"""
import copy
import json
import os

LOCALES = os.path.join(os.path.dirname(__file__), "..", "src", "locales")

EN = {
    "embed_source_local": "Local (BGE model)",
    "embed_source_cloud": "Cloud (via the gateway)",
    "max_concurrency": "Concurrent tasks",
    "max_concurrency_hint": "How many understanding tasks run at the same time. Waiting tasks are queued. Raise it only when the provider tolerates parallel requests; 2 is a safe default.",
    "models_expand": "Show {count} models",
    "models_collapse": "Hide models",
    "rerank_model_pick_hint": "Pick a text model from the refreshed list, or keep typing a name.",
    "tasks_tab": "VL tasks",
    "tasks_refresh": "Refresh",
    "run_state_queued": "Queued",
    "run_state_running": "Running",
    "run_state_finished": "Finished",
    "run_state_cancelled": "Cancelled",
    "run_state_failed": "Failed",
    "run_state_interrupted": "Interrupted",
    "run_state_unknown": "Unknown",
    "run_detail_show": "Details",
    "run_detail_hide": "Hide details",
    "run_calls_empty": "No calls recorded for this task (skipped photos produce no record).",
    "run_calls_more": "Show {count} more",
    "run_call_success": "Succeeded",
    "run_call_failed": "Failed",
    "run_call_file": "Photo",
    "run_call_model": "Model",
    "run_call_duration": "Duration",
    "run_call_tokens": "Tokens",
    "run_call_response": "Raw response",
    "run_call_error": "Error",
    "run_started_at": "Started",
    "run_finished_at": "Finished",
    "run_progress_label": "Progress",
}

ZH = {
    "embed_source_local": "本地（BGE 模型）",
    "embed_source_cloud": "云端（走网关）",
    "max_concurrency": "并发任务数",
    "max_concurrency_hint": "同时执行的理解任务数，超出的任务排队等待。仅在服务商能承受并发时调大；默认 2 比较稳妥。",
    "models_expand": "展开 {count} 个模型",
    "models_collapse": "收起模型",
    "rerank_model_pick_hint": "可点刷新后的列表选择文本模型，也可继续手动输入。",
    "tasks_tab": "VL 任务",
    "tasks_refresh": "刷新",
    "run_state_queued": "排队中",
    "run_state_running": "执行中",
    "run_state_finished": "已完成",
    "run_state_cancelled": "已取消",
    "run_state_failed": "失败",
    "run_state_interrupted": "已中断",
    "run_state_unknown": "未知",
    "run_detail_show": "查看详细",
    "run_detail_hide": "收起详细",
    "run_calls_empty": "该任务没有调用记录（被跳过的照片不产生记录）。",
    "run_calls_more": "再显示 {count} 条",
    "run_call_success": "成功",
    "run_call_failed": "失败",
    "run_call_file": "照片",
    "run_call_model": "模型",
    "run_call_duration": "耗时",
    "run_call_tokens": "Token",
    "run_call_response": "原始返回",
    "run_call_error": "错误",
    "run_started_at": "开始于",
    "run_finished_at": "结束于",
    "run_progress_label": "进度",
}

ZH_TW = {
    "embed_source_local": "本機（BGE 模型）",
    "embed_source_cloud": "雲端（走閘道）",
    "max_concurrency": "並行任務數",
    "max_concurrency_hint": "同時執行的理解任務數，超出者排隊等待。僅在服務商能承受並行時調大；預設 2 較為穩妥。",
    "models_expand": "展開 {count} 個模型",
    "models_collapse": "收合模型",
    "rerank_model_pick_hint": "可從重新整理後的清單挑選文字模型，也可繼續手動輸入。",
    "tasks_tab": "VL 任務",
    "tasks_refresh": "重新整理",
    "run_state_queued": "排隊中",
    "run_state_running": "執行中",
    "run_state_finished": "已完成",
    "run_state_cancelled": "已取消",
    "run_state_failed": "失敗",
    "run_state_interrupted": "已中斷",
    "run_state_unknown": "未知",
    "run_detail_show": "查看詳細",
    "run_detail_hide": "收合詳細",
    "run_calls_empty": "此任務沒有呼叫記錄（被跳過的照片不會產生記錄）。",
    "run_calls_more": "再顯示 {count} 筆",
    "run_call_success": "成功",
    "run_call_failed": "失敗",
    "run_call_file": "照片",
    "run_call_model": "模型",
    "run_call_duration": "耗時",
    "run_call_tokens": "Token",
    "run_call_response": "原始回應",
    "run_call_error": "錯誤",
    "run_started_at": "開始於",
    "run_finished_at": "結束於",
    "run_progress_label": "進度",
}

JA = {
    "embed_source_local": "ローカル（BGE モデル）",
    "embed_source_cloud": "クラウド（ゲートウェイ経由）",
    "max_concurrency": "同時実行タスク数",
    "max_concurrency_hint": "同時に実行する理解タスク数。超えた分は待機します。プロバイダーが並列を許容する場合のみ増やしてください。既定の 2 が安全です。",
    "models_expand": "{count} 件のモデルを表示",
    "models_collapse": "モデルを隠す",
    "rerank_model_pick_hint": "更新後の一覧からテキストモデルを選ぶか、名前を直接入力できます。",
    "tasks_tab": "VL タスク",
    "tasks_refresh": "更新",
    "run_state_queued": "待機中",
    "run_state_running": "実行中",
    "run_state_finished": "完了",
    "run_state_cancelled": "キャンセル済み",
    "run_state_failed": "失敗",
    "run_state_interrupted": "中断",
    "run_state_unknown": "不明",
    "run_detail_show": "詳細",
    "run_detail_hide": "詳細を閉じる",
    "run_calls_empty": "このタスクの呼び出し記録はありません（スキップした写真は記録されません）。",
    "run_calls_more": "さらに {count} 件表示",
    "run_call_success": "成功",
    "run_call_failed": "失敗",
    "run_call_file": "写真",
    "run_call_model": "モデル",
    "run_call_duration": "所要時間",
    "run_call_tokens": "トークン",
    "run_call_response": "生の応答",
    "run_call_error": "エラー",
    "run_started_at": "開始",
    "run_finished_at": "終了",
    "run_progress_label": "進捗",
}

OVERRIDES = {
    "zh-CN.json": ZH,
    "zh-TW.json": ZH_TW,
    "ja.json": JA,
}


def main():
    files = sorted(f for f in os.listdir(LOCALES) if f.endswith(".json"))
    assert len(files) == 17, files
    for name in files:
        path = os.path.join(LOCALES, name)
        with open(path, encoding="utf-8") as handle:
            data = json.load(handle)
        pack = OVERRIDES.get(name, EN)
        section = data.setdefault("settings", {}).setdefault("vl", {})
        for key, value in pack.items():
            section[key] = value
        with open(path, "w", encoding="utf-8", newline="\n") as handle:
            json.dump(data, handle, ensure_ascii=False, indent=2)
            handle.write("\n")
        print("updated", name)


if __name__ == "__main__":
    main()
