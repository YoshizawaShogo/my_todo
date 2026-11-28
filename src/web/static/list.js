"use strict";
// 詳細タブ。過去も未来もすべてのタスクを表で出し、細かい条件で絞り込む。

const LIST_DEFAULT = {
  text: "",
  statuses: ["todo", "doing"], // 空ならすべて
  doingAlways: false, // 作業中は日付の条件を無視して含める
  dateField: "plan",
  range: "all",
  from: "",
  to: "",
  tags: [],
  tagMode: "any", // any / all / none
  untagged: false,
  section: "all", // all / tasks / inbox
  parentId: null,
  group: "date",
  sortKey: "date",
  sortDir: "asc",
};

const list = {
  filter: structuredClone({ ...LIST_DEFAULT, ...load_json("list_filter", {}) }),
  views: load_json("list_views", []), // 保存した条件 [{name, filter}]
};

const RANGES = [
  ["all", "指定なし"],
  ["overdue", "期限切れ"],
  ["until_today", "今日まで"],
  ["today", "今日"],
  ["this_week", "今週"],
  ["next_week", "来週"],
  ["last_week", "先週"],
  ["this_month", "今月"],
  ["last_month", "先月"],
  ["future", "今日以降"],
  ["past", "今日より前"],
  ["none", "日付なし"],
  ["custom", "期間指定"],
];

const PRESETS = [
  ["今日やること", { statuses: ["todo", "doing"], doingAlways: true, dateField: "plan", range: "until_today", group: "date", sortKey: "date", sortDir: "asc" }],
  ["今週", { statuses: ["todo", "doing"], dateField: "plan", range: "this_week", group: "date", sortKey: "date", sortDir: "asc" }],
  ["来週", { statuses: ["todo", "doing"], dateField: "plan", range: "next_week", group: "date", sortKey: "date", sortDir: "asc" }],
  ["⚠️ 期限切れ", { statuses: ["todo", "doing"], dateField: "plan", range: "overdue", group: "date", sortKey: "date", sortDir: "asc" }],
  ["予定なし", { statuses: ["todo", "doing"], dateField: "plan", range: "none", group: "parent", sortKey: "created_at", sortDir: "asc" }],
  ["✅ 完了（今週）", { statuses: ["done"], dateField: "completed_at", range: "this_week", group: "date", sortKey: "completed_at", sortDir: "desc" }],
  ["✅ 完了（すべて）", { statuses: ["done"], dateField: "completed_at", range: "all", group: "date", sortKey: "completed_at", sortDir: "desc" }],
  ["すべて", { statuses: [], dateField: "plan", range: "all", group: "none", sortKey: "created_at", sortDir: "desc" }],
];

// 条件は配列（状態・タグ）を持つので、必ず複製して渡す（共有したまま push すると既定やプリセット自体が変わる）
function preset_filter(p) {
  return structuredClone({ ...LIST_DEFAULT, ...p });
}

function save_list_filter() {
  save_json("list_filter", list.filter);
  render_list();
}

// ---------- 条件の評価 ----------
function range_bounds(f, today) {
  const mon = monday_of(today);
  const [y, m] = today.split("-").map(Number);
  const month_start = (yy, mm) => `${yy}-${pad2(mm)}-01`;
  const month_end = (yy, mm) => add_days(mm === 12 ? month_start(yy + 1, 1) : month_start(yy, mm + 1), -1);
  switch (f.range) {
    case "today": return [today, today];
    case "until_today": return [null, today];
    case "this_week": return [mon, add_days(mon, 6)];
    case "next_week": return [add_days(mon, 7), add_days(mon, 13)];
    case "last_week": return [add_days(mon, -7), add_days(mon, -1)];
    case "this_month": return [month_start(y, m), month_end(y, m)];
    case "last_month": {
      const [py, pm] = m === 1 ? [y - 1, 12] : [y, m - 1];
      return [month_start(py, pm), month_end(py, pm)];
    }
    case "future": return [today, null];
    case "past": return [null, add_days(today, -1)];
    case "custom": return [f.from || null, f.to || null];
    default: return [null, null];
  }
}

