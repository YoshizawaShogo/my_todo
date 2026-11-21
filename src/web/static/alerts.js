"use strict";
// 異常を知らせる。ヘッダーのタブの右に出す。
// - アプリ（my_todo.exe）と通信できない：赤い帯。5 秒ごとに確かめ、つながり直したら消して読み直す
// - 要確認の一覧：期限切れ・開始遅れ・次の予定を過ぎた・工数の記録漏れ（今週の過ぎた平日）

const alerts = {
  offline: false,
  build: null, // この画面を開いたときのアプリの版
  outdated: false, // アプリが新しくなったのに画面が古いまま
  unrecorded: [], // 勤務時間を記録していない日（今週の、今日より前の平日）
  unrecordedKey: null,
  open: false,
};

// ---------- 通信切れ ----------
/// 通信切れの印を付ける・消す。つながり直したときの読み直しは、5 秒ごとの確認からだけ行う
/// （操作の途中の api() から読み直すと、読み直しが二重に走るため）
function set_offline(off, { reload: doReload = true } = {}) {
  if (alerts.offline === off) return;
  alerts.offline = off;
  $("conn_alert").hidden = !off;
  if (!off) {
    toast("アプリと再びつながりました");
    if (doReload) reload();
  }
}

async function check_connection() {
  try {
    const res = await fetch("/api/about", { cache: "no-store", signal: AbortSignal.timeout(3000) });
    set_offline(!res.ok);
    if (res.ok) {
      const about = await res.json();
      // 開いたときと版が違えば、画面が古い（アプリを新しくして起動し直した）
      if (alerts.build === null) alerts.build = about.build;
      else if (about.build !== alerts.build && !alerts.outdated) {
        alerts.outdated = true;
        $("outdated_alert").hidden = false;
      }
    }
  } catch {
    set_offline(true);
  }
}

// ---------- 要確認の一覧 ----------
function collect_alerts() {
  const today = today_text();
  const groups = { overdue: [], late_start: [], next_overdue: [] };
  for (const t of state.tasks) {
    const a = task_alerts(t, today);
    for (const k of Object.keys(groups)) if (a[k]) groups[k].push(t);
  }
  return groups;
}

async function refresh_unrecorded() {
  const today = today_text();
  const mon = monday_of(today);
  const yesterday = add_days(today, -1);
  const key = `${today}@${reloadCount}`;
  if (alerts.unrecordedKey === key) return;
  alerts.unrecordedKey = key;
  if (yesterday < mon) {
    alerts.unrecorded = [];
    render_alerts();
    return;
  }
  const days = await api("GET", `/work_days?from=${mon}&to=${yesterday}`);
  const recorded = new Set(days.map((d) => d.date));
  const missing = [];
  for (let d = mon; d <= yesterday; d = add_days(d, 1)) {
    const wd = weekday_of(d);
    if (wd !== 0 && wd !== 6 && !recorded.has(d)) missing.push(d);
  }
  alerts.unrecorded = missing;
  render_alerts();
}

function render_alerts() {
  const g = collect_alerts();
  const total = g.overdue.length + g.late_start.length + g.next_overdue.length + alerts.unrecorded.length;
  const btn = $("alert_button");
  btn.hidden = total === 0;
  btn.textContent = `⚠ 要確認 ${total}`;
  btn.title = [
    g.overdue.length && `期限切れ ${g.overdue.length}`,
    g.late_start.length && `開始遅れ ${g.late_start.length}`,
    g.next_overdue.length && `次の予定を過ぎた ${g.next_overdue.length}`,
    alerts.unrecorded.length && `工数の記録漏れ ${alerts.unrecorded.length} 日`,
  ].filter(Boolean).join("\n");
  if (total === 0) toggle_alert_panel(false);
  if (alerts.open) draw_alert_panel(g);
}

function toggle_alert_panel(open = !alerts.open) {
  alerts.open = open;
  $("alert_panel").hidden = !open;
  if (open) draw_alert_panel(collect_alerts());
}

function draw_alert_panel(g) {
  const section = (title, hint, items) => items.length
    ? el("div", { class: "alert_group" },
      el("div", { class: "alert_title" }, `${title}（${items.length}）`, el("span", { class: "alert_hint" }, hint)),
      el("ul", {}, ...items))
    : null;
  const task_item = (t, note) => el("li", {},
    el("button", {
      type: "button",
      onclick: () => {
        toggle_alert_panel(false);
        select(t.id);
      },
    }, el("span", { class: "name" }, t.title), el("span", { class: "note" }, note)));
  const parts = [
    section("期限切れ", "終了予定日を過ぎた未完了", g.overdue.map((t) => task_item(t, `〜${short_date(t.planned_end)}`))),
    section("開始遅れ", "開始予定日を過ぎても未着手", g.late_start.map((t) => task_item(t, `${short_date(t.planned_start)} 開始予定`))),
    section("次の予定を過ぎた", "", g.next_overdue.map((t) => task_item(t, `▶ ${short_date(t.next_action_date)} ${t.next_action || ""}`))),
    section("工数の記録漏れ", "今週の過ぎた平日で勤務時間が未記録", alerts.unrecorded.map((d) => el("li", {},
      el("button", {
        type: "button",
        onclick: () => {
          toggle_alert_panel(false);
          switch_tab("work");
          set_work_date(d);
        },
      }, el("span", { class: "name" }, short_date(d)), el("span", { class: "note" }, "工数記録を開く"))))),
  ].filter(Boolean);
  $("alert_panel").replaceChildren(...parts);
}

function setup_alerts() {
  $("alert_button").addEventListener("click", (ev) => {
    ev.stopPropagation();
    toggle_alert_panel();
  });
  $("alert_panel").addEventListener("click", (ev) => ev.stopPropagation());
  document.addEventListener("click", () => { if (alerts.open) toggle_alert_panel(false); });
  $("outdated_alert").addEventListener("click", () => location.reload());
  check_connection();
  setInterval(check_connection, 5000);
}
