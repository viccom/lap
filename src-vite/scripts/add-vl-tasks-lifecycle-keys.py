#!/usr/bin/env python3
"""Add the VL task lifecycle + schedule keys to every locale.

Covers:
- run lifecycle actions: retry failed, delete one run, clear finished
- scheduled runs: title, empty hint, modes, template picker, add/remove,
  next-fire label, validation error
- the max_concurrency_hint rewording: the setting now caps the number of
  LLM requests in flight (across runs), not just parallel runs.

English is the source of truth; zh-CN / zh-TW / ja carry real translations
and the remaining locales receive the English text, matching the pattern of
the earlier locale batch.
"""
import json
import os

LOCALES = os.path.join(os.path.dirname(__file__), "..", "src", "locales")

EN = {
    "run_retry": "Retry failed",
    "run_delete": "Delete",
    "run_delete_confirm": "Delete this run and its call history?",
    "run_retry_started": "Retry run started",
    "clear_finished": "Clear finished",
    "clear_finished_confirm": "Delete every finished, cancelled, failed or interrupted run and its history?",
    "clear_finished_done": "Cleared {count} runs",
    "schedules_title": "Scheduled runs",
    "schedules_empty": "No scheduled runs",
    "schedule_mode_daily": "Daily",
    "schedule_mode_once": "Once",
    "schedule_template": "Template",
    "schedule_add": "Add schedule",
    "schedule_remove": "Remove",
    "schedule_next_fire": "Next",
    "schedule_invalid": "Fill in the time and template (a date is needed for once)",
    "max_concurrency_hint": "How many understanding requests are sent to the gateway at the same time, across all tasks. Waiting work is queued; 2 is a safe default.",
}
ZH = {
    "run_retry": "重试失败",
    "run_delete": "删除",
    "run_delete_confirm": "删除这条任务及其调用记录？",
    "run_retry_started": "已创建重试任务",
    "clear_finished": "清理已结束",
    "clear_finished_confirm": "删除全部已结束任务（已完成/已取消/失败/已中断）及其记录？",
    "clear_finished_done": "已清理 {count} 条任务",
    "schedules_title": "计划任务",
    "schedules_empty": "没有计划任务",
    "schedule_mode_daily": "每天",
    "schedule_mode_once": "单次",
    "schedule_template": "选择模板",
    "schedule_add": "新建计划",
    "schedule_remove": "删除",
    "schedule_next_fire": "下次",
    "schedule_invalid": "请填写时间与模板（单次还需日期）",
    "max_concurrency_hint": "同时发送到网关的理解请求数上限（对所有任务生效）。超出的部分会排队；2 是稳妥的默认值。",
}
ZH_TW = {
    "run_retry": "重試失敗",
    "run_delete": "刪除",
    "run_delete_confirm": "刪除這條任務及其呼叫記錄？",
    "run_retry_started": "已建立重試任務",
    "clear_finished": "清理已結束",
    "clear_finished_confirm": "刪除全部已結束任務（已完成/已取消/失敗/已中斷）及其記錄？",
    "clear_finished_done": "已清理 {count} 條任務",
    "schedules_title": "計畫任務",
    "schedules_empty": "沒有計畫任務",
    "schedule_mode_daily": "每天",
    "schedule_mode_once": "單次",
    "schedule_template": "選擇範本",
    "schedule_add": "新增計畫",
    "schedule_remove": "刪除",
    "schedule_next_fire": "下次",
    "schedule_invalid": "請填寫時間與範本（單次還需日期）",
    "max_concurrency_hint": "同時傳送到閘道的理解請求數上限（對所有任務生效）。超出的部分會排隊；2 是穩妥的預設值。",
}
JA = {
    "run_retry": "失敗を再試行",
    "run_delete": "削除",
    "run_delete_confirm": "このタスクと呼び出し履歴を削除しますか？",
    "run_retry_started": "再試行タスクを開始しました",
    "clear_finished": "終了分を整理",
    "clear_finished_confirm": "終了したタスク（完了・取消・失敗・中断）と履歴をすべて削除しますか？",
    "clear_finished_done": "{count} 件のタスクを整理しました",
    "schedules_title": "予定タスク",
    "schedules_empty": "予定タスクはありません",
    "schedule_mode_daily": "毎日",
    "schedule_mode_once": "一回のみ",
    "schedule_template": "テンプレート",
    "schedule_add": "予定を追加",
    "schedule_remove": "削除",
    "schedule_next_fire": "次回",
    "schedule_invalid": "時刻とテンプレートを入力してください（一回のみには日付も）",
    "max_concurrency_hint": "ゲートウェイへ同時に送信する理解リクエスト数の上限（全タスク共通）。超過分はキューに入ります。2 が安全な初期値です。",
}


def apply_locale(path, pack):
    with open(path, encoding="utf-8") as f:
        data = json.load(f)
    data.setdefault("settings", {}).setdefault("vl", {}).update(pack)
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