/// その日付の種類での期間の一覧 [[開始, 終了], ...]
function task_intervals(t, field) {
  const day = (s) => (s ? s.slice(0, 10) : null);
  if (field === "plan") {
    const out = [];
    if (t.planned_start && t.planned_end) out.push([t.planned_start, t.planned_end]);
    else for (const d of [t.planned_start, t.planned_end]) if (d) out.push([d, d]);
    if (t.next_action_date) out.push([t.next_action_date, t.next_action_date]);
    return out;
  }
  const d = day(t[field]);
  return d ? [[d, d]] : [];
}

function overlaps([s, e], [from, to]) {
  return (to === null || s <= to) && (from === null || e >= from);
}

function match_date(t, f, today) {
  if (f.range === "all") return true;
  const iv = task_intervals(t, f.dateField);
  if (f.range === "none") return iv.length === 0;
  if (f.range === "overdue") {
    if (t.status === "done") return false;
    // 予定なら終了予定日と次の予定で判定する（開始予定日が過ぎただけでは期限切れにしない）
    const dates = f.dateField === "plan" ? [t.planned_end, t.next_action_date].filter(Boolean) : iv.map((x) => x[1]);
    return dates.some((d) => d < today);
  }
  const b = range_bounds(f, today);
  return iv.some((x) => overlaps(x, b));
}

/// 並べ替え・日付ごとのグループに使う代表の日付（期間の中で一番早いもの）
function rep_date(t, f, today) {
  const iv = task_intervals(t, f.dateField);
  if (!iv.length) return null;
  const b = f.range === "all" || f.range === "none" || f.range === "overdue" ? [null, null] : range_bounds(f, today);
  const hits = iv.filter((x) => overlaps(x, b)).map(([s]) => (b[0] && s < b[0] ? b[0] : s));
  const pool = hits.length ? hits : iv.map(([s]) => s);
  return pool.sort()[0];
}

function list_matches(t, f, today) {
  if (f.statuses.length && !f.statuses.includes(t.status)) return false;
  const doingBypass = f.doingAlways && t.status === "doing";
  if (!doingBypass && !match_date(t, f, today)) return false;
  if (f.section !== "all") {
    const inbox = root_of(t).is_inbox;
    if ((f.section === "inbox") !== inbox) return false;
  }
  if (f.parentId !== null && (t.id === f.parentId || !is_self_or_descendant(f.parentId, t.id))) return false;
  const tags = inherited_tags(t);
  if (f.untagged && tags.length) return false;
  if (f.tags.length) {
    const has = (n) => tags.some((x) => same_tag(x, n));
    if (f.tagMode === "any" && !f.tags.some(has)) return false;
    if (f.tagMode === "all" && !f.tags.every(has)) return false;
    if (f.tagMode === "none" && f.tags.some(has)) return false;
  }
  const words = f.text.trim().toLowerCase().split(/\s+/).filter(Boolean);
  if (words.length) {
    const hay = [t.title, t.note, t.next_action, ...t.tags, ...path_titles(t)].filter(Boolean).join("\n").toLowerCase();
    if (!words.every((w) => hay.includes(w))) return false;
  }
  return true;
}

// ---------- 並べ替え・グループ ----------
const SORT_COLUMNS = {
  title: (t) => t.title,
  status: (t) => ({ doing: 0, todo: 1, done: 2 })[t.status],
  planned_start: (t) => t.planned_start,
  planned_end: (t) => t.planned_end,
  next_action_date: (t) => t.next_action_date,
  completed_at: (t) => t.completed_at,
  created_at: (t) => t.created_at,
  estimate_min: (t) => t.estimate_min,
  actual: (t) => subtree_actual(t) || null,
};

function sort_value(t, f, today) {
  if (f.sortKey === "date") return rep_date(t, f, today);
  return (SORT_COLUMNS[f.sortKey] || SORT_COLUMNS.created_at)(t) ?? null;
}

function compare_values(a, b) {
  if (a === null && b === null) return 0;
  if (a === null) return 1; // 値なしは常に最後
  if (b === null) return -1;
  return a < b ? -1 : a > b ? 1 : 0;
}

function group_keys(t, f, today) {
  switch (f.group) {
    case "date": return [rep_date(t, f, today) ?? ""];
    case "status": return [t.status];
    case "parent": return [t.parent_id === null ? "" : String(t.parent_id)];
    case "tag": return t.tags.length ? t.tags : [""];
    default: return ["all"];
  }
}

