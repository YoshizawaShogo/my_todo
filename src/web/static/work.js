"use strict";
// 工数記録タブ。1 日の勤務時間をタスクごとの割合で 0.5 時間刻みに振り分ける。

const WORK_STEP = 30; // 分

const work = {
  date: today_text(),
  day: null, // サーバーから取った WorkDay
  week: [], // 週の DaySummary
  rows: [], // [{task_id, weight, reason}]
  worked: null, // 分（未記録なら初期値）
  loadedKey: null,
  saveTimer: null,
  dirty: false, // 画面で変えたがまだ保存していない
  saving: false, // 保存の通信中
  pending: false, // 保存中にさらに変更があった
};

const REASON_LABEL = {
  recorded: "記録済み",
  doing: "作業中",
  completed: "この日に完了",
  started: "この日に着手",
  next_action: "次の予定",
  planned: "予定期間",
  changed: "この日に変更",
  added: "追加",
};

// ---------- 換算と表示 ----------
function minutes_per_day() {
  return state.settings?.minutes_per_day || 480;
}

/// 分 → "4.5h"
function fmt_hours(min) {
  return `${Math.round((min / 60) * 100) / 100}h`;
}

/// 分 → "4.5h（0.56D）"
function fmt_effort(min) {
  const d = Math.round((min / minutes_per_day()) * 100) / 100;
  return `${fmt_hours(min)}（${d}D）`;
}

/// "1d" "1.5D" "4h" "30m" "2"（数字だけは時間）→ 分。空なら null、読めなければ NaN
function parse_effort(text) {
  const s = text.trim().replace(/\s+/g, "");
  if (!s) return null;
  const m = s.match(/^(\d+(?:\.\d+)?)(d|日|h|時間|m|分)?$/i);
  if (!m) return NaN;
  const n = Number(m[1]);
  const unit = (m[2] || "h").toLowerCase();
  if (unit === "d" || unit === "日") return Math.round(n * minutes_per_day());
  if (unit === "m" || unit === "分") return Math.round(n);
  return Math.round(n * 60);
}

/// サーバーと同じ振り分け（最大剰余方式・30 分刻み）。プレビュー用
function allocate_preview(worked, weights) {
  const total = weights.reduce((a, b) => a + b, 0);
  if (total <= 0 || worked <= 0) return weights.map(() => 0);
  const units = Math.floor(worked / WORK_STEP);
  const raw = weights.map((w) => (w / total) * units);
  const out = raw.map(Math.floor);
  let rest = units - out.reduce((a, b) => a + b, 0);
  const order = weights.map((_, i) => i).sort((a, b) =>
    (raw[b] - Math.floor(raw[b])) - (raw[a] - Math.floor(raw[a])) || weights[b] - weights[a] || a - b);
  for (const i of order) {
    if (rest === 0) break;
    if (weights[i] > 0) { out[i]++; rest--; }
  }
  return out.map((u) => u * WORK_STEP);
}

/// 平日は標準の勤務時間、土日は 0 を初期値にする
function default_worked(date) {
  const wd = weekday_of(date);
  return wd === 0 || wd === 6 ? 0 : (state.settings?.default_work_min ?? 480);
}

