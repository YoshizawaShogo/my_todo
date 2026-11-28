"use strict";
// 異常を知らせる。ヘッダーのタブの右に出す。
// - アプリ（my_todo.exe）と通信できない：赤い帯。5 秒ごとに確かめ、つながり直したら消して読み直す
// - 要確認の一覧：期限切れ・開始遅れ・次の予定を過ぎた・順番が逆（依存関係）・工数の記録漏れ（今週の過ぎた平日）
//   他の人の作業の遅れは一覧に出さない（自分の作業に影響するときは「順番が逆」で出る）

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
  const groups = { overdue: [], late_start: [], next_overdue: [], broken: [], no_estimate: [], too_heavy: [], overload: [] };
  for (const t of state.tasks) {
    if (t.kind === "external") continue;
    const a = task_alerts(t, today);
    for (const k of ["overdue", "late_start", "next_overdue"]) if (a[k]) groups[k].push(t);
    if (needs_estimate(t)) groups.no_estimate.push(t);
    if (too_heavy(t)) groups.too_heavy.push(t);
  }
  groups.overload = overloaded_periods();
  groups.broken = all_dependencies().filter(({ pre, post }) => dependency_broken(pre, post));
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
  const total = g.overdue.length + g.late_start.length + g.next_overdue.length + g.broken.length +
    g.no_estimate.length + g.too_heavy.length + g.overload.length + alerts.unrecorded.length;
  const btn = $("alert_button");
  btn.hidden = total === 0;
  btn.textContent = `⚠ 要確認 ${total}`;
  btn.title = [
    g.overdue.length && `期限切れ ${g.overdue.length}`,
    g.late_start.length && `開始遅れ ${g.late_start.length}`,
    g.next_overdue.length && `次の予定を過ぎた ${g.next_overdue.length}`,
    g.broken.length && `順番が逆 ${g.broken.length}`,
    g.too_heavy.length && `期間に収まらない ${g.too_heavy.length}`,
    g.overload.length && `詰め込みすぎの期間 ${g.overload.length}`,
    g.no_estimate.length && `見積もりなし ${g.no_estimate.length}`,
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
    section("順番が逆", "先のタスクが終わる前に、後のタスクを始める予定になっている", g.broken.map(({ pre, post }) =>
      task_item(post, `「${pre.title}」が ${short_date(dep_dates(pre).end)} まで`))),
    section("期間に収まらない", "見積もりが「期間の平日 × 1 日の時間」を超えている", g.too_heavy.map((t) => {
      const l = task_load(t);
      return task_item(t, `${fmt_hours(l.est)} を平日 ${l.days} 日で`);
    })),
    section("詰め込みすぎの期間", "その期間に収めるタスクの残りの見積もりが、平日 × 1 日の時間に入らない", g.overload.map((p) => el("li", {},
      el("button", {
        type: "button",
        title: p.tasks.map((t) => t.title).join("\n"),
        onclick: () => {
          toggle_alert_panel(false);
          switch_tab("gantt");
          gantt.start = monday_of(add_days(p.start, -7));
          render_gantt();
        },
      }, el("span", { class: "name" }, `${short_date(p.start)}〜${short_date(p.end)}`),
        el("span", { class: "note" }, `${fmt_hours(Math.round(p.total))} ／ 入るのは ${fmt_hours(p.cap)}（${p.tasks.length} 件）`))))),
    section("見積もりなし", "自分の未完了のタスク（未分類も含む。見積もる癖をつけるため）", g.no_estimate.map((t) => task_item(t,
      t.planned_end ? `〜${short_date(t.planned_end)}` : t.planned_start ? `${short_date(t.planned_start)}〜` : t.is_inbox && t.parent_id === null ? "未分類" : ""))),
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