function group_label(key, f, today) {
  switch (f.group) {
    case "date": {
      if (!key) return "日付なし";
      const rel = key === today ? "今日 " : key === add_days(today, 1) ? "明日 " : key === add_days(today, -1) ? "昨日 " : key < today && f.dateField === "plan" ? "過ぎた予定 " : "";
      return rel + short_date(key);
    }
    case "status": return STATUS_LABEL[key];
    case "parent": {
      if (!key) return "（最上位）";
      const p = state.byId.get(Number(key));
      return p ? [...path_titles(p), p.title].join(" › ") : "";
    }
    case "tag": return key || "タグなし";
    default: return "";
  }
}

// ---------- 描画 ----------
function render_list() {
  const f = list.filter;
  const today = today_text();
  render_list_controls(f);

  const rows = state.tasks.filter((t) => list_matches(t, f, today));
  const dir = f.sortDir === "desc" ? -1 : 1;
  rows.sort((a, b) => {
    const va = sort_value(a, f, today), vb = sort_value(b, f, today);
    // 値なしは並び順に関係なく最後
    if (va === null || vb === null) return compare_values(va, vb) || a.id - b.id;
    return dir * compare_values(va, vb) || a.id - b.id;
  });
  $("list_count").textContent = `${rows.length} 件`;

  // グループに分ける（並び順を保つ）
  const groups = new Map();
  for (const t of rows) {
    for (const k of group_keys(t, f, today)) {
      if (!groups.has(k)) groups.set(k, []);
      groups.get(k).push(t);
    }
  }
  let keys = [...groups.keys()];
  if (f.group === "date") {
    keys.sort((a, b) => (a === "" ? 1 : b === "" ? -1 : (f.sortKey === "date" || f.sortKey === f.dateField ? dir : 1) * a.localeCompare(b)));
  } else if (f.group === "status") {
    keys.sort((a, b) => SORT_COLUMNS.status({ status: a }) - SORT_COLUMNS.status({ status: b }));
  } else if (f.group === "tag" || f.group === "parent") {
    keys.sort((a, b) => (a === "" ? 1 : b === "" ? -1 : group_label(a, f, today).localeCompare(group_label(b, f, today), "ja")));
  }

  const head = el("tr", {},
    el("th", { class: "col_check" }),
    sort_header("名前", "title", "col_title"),
    sort_header("状態", "status", "col_status"),
    sort_header("開始", "planned_start", "col_date"),
    sort_header("終了", "planned_end", "col_date"),
    sort_header("次の予定", "next_action_date", "col_next"),
    sort_header("完了", "completed_at", "col_date"),
    sort_header("見積", "estimate_min", "col_effort"),
    sort_header("実績", "actual", "col_effort"),
  );
  const body = el("tbody");
  if (!rows.length) {
    body.append(el("tr", {}, el("td", { colspan: 9, class: "empty_hint" }, "条件に合うタスクはありません")));
  }
  for (const k of keys) {
    if (f.group !== "none") {
      body.append(el("tr", { class: "group_row" }, el("td", { colspan: 9 },
        group_label(k, f, today), el("span", { class: "count" }, String(groups.get(k).length)))));
    }
    for (const t of groups.get(k)) body.append(list_row(t, today));
  }
  const table = el("table", { class: "list_table" }, el("thead", {}, head), body);
  $("list_table").replaceChildren(table);
}

function sort_header(label, key, cls) {
  const f = list.filter;
  const active = f.sortKey === key;
  return el("th", {
    class: cls + " sortable" + (active ? " sorted" : ""),
    title: "クリックで並べ替え",
    onclick: () => {
      if (f.sortKey === key) f.sortDir = f.sortDir === "asc" ? "desc" : "asc";
      else { f.sortKey = key; f.sortDir = "asc"; }
      save_list_filter();
    },
  }, label, active ? (f.sortDir === "asc" ? " ▲" : " ▼") : "");
}

function date_cell(d, t, today, extra) {
  const cls = !d || t.status === "done" ? "" : d < today ? " overdue" : d === today ? " today" : "";
  return el("td", { class: "col_date" + cls }, d ? short_date(d) : "", extra);
}

function effort_cell(t) {
  const actual = subtree_actual(t);
  const over = t.estimate_min && actual > t.estimate_min;
  return el("td", {
    class: "col_effort" + (over ? " overdue" : ""),
    title: actual ? fmt_effort(actual) + (actual !== t.actual_min ? "（子を含む）" : "") : "",
  }, actual ? fmt_hours(actual) : "");
}