// ---------- 読み込み ----------
async function load_work_day(force = false) {
  // 未保存・保存中は、画面の内容をサーバーの古い内容で上書きしない
  if (work.dirty || work.saving) return;
  const key = `${work.date}@${reloadCount}`;
  if (!force && work.loadedKey === key) return;
  work.loadedKey = key;
  const mon = monday_of(work.date);
  const [day, week] = await Promise.all([
    api("GET", `/work_days/${work.date}`),
    api("GET", `/work_days?from=${mon}&to=${add_days(mon, 6)}`),
  ]);
  if (work.loadedKey !== key || work.dirty || work.saving) return; // 取得中に日付が変わった・変更された
  work.day = day;
  work.week = week;
  work.worked = day.worked_min ?? default_worked(work.date);
  // 記録済みの割合 → その日に関わったタスク（割合 0）の順に並べる
  const rows = day.entries.map((e) => ({ task_id: e.task_id, weight: e.weight, reason: "recorded" }));
  for (const s of day.suggestions) {
    if (!rows.some((r) => r.task_id === s.task_id) && state.byId.has(s.task_id)) {
      rows.push({ task_id: s.task_id, weight: 0, reason: s.reason });
    }
  }
  // 手で足した行（まだ割合 0）は日付が同じなら残す
  for (const r of work.rows) {
    if (r.reason === "added" && r.date === work.date && !rows.some((x) => x.task_id === r.task_id)) rows.push(r);
  }
  // 同じ日を読み直したときは、並び順を変えない（割合を付けた行が飛ばないように）
  if (work.rowsDate === work.date) {
    const prev = new Map(work.rows.map((r, i) => [r.task_id, i]));
    rows.sort((a, b) => (prev.get(a.task_id) ?? 1e9) - (prev.get(b.task_id) ?? 1e9));
  }
  work.rows = rows.filter((r) => state.byId.has(r.task_id));
  work.rowsDate = work.date;
  draw_work();
}

function render_work() {
  if (!state.settings) return;
  load_work_day();
  draw_work();
}

// ---------- 描画 ----------
function draw_work() {
  if (!work.day || work.day.date !== work.date) return;
  $("work_date").value = work.date;
  draw_week();

  const worked = work.worked;
  const input = $("work_worked");
  if (document.activeElement !== input) input.value = String(worked / 60);
  const over = worked - (state.settings?.default_work_min ?? 480);
  $("work_overtime").textContent = worked > 0 && over > 0 && default_worked(work.date) > 0 ? `残業 +${fmt_hours(over)}` : "";
  $("work_overtime").hidden = !$("work_overtime").textContent;
  $("work_state").textContent = work.day.worked_min === null ? "未記録（初期値を表示中）" : `合計 ${fmt_effort(worked)}`;

  const weights = work.rows.map((r) => r.weight);
  const minutes = allocate_preview(worked, weights);
  const total = weights.reduce((a, b) => a + b, 0);
  const box = $("work_rows");
  if (!work.rows.length) {
    box.replaceChildren(el("p", { class: "empty_hint" }, "この日に関わったタスクはありません。下の欄からタスクを追加してください。"));
  } else {
    box.replaceChildren(...work.rows.map((r, i) => work_row(r, minutes[i], total)));
  }
  if (worked > 0 && total === 0 && work.rows.length) {
    box.append(el("p", { class: "empty_hint warn" }, "割合がすべて 0 です。スライダーで振り分けてください。"));
  }
  // 消したタスクの分などで、記録の合計が勤務時間に足りないとき
  if (work.day.worked_min !== null && work.day.allocated_min !== work.day.worked_min && total > 0 && !work.saveTimer) {
    box.append(el("p", { class: "empty_hint warn" }, `振り分け済み ${fmt_hours(work.day.allocated_min)} が勤務時間と合っていません。割合を動かすと計算し直します。`));
  }

  // 追加の候補（未完了を先に）
  const opts = [...state.tasks]
    .filter((t) => !work.rows.some((r) => r.task_id === t.id))
    .sort((a, b) => (a.status === "done") - (b.status === "done"))
    .map((t) => el("option", { value: task_option_label(t) }));
  $("work_task_options").replaceChildren(...opts);

  const sd = $("setting_day_hours"), sw = $("setting_default_hours");
  if (document.activeElement !== sd) sd.value = String(minutes_per_day() / 60);
  if (document.activeElement !== sw) sw.value = String((state.settings?.default_work_min ?? 480) / 60);
}

function task_option_label(t) {
  return [...path_titles(t), t.title].join(" › ") + (t.status === "done" ? "（完了）" : "");
}

