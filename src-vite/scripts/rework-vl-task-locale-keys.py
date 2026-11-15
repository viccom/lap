#!/usr/bin/env python3
"""Rework the VL scheduled-task keys after the task-queue redesign.

Removes the schedule-shaped keys of the first iteration (schedules_* /
schedule_mode_* / schedule_template / schedule_add / schedule_next_fire) and
adds the task-queue keys plus the manual/scheduled run-source labels and the
browser-dialog wording (vl.run_mode*, vl.run_schedule*, vl.task_queued).
English is the source of truth; zh-CN / zh-TW / ja carry real translations.
"""
import json
import os

LOCALES = os.path.join(os.path.dirname(__file__), "..", "src", "locales")

REMOVE_SETTINGS = [
    "schedules_title",
    "schedules_empty",
    "schedule_mode_daily",
    "schedule_mode_once",
    "schedule_template",
    "schedule_add",
    "schedule_next_fire",
]

EN_SETTINGS = {
    "tasks_queue_title": "Queued tasks",
    "tasks_queue_empty": "No queued tasks — pick photos in the browser and choose a plan time",
    "tasks_queue_hint": "A queued task runs the exact photos it was created with, at its own time. Missed tasks expire at midnight and can be rescheduled.",
    "task_state_pending": "Waiting",
    "task_state_fired": "Fired",
    "task_state_expired": "Expired",
    "task_state_failed": "Failed",
    "task_files": "{count} photos",
    "task_edit_time": "Reschedule",
    "task_save": "Save",
    "run_source_scheduled": "Planned",
    "run_source_manual": "Manual",
}
ZH_SETTINGS = {
    "tasks_queue_title": "计划任务",
    "tasks_queue_empty": "暂无计划任务——在图片浏览界面选择照片并设定计划时间即可",
    "tasks_queue_hint": "计划任务只运行创建时选定的那批照片，到点自动执行；错过当天 24 点过期，可改期重新排队。",
    "task_state_pending": "待执行",
    "task_state_fired": "已执行",
    "task_state_expired": "已过期",
    "task_state_failed": "失败",
    "task_files": "{count} 张照片",
    "task_edit_time": "改时间",
    "task_save": "保存",
    "run_source_scheduled": "计划",
    "run_source_manual": "手动",
}
ZH_TW_SETTINGS = {
    "tasks_queue_title": "計畫任務",
    "tasks_queue_empty": "暫無計畫任務——在圖片瀏覽界面選擇照片並設定計畫時間即可",
    "tasks_queue_hint": "計畫任務只執行建立時選定的那批照片，到點自動執行；錯過當天 24 點過期，可改期重新排隊。",
    "task_state_pending": "待執行",
    "task_state_fired": "已執行",
    "task_state_expired": "已過期",
    "task_state_failed": "失敗",
    "task_files": "{count} 張照片",
    "task_edit_time": "改時間",
    "task_save": "儲存",
    "run_source_scheduled": "計畫",
    "run_source_manual": "手動",
}
JA_SETTINGS = {
    "tasks_queue_title": "予定タスク",
    "tasks_queue_empty": "予定タスクはありません——ブラウザで写真を選び、実行時刻を設定してください",
    "tasks_queue_hint": "予定タスクは作成時に選んだ写真だけを、時刻になると実行します。当日中に実行できなければ期限切れになり、再設定できます。",
    "task_state_pending": "待機中",
    "task_state_fired": "実行済み",
    "task_state_expired": "期限切れ",
    "task_state_failed": "失敗",
    "task_files": "{count} 枚",
    "task_edit_time": "時刻変更",
    "task_save": "保存",
    "run_source_scheduled": "予定",
    "run_source_manual": "手動",
}

EN_VL = {
    "run_mode": "When",
    "run_mode_now": "Run now",
    "run_mode_schedule": "Planned",
    "run_schedule_time": "Runs at",
    "run_schedule": "Schedule",
    "run_schedule_invalid": "Set a valid date and time for the planned run",
    "task_queued": "{count} photos queued for {time}",
}
ZH_VL = {
    "run_mode": "执行时机",
    "run_mode_now": "立即执行",
    "run_mode_schedule": "计划执行",
    "run_schedule_time": "执行时间",
    "run_schedule": "定时开始",
    "run_schedule_invalid": "请设置有效的计划日期与时间",
    "task_queued": "已加入计划：{count} 张照片将于 {time} 执行",
}
ZH_TW_VL = {
    "run_mode": "執行時機",
    "run_mode_now": "立即執行",
    "run_mode_schedule": "計畫執行",
    "run_schedule_time": "執行時間",
    "run_schedule": "定時開始",
    "run_schedule_invalid": "請設定有效的計畫日期與時間",
    "task_queued": "已加入計畫：{count} 張照片將於 {time} 執行",
}
JA_VL = {
    "run_mode": "実行タイミング",
    "run_mode_now": "今すぐ実行",
    "run_mode_schedule": "予定実行",
    "run_schedule_time": "実行時刻",
    "run_schedule": "時刻を設定",
    "run_schedule_invalid": "有効な日付と時刻を設定してください",
    "task_queued": "{count} 枚を {time} の実行に追加しました",
}


def apply_locale(path, settings_pack, vl_pack):
    with open(path, encoding="utf-8") as f:
        data = json.load(f)
    vl = data.setdefault("settings", {}).setdefault("vl", {})
    for key in REMOVE_SETTINGS:
        vl.pop(key, None)
    vl.update(settings_pack)
    data.setdefault("vl", {}).update(vl_pack)
    with open(path, "w", encoding="utf-8", newline="\n") as f:
        json.dump(data, f, ensure_ascii=False, indent=2)
        f.write("\n")


def main():
    files = sorted(f for f in os.listdir(LOCALES) if f.endswith(".json"))
    assert len(files) == 17, files
    overrides = {
        "zh-CN.json": (ZH_SETTINGS, ZH_VL),
        "zh-TW.json": (ZH_TW_SETTINGS, ZH_TW_VL),
        "ja.json": (JA_SETTINGS, JA_VL),
    }
    for name in files:
        settings, vl = overrides.get(name, (EN_SETTINGS, EN_VL))
        apply_locale(os.path.join(LOCALES, name), settings, vl)
        print("updated", name)


if __name__ == "__main__":
    main()