function list_row(t, today) {
  const done = t.status === "done";
  const row = el("tr", {
    class: "list_row" + (done ? " done" : "") + (is_selected(t.id) ? " selected" : ""),
    "data-task_id": t.id,
    onclick: () => select(t.id),
  });
  // 完了のチェックボックスの代わりに、状態の印（表示だけ。状態は詳細欄で変える）
  const check = status_mark(t);
  const path = path_titles(t);
  const title = el("span", {
    class: "title",
    ondblclick: (ev) => { ev.stopPropagation(); start_inline_edit(title, t); },
    title: "ダブルクリックで名前を変更",
  }, t.title);
  const nameCell = el("td", { class: "col_title" },
    el("div", { class: "title_line" }, title,
      ...estimate_badge(t),
      ...link_mark(t),
      ...tag_chips(t.tags, (n) => list.filter.tags.some((x) => same_tag(x, n)), (n) => { toggle_in(list.filter.tags, n); list.filter.untagged = false; save_list_filter(); })),
    el("div", { class: "crumb" }, path.length ? path.join(" › ") : t.is_inbox ? "未分類" : ""));

  let diff = null;
  if (done && t.planned_end && t.completed_at) {
    const n = days_between(t.planned_end, t.completed_at.slice(0, 10));
    diff = el("span", {
      class: "plan_diff" + (n > 0 ? " late" : n < 0 ? " early" : ""),
      title: `終了予定日 ${short_date(t.planned_end)}`,
    }, n > 0 ? `${n}日遅れ` : n < 0 ? `${-n}日早い` : "予定どおり");
  }
  const next = t.next_action_date || t.next_action
    ? el("td", { class: "col_next" + (done ? "" : t.next_action_date && t.next_action_date < today ? " overdue" : t.next_action_date === today ? " today" : "") },
      t.next_action_date ? short_date(t.next_action_date) + " " : "", t.next_action || "")
    : el("td", { class: "col_next" });

  row.append(
    el("td", { class: "col_check" }, check),
    nameCell,
    el("td", { class: "col_status" }, el("span", { class: "status_badge " + t.status }, STATUS_LABEL[t.status])),
    date_cell(t.planned_start, t, today),
    date_cell(t.planned_end, t, today),
    next,
    el("td", { class: "col_date" }, t.completed_at ? short_date(t.completed_at.slice(0, 10)) : "", diff ? " " : "", diff),
    el("td", { class: "col_effort" }, t.estimate_min ? fmt_hours(t.estimate_min) : ""),
    effort_cell(t),
  );
  return row;
}

// ---------- 条件の入力欄 ----------
function same_filter(a, b) {
  const pick = (f) => JSON.stringify({ ...f, text: "", tags: [], untagged: false, section: "all", parentId: null, tagMode: "any" });
  return pick(a) === pick(b);
}