function work_row(r, minutes, total) {
  const t = state.byId.get(r.task_id);
  const pct = total > 0 ? Math.round((r.weight / total) * 100) : 0;
  const slider = el("input", { type: "range", min: "0", max: "10", step: "1", class: "work_slider" });
  slider.value = String(Math.min(10, r.weight));
  const num = el("input", { type: "number", min: "0", step: "1", class: "work_weight", title: "割合（スライダーより大きい値も入れられる）" });
  num.value = String(r.weight);
  const apply = (v, save) => {
    r.weight = Math.max(0, Number(v) || 0);
    if (save) schedule_save(0);
    else preview_work();
  };
  // 動かしている間はプレビューだけ、離したら保存
  slider.addEventListener("input", () => { apply(slider.value, false); });
  slider.addEventListener("change", () => apply(slider.value, true));
  num.addEventListener("change", () => apply(num.value, true));

  const path = path_titles(t);
  return el("div", { class: "work_row" + (r.weight > 0 ? "" : " zero") + (is_selected(t.id) ? " selected" : ""), "data-task_id": t.id },
    el("div", { class: "work_name", onclick: () => select(t.id), title: "クリックで右に詳細" },
      el("span", { class: "title" + (t.status === "done" ? " done" : "") }, t.title),
      el("span", { class: "crumb" }, path.join(" › ")),
      el("span", { class: "reason" }, REASON_LABEL[r.reason] || "")),
    slider,
    num,
    el("span", { class: "work_pct" }, total > 0 ? `${pct}%` : ""),
    el("span", { class: "work_min" + (minutes > 0 ? "" : " zero") }, fmt_hours(minutes)),
    el("span", { class: "work_total", title: "このタスクの記録の合計（全期間）" }, t.actual_min ? `累計 ${fmt_hours(t.actual_min)}` : ""),
    // その日の候補（線表で重なる・その日に完了）は、外しても読み直すと割合 0 で戻ってくるので × を出さない。
    // 割合 0 なら記録には入らない
    is_day_suggestion(t.id)
      ? el("span", { class: "work_remove_placeholder" })
      : icon_button("×", "この日の振り分けから外す", () => {
        work.rows = work.rows.filter((x) => x !== r);
        schedule_save(0);
      }));
}

/// 線表でその日にバーが重なっている・その日に完了した（記録を消しても候補に出る）タスクか
function is_day_suggestion(task_id) {
  return (work.day?.suggestions ?? []).some((s) => s.task_id === task_id && s.always_suggested);
}

/// スライダーを動かしている間は、行を作り直さずに数字だけ書き換える
function preview_work() {
  const weights = work.rows.map((r) => r.weight);
  const minutes = allocate_preview(work.worked, weights);
  const total = weights.reduce((a, b) => a + b, 0);
  document.querySelectorAll("#work_rows .work_row").forEach((rowEl, i) => {
    const r = work.rows[i];
    if (!r) return;
    rowEl.classList.toggle("zero", !(r.weight > 0));
    rowEl.querySelector(".work_pct").textContent = total > 0 ? `${Math.round((r.weight / total) * 100)}%` : "";
    const m = rowEl.querySelector(".work_min");
    m.textContent = fmt_hours(minutes[i]);
    m.classList.toggle("zero", !(minutes[i] > 0));
    const num = rowEl.querySelector(".work_weight");
    if (document.activeElement !== num) num.value = String(r.weight);
  });
}

function draw_week() {
  const mon = monday_of(work.date);
  const today = today_text();
  let total = 0;
  const cells = [];
  for (let i = 0; i < 7; i++) {
    const d = add_days(mon, i);
    const s = work.week.find((x) => x.date === d);
    if (s) total += s.worked_min;
    const st = !s ? "none" : s.allocated_min === s.worked_min ? "ok" : "partial";
    cells.push(el("button", {
      type: "button",
      class: `week_day ${st}` + (d === work.date ? " active" : "") + (d === today ? " today" : "") + ([0, 6].includes(weekday_of(d)) ? " weekend" : ""),
      title: !s ? "未記録" : st === "ok" ? "振り分け済み" : "振り分けが勤務時間と合っていません",
      onclick: () => set_work_date(d),
    },
      el("span", { class: "wd" }, short_date(d)),
      el("span", { class: "hours" }, s ? fmt_hours(s.worked_min) : "—"),
      el("span", { class: "mark" }, st === "ok" ? "✓" : st === "partial" ? "!" : "")));
  }
  $("work_week").replaceChildren(...cells);
  $("work_week_total").textContent = `この週 ${fmt_effort(total)}`;
}