function render_list_controls(f) {
  // プリセットと保存した条件
  const presets = PRESETS.map(([name, p]) => el("button", {
    type: "button",
    class: "preset" + (same_filter(f, preset_filter(p)) ? " active" : ""),
    onclick: () => {
      // キーワード・タグ・場所はそのまま残す
      list.filter = structuredClone({ ...preset_filter(p), text: f.text, tags: f.tags, tagMode: f.tagMode, untagged: f.untagged, section: f.section, parentId: f.parentId });
      save_list_filter();
    },
  }, name));
  const saved = list.views.map((v, i) => el("span", { class: "preset saved" + (JSON.stringify(v.filter) === JSON.stringify(f) ? " active" : "") },
    el("button", { type: "button", class: "preset_name", onclick: () => { list.filter = structuredClone({ ...LIST_DEFAULT, ...v.filter }); save_list_filter(); } }, "★ " + v.name),
    el("button", {
      type: "button",
      class: "remove",
      title: "この保存した条件を消す",
      onclick: () => {
        if (!confirm(`保存した条件「${v.name}」を消しますか？`)) return;
        list.views.splice(i, 1);
        save_json("list_views", list.views);
        render_list();
      },
    }, "×")));
  $("list_presets").replaceChildren(
    el("span", { class: "preset_label" }, "プリセット"),
    ...presets,
    ...(saved.length ? [el("span", { class: "toolbar_sep" }), el("span", { class: "preset_label" }, "保存した条件"), ...saved] : []));

  const text = $("lf_text");
  if (document.activeElement !== text) text.value = f.text;

  $("lf_status").replaceChildren(...["todo", "doing", "done"].map((s) => el("button", {
    type: "button",
    class: "chip" + (f.statuses.includes(s) ? " active" : ""),
    onclick: () => {
      const i = f.statuses.indexOf(s);
      if (i >= 0) f.statuses.splice(i, 1);
      else f.statuses.push(s);
      save_list_filter();
    },
  }, STATUS_LABEL[s])));
  $("lf_doing_always").checked = f.doingAlways;

  $("lf_date_field").value = f.dateField;
  $("lf_range").replaceChildren(...RANGES.map(([k, label]) => el("button", {
    type: "button",
    class: "chip" + (f.range === k ? " active" : ""),
    // 選択中の範囲をもう一度押すと「指定なし」に戻す
    onclick: () => { f.range = f.range === k ? "all" : k; save_list_filter(); },
  }, label)));
  $("lf_custom").hidden = f.range !== "custom";
  $("lf_from").value = f.from;
  $("lf_to").value = f.to;

  $("lf_tags").replaceChildren(
    // 「タグなし」とタグの選択は同時に成り立たないので、片方を選ぶともう片方を外す
    ...tag_filter_buttons(() => list.filter.tags, (n) => { toggle_in(list.filter.tags, n); list.filter.untagged = false; save_list_filter(); }),
    el("button", {
      type: "button",
      class: "chip" + (f.untagged ? " active" : ""),
      title: "自分にも祖先にもタグがないもの",
      onclick: () => {
        f.untagged = !f.untagged;
        if (f.untagged) f.tags = [];
        save_list_filter();
      },
    }, "タグなし"));
  $("lf_tag_mode").value = f.tagMode;

  $("lf_section").value = f.section;
  // 親の候補：子を持つタスクを木の順に
  const options = [el("option", { value: "" }, "親：指定なし")];
  const walk = (key, depth) => {
    for (const t of children_of(key)) {
      if (children_of(t.id).length) {
        options.push(el("option", { value: String(t.id) }, `${"　".repeat(depth)}${t.title} の下`));
        walk(t.id, depth + 1);
      }
    }
  };
  walk("root", 0);
  walk("inbox", 0);
  $("lf_parent").replaceChildren(...options);
  $("lf_parent").value = f.parentId === null ? "" : String(f.parentId);
  if (f.parentId !== null && !state.byId.has(f.parentId)) f.parentId = null;

  $("lf_group").value = f.group;
}

function setup_list() {
  const f = () => list.filter;
  let timer;
  $("lf_text").addEventListener("input", (ev) => {
    clearTimeout(timer);
    timer = setTimeout(() => { f().text = ev.target.value; save_list_filter(); }, 150);
  });
  $("lf_text").addEventListener("keydown", (ev) => {
    if (ev.key === "Escape") { ev.target.value = ""; f().text = ""; save_list_filter(); }
  });
  $("lf_doing_always").addEventListener("change", (ev) => { f().doingAlways = ev.target.checked; save_list_filter(); });
  $("lf_date_field").addEventListener("change", (ev) => { f().dateField = ev.target.value; save_list_filter(); });
  $("lf_from").addEventListener("change", (ev) => { f().from = ev.target.value; save_list_filter(); });
  $("lf_to").addEventListener("change", (ev) => { f().to = ev.target.value; save_list_filter(); });
  $("lf_tag_mode").addEventListener("change", (ev) => { f().tagMode = ev.target.value; save_list_filter(); });
  $("lf_section").addEventListener("change", (ev) => { f().section = ev.target.value; save_list_filter(); });
  $("lf_parent").addEventListener("change", (ev) => { f().parentId = ev.target.value ? Number(ev.target.value) : null; save_list_filter(); });
  $("lf_group").addEventListener("change", (ev) => { f().group = ev.target.value; save_list_filter(); });
  $("lf_reset").addEventListener("click", () => { list.filter = structuredClone(LIST_DEFAULT); save_list_filter(); });
  $("lf_save").addEventListener("click", () => {
    const name = prompt("この条件に名前を付けて保存します", "")?.trim();
    if (!name) return;
    const i = list.views.findIndex((v) => v.name === name);
    const view = { name, filter: JSON.parse(JSON.stringify(list.filter)) };
    if (i >= 0) list.views[i] = view;
    else list.views.push(view);
    save_json("list_views", list.views);
    render_list();
  });
}