// ---------- 保存 ----------
function schedule_save(delay = 400) {
  work.dirty = true;
  clearTimeout(work.saveTimer);
  draw_work();
  work.saveTimer = setTimeout(save_work_day, delay);
}

/// 保存は 1 件ずつ順に送る。送っている間の変更は、終わってからまとめて送る
async function save_work_day() {
  work.saveTimer = null;
  if (work.saving) {
    work.pending = true;
    return;
  }
  work.saving = true;
  const date = work.date;
  const body = {
    worked_min: work.worked,
    allocations: work.rows.filter((r) => r.weight > 0).map((r) => ({ task_id: r.task_id, weight: r.weight })),
  };
  let failed = false;
  try {
    await api("PUT", `/work_days/${date}`, body);
  } catch {
    failed = true;
  }
  work.saving = false;
  if (work.pending && !failed) {
    work.pending = false;
    return save_work_day();
  }
  work.pending = false;
  work.dirty = false;
  work.loadedKey = null;
  await reload();
}

/// 未保存の変更を保存し終えるまで待つ
async function flush_work() {
  if (work.saveTimer) {
    clearTimeout(work.saveTimer);
    await save_work_day();
  }
  while (work.saving) await new Promise((r) => setTimeout(r, 50));
}

async function set_work_date(d) {
  await flush_work();
  work.date = d;
  work.day = null;
  work.loadedKey = null;
  load_work_day(true);
}

function setup_work() {
  $("work_prev").addEventListener("click", () => set_work_date(add_days(work.date, -1)));
  $("work_next").addEventListener("click", () => set_work_date(add_days(work.date, 1)));
  $("work_today").addEventListener("click", () => set_work_date(today_text()));
  $("work_date").addEventListener("change", (ev) => { if (ev.target.value) set_work_date(ev.target.value); });

  const setWorked = (min) => {
    work.worked = Math.max(0, Math.min(24 * 60, Math.round(min / WORK_STEP) * WORK_STEP));
    schedule_save();
  };
  $("work_minus").addEventListener("click", () => setWorked(work.worked - WORK_STEP));
  $("work_plus").addEventListener("click", () => setWorked(work.worked + WORK_STEP));
  $("work_worked").addEventListener("change", (ev) => setWorked(Number(ev.target.value) * 60));
  $("work_worked").addEventListener("keydown", (ev) => { if (ev.key === "Enter") ev.target.blur(); });

  const addInput = $("work_add_input");
  const addTask = () => {
    const label = addInput.value.trim();
    if (!label) return;
    const t = state.tasks.find((x) => task_option_label(x) === label) ||
      state.tasks.find((x) => x.title === label.replace(/（完了）$/, ""));
    if (!t) { toast("そのタスクは見つかりません", "error"); return; }
    addInput.value = "";
    const existing = work.rows.find((r) => r.task_id === t.id);
    if (existing) { if (!existing.weight) existing.weight = 1; }
    else work.rows.push({ task_id: t.id, weight: 1, reason: "added", date: work.date });
    schedule_save(0);
  };
  addInput.addEventListener("keydown", (ev) => { if (ev.key === "Enter" && !ev.isComposing) { ev.preventDefault(); addTask(); } });
  addInput.addEventListener("input", (ev) => {
    // 候補から選んだときはそのまま追加
    if (ev.inputType === "insertReplacementText" || ev.inputType === undefined) {
      if (state.tasks.some((t) => task_option_label(t) === addInput.value)) addTask();
    }
  });

  const saveSetting = async (patch) => {
    try {
      await api("PATCH", "/settings", patch);
    } finally {
      work.loadedKey = null;
      await reload();
    }
  };
  $("setting_day_hours").addEventListener("change", (ev) => saveSetting({ minutes_per_day: Math.round(Number(ev.target.value) * 60) }));
  $("setting_default_hours").addEventListener("change", (ev) => saveSetting({ default_work_min: Math.round(Number(ev.target.value) * 2) * 30 }));
}
