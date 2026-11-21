"use strict";
// 共通部分：状態・API・木の操作・詳細パネル・元に戻す・タブ。
// 線表は gantt.js、詳細タブは list.js、工数記録タブは work.js、日付の入力は datepicker.js。

// ---------- 状態 ----------
const state = {
  tasks: [],
  byId: new Map(),
  children: new Map(), // parent_id / "root" / "inbox" -> [task]（sort_order 順）
  tags: [], // [{name, count}]
  settings: null, // {minutes_per_day, default_work_min}
  selectedId: null, // 詳細パネルに出すタスク（最後にクリックしたもの）
  selected: new Set(), // 選んでいるタスク（Shift・Ctrl＋クリックで複数）
  anchorId: null, // Shift＋クリックの範囲の起点
  dragIds: null, // ドラッグ中のタスク（複数選択ならすべて、画面の並び順）
  tab: load_json("tab", "gantt"), // gantt / list
  collapsed: new Set(load_json("collapsed", [])),
  dragId: null,
};

function load_json(key, fallback) {
  try {
    const v = localStorage.getItem("my_todo." + key);
    return v === null ? fallback : JSON.parse(v);
  } catch {
    return fallback;
  }
}

function save_json(key, value) {
  try {
    localStorage.setItem("my_todo." + key, JSON.stringify(value));
  } catch {
    // 保存できなくても動作は続ける
  }
}

const $ = (id) => document.getElementById(id);

// ---------- API ----------
async function api(method, path, body) {
  let res;
  try {
    res = await fetch("/api" + path, {
      method,
      headers: body ? { "Content-Type": "application/json" } : {},
      body: body ? JSON.stringify(body) : undefined,
    });
  } catch (e) {
    // アプリ（my_todo.exe）が止まっている
    set_offline(true);
    toast("アプリと通信できません。my_todo を起動し直してください", "error");
    throw e;
  }
  set_offline(false, { reload: false });
  if (!res.ok) {
    let msg = res.statusText;
    try {
      msg = (await res.json()).error || msg;
    } catch {}
    toast(msg, "error");
    throw new Error(msg);
  }
  return res.status === 204 ? null : res.json();
}

let toastTimer;
function toast(msg, kind = "info") {
  const t = $("toast");
  t.textContent = msg;
  t.className = "toast " + kind;
  t.hidden = false;
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => (t.hidden = true), 3000);
}

let reloadCount = 0;

async function reload() {
  const [tasks, tags, settings] = await Promise.all([api("GET", "/tasks"), api("GET", "/tags"), api("GET", "/settings")]);
  reloadCount++;
  state.settings = settings;
  state.tags = tags;
  state.tasks = tasks;
  state.byId = new Map(tasks.map((t) => [t.id, t]));
  state.children = new Map();
  for (const t of tasks) {
    const key = sibling_key(t);
    if (!state.children.has(key)) state.children.set(key, []);
    state.children.get(key).push(t);
  }
  for (const list of state.children.values()) list.sort((a, b) => a.sort_order - b.sort_order || a.id - b.id);
  refresh_unrecorded().catch(() => {});
  if (state.selectedId !== null && !state.byId.has(state.selectedId)) state.selectedId = null;
  state.selected = new Set([...state.selected].filter((id) => state.byId.has(id)));
  render();
}

// ---------- 木 ----------
function children_of(key) {
  return state.children.get(key) || [];
}

function sibling_key(task) {
  return task.parent_id ?? (task.is_inbox ? "inbox" : "root");
}

function parent_of(task) {
  return task.parent_id === null ? null : state.byId.get(task.parent_id);
}

/// 親から順に並べた祖先のタイトル
function path_titles(task) {
  const path = [];
  for (let cur = parent_of(task); cur; cur = parent_of(cur)) path.unshift(cur.title);
  return path;
}

/// 最上位の祖先（自分が最上位なら自分）
function root_of(task) {
  let cur = task;
  while (cur.parent_id !== null) cur = parent_of(cur);
  return cur;
}

/// 自分か祖先に付いているタグ（親のタグは子にも効く）
function inherited_tags(task) {
  const tags = [];
  for (let cur = task; cur; cur = parent_of(cur)) tags.push(...cur.tags);
  return tags;
}

function is_self_or_descendant(id, maybeDescendantId) {
  for (let cur = state.byId.get(maybeDescendantId); cur; cur = parent_of(cur)) {
    if (cur.id === id) return true;
  }
  return false;
}

/// 自分と子孫の工数の合計（分）
function subtree_actual(task) {
  return task.actual_min + children_of(task.id).reduce((n, c) => n + subtree_actual(c), 0);
}

function count_descendants(id) {
  return children_of(id).reduce((n, c) => n + 1 + count_descendants(c.id), 0);
}

function expand(id) {
  state.collapsed.delete(id);
  save_json("collapsed", [...state.collapsed]);
}

function toggle_collapse(id) {
  if (state.collapsed.has(id)) state.collapsed.delete(id);
  else state.collapsed.add(id);
  save_json("collapsed", [...state.collapsed]);
  render();
}

// ---------- 変更 ----------
function move_target(key) {
  if (key === "inbox") return { parent_id: null, inbox: true };
  if (key === "root") return { parent_id: null, inbox: false };
  return { parent_id: key, inbox: false };
}

async function move(id, key, index) {
  await api("POST", `/tasks/${id}/move`, { ...move_target(key), index });
  await reload();
}

/// ref の前（before）か後ろ（after）へ移す
async function move_relative(id, refId, where) {
  const ref = state.byId.get(refId);
  const key = sibling_key(ref);
  const list = children_of(key).filter((t) => t.id !== id);
  const refIndex = list.findIndex((t) => t.id === refId);
  await move(id, key, where === "before" ? refIndex : refIndex + 1);
}

/// key は親の id か "root"（タスク）/ "inbox"（未分類）
async function create(title, key) {
  const body = typeof key === "number" ? { title, parent_id: key } : { title, inbox: key !== "root" };
  return api("POST", "/tasks", body);
}

async function update(id, patch) {
  const t = await api("PATCH", `/tasks/${id}`, patch);
  await reload();
  return t;
}

async function set_done(task, done) {
  await update(task.id, { status: done ? "done" : "todo" });
  if (done) toast(`✅「${task.title}」を完了にしました（Ctrl+Z で元に戻す）`);
}

async function remove(id) {
  const task = state.byId.get(id);
  const n = count_descendants(id);
  const msg = n > 0
    ? `🗑️「${task.title}」と子タスク ${n} 件を削除しますか？\n（削除したあとでも Ctrl+Z で元に戻せます）`
    : `🗑️「${task.title}」を削除しますか？\n（削除したあとでも Ctrl+Z で元に戻せます）`;
  if (!confirm(msg)) return;
  await api("DELETE", `/tasks/${id}`);
  if (state.selectedId === id) state.selectedId = null;
  state.selected.delete(id);
  await reload();
  toast(`🗑️「${task.title}」を削除しました（Ctrl+Z で元に戻す）`);
}

// ---------- 元に戻す・やり直し ----------
let undoBusy = false;

async function undo_redo(kind) {
  if (undoBusy) return;
  undoBusy = true;
  try {
    // 書きかけのメモなどを先に保存してから戻す
    document.activeElement?.blur?.();
    const { label } = await api("POST", "/" + kind);
    await reload();
    if (label) toast(kind === "undo" ? `↩️ 元に戻しました：${label}` : `↪️ やり直しました：${label}`);
    else toast(kind === "undo" ? "元に戻せる操作はありません" : "やり直せる操作はありません");
  } finally {
    undoBusy = false;
  }
}

// ---------- 描画の道具 ----------
function el(tag, attrs = {}, ...kids) {
  const e = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) {
    if (k === "class") e.className = v;
    else if (k.startsWith("on")) e.addEventListener(k.slice(2), v);
    else if (v !== false && v !== null && v !== undefined) e.setAttribute(k, v === true ? "" : v);
  }
  for (const kid of kids) if (kid !== null && kid !== undefined && kid !== false) e.append(kid);
  return e;
}

function icon_button(label, title, onclick, extraClass = "") {
  return el("button", {
    type: "button",
    class: "icon_button " + extraClass,
    title,
    onclick: (ev) => {
      ev.stopPropagation();
      onclick();
    },
  }, label);
}

function same_tag(a, b) {
  return a.toLowerCase() === b.toLowerCase();
}

/// タグ名から色相を決める（同じ名前はいつも同じ色）。FNV-1a でハッシュし、黄金角で散らす
function tag_style(name) {
  let h = 2166136261;
  for (const ch of name.toLowerCase()) {
    h ^= ch.codePointAt(0);
    h = Math.imul(h, 16777619) >>> 0;
  }
  return `--tag_h:${Math.round((h % 1000) * 137.508) % 360}`;
}

/// タグのチップ。active(name) が真のものは強調、クリックで onToggle(name)
function tag_chips(tags, active, onToggle) {
  return tags.map((name) => el("span", {
    class: "tag_chip" + (active(name) ? " active" : ""),
    style: tag_style(name),
    title: "クリックでこのタグに絞り込む",
    onclick: (ev) => {
      ev.stopPropagation();
      onToggle(name);
    },
  }, name));
}

/// タグの絞り込みボタン（件数付き）
function tag_filter_buttons(selected, onToggle) {
  return state.tags.map((t) => {
    const on = selected.some((n) => same_tag(n, t.name));
    return el("button", {
      type: "button",
      class: "tag_chip" + (on ? " active" : ""),
      style: tag_style(t.name),
      onclick: () => onToggle(t.name),
    }, t.name, el("span", { class: "count" }, String(t.count)));
  });
}

function toggle_in(list, name) {
  const i = list.findIndex((t) => same_tag(t, name));
  if (i >= 0) list.splice(i, 1);
  else list.push(name);
}

/// タイトルをその場で編集する
function start_inline_edit(titleEl, task, onDone) {
  const input = el("input", { class: "title_input", type: "text" });
  input.value = task.title;
  let finished = false;
  const finish = async (save) => {
    if (finished) return;
    finished = true;
    const value = input.value.trim();
    if (save && value && value !== task.title) await update(task.id, { title: value });
    else render();
    onDone?.();
  };
  input.addEventListener("keydown", (ev) => {
    if (ev.key === "Enter" && !ev.isComposing) finish(true);
    if (ev.key === "Escape") finish(false);
  });
  input.addEventListener("blur", () => finish(true));
  for (const type of ["click", "mousedown", "pointerdown", "dblclick"]) input.addEventListener(type, (ev) => ev.stopPropagation());
  titleEl.replaceWith(input);
  input.focus();
  input.select();
}

// ---------- 日付 ----------
const pad2 = (n) => String(n).padStart(2, "0");

function today_text() {
  const d = new Date();
  return `${d.getFullYear()}-${pad2(d.getMonth() + 1)}-${pad2(d.getDate())}`;
}

function add_days(ymd, n) {
  const [y, m, d] = ymd.split("-").map(Number);
  const dt = new Date(y, m - 1, d + n);
  return `${dt.getFullYear()}-${pad2(dt.getMonth() + 1)}-${pad2(dt.getDate())}`;
}

function days_between(a, b) {
  const [ya, ma, da] = a.split("-").map(Number);
  const [yb, mb, db] = b.split("-").map(Number);
  return Math.round((Date.UTC(yb, mb - 1, db) - Date.UTC(ya, ma - 1, da)) / 86400000);
}

function weekday_of(ymd) {
  const [y, m, d] = ymd.split("-").map(Number);
  return new Date(y, m - 1, d).getDay();
}

/// その週の月曜
function monday_of(ymd) {
  return add_days(ymd, -((weekday_of(ymd) + 6) % 7));
}

/// "2026-11-25" → "11/25(水)"。今年以外は年も付ける
function short_date(ymd) {
  if (!ymd || !/^\d{4}-\d{2}-\d{2}$/.test(ymd)) return ymd ?? "";
  const [y, m, d] = ymd.split("-").map(Number);
  const prefix = y === new Date().getFullYear() ? "" : `${y}/`;
  return `${prefix}${m}/${d}(${"日月火水木金土"[weekday_of(ymd)]})`;
}

function fmt(s) {
  return s ? s.replace("T", " ").slice(0, 16) : "";
}

const STATUS_LABEL = { todo: "未着手", doing: "作業中", done: "完了" };

/// タスクの異常（線表の枠の色と、ヘッダーの「⚠」の一覧で使う）
/// - overdue：終了予定日を過ぎたのに完了していない
/// - late_start：開始予定日を過ぎたのに未着手（期限切れのときは期限切れを優先）
/// - next_overdue：次の予定の日を過ぎた（未完了）
function task_alerts(t, today = today_text()) {
  if (t.status === "done") return { overdue: false, late_start: false, next_overdue: false };
  const overdue = Boolean(t.planned_end && t.planned_end < today);
  return {
    overdue,
    late_start: !overdue && t.status === "todo" && Boolean(t.planned_start && t.planned_start < today),
    next_overdue: Boolean(t.next_action_date && t.next_action_date < today),
  };
}

// ---------- タブ ----------
function render() {
  for (const b of document.querySelectorAll(".tab")) {
    const active = b.dataset.tab === state.tab;
    b.classList.toggle("active", active);
    b.setAttribute("aria-selected", active);
  }
  for (const name of TABS) $("view_" + name).hidden = state.tab !== name;
  render_alerts();
  if (state.tab === "gantt") render_gantt();
  else if (state.tab === "list") render_list();
  else if (state.tab === "work") render_work();
  else if (state.tab === "jira") render_jira();
  else render_help();
  render_detail();
}

const TABS = ["gantt", "list", "work", "jira", "help"];

function switch_tab(tab) {
  if (state.tab === tab) return;
  state.tab = tab;
  save_json("tab", tab);
  pendingAdd = null;
  render();
  window.scrollTo(0, 0);
}

// ---------- 連続追加 ----------
let pendingAdd = null; // { key }：親の id / "root" / "inbox"

function begin_add(key) {
  if (state.tab !== "gantt") switch_tab("gantt");
  pendingAdd = { key };
  if (typeof key === "number") expand(key);
  render();
  // 表示した直後から打ち込めるように、すぐにフォーカスする
  const input = document.querySelector("[data-pending_add]");
  input?.focus();
  input?.scrollIntoView({ block: "nearest" });
}

/// 追加用の入力欄。Enter で追加して続けて入力、Esc か欄の外で終了
function add_input(pending) {
  const input = el("input", {
    class: "title_input",
    type: "text",
    placeholder: "名前を入力して Enter（続けて追加 / Esc で終了）",
    "data-pending_add": true,
  });
  input.addEventListener("keydown", async (ev) => {
    if (ev.key === "Escape") {
      pendingAdd = null;
      render();
    } else if (ev.key === "Enter" && !ev.isComposing) {
      const title = input.value.trim();
      if (!title) return;
      input.value = "";
      await create(title, pending.key);
      await reload();
      document.querySelector("[data-pending_add]")?.focus();
    }
  });
  input.addEventListener("blur", () => {
    setTimeout(() => {
      if (pendingAdd === pending && !document.activeElement?.hasAttribute("data-pending_add")) {
        pendingAdd = null;
        render();
      }
    }, 150);
  });
  for (const type of ["click", "mousedown", "pointerdown"]) input.addEventListener(type, (ev) => ev.stopPropagation());
  requestAnimationFrame(() => input.focus());
  return input;
}

// ---------- 名前のドラッグ（並べ替え・階層変更） ----------
function drop_zone(rowEl, ev) {
  const rect = rowEl.getBoundingClientRect();
  const y = (ev.clientY - rect.top) / rect.height;
  if (y < 0.25) return "before";
  if (y > 0.75) return "after";
  return "into";
}

function clear_drop_marks() {
  document.querySelectorAll(".drop_before, .drop_after, .drop_into").forEach((e) =>
    e.classList.remove("drop_before", "drop_after", "drop_into"));
}

/// ドラッグしているタスク（のどれか）の上や子孫の上には落とせない
function dragging_onto_self(targetId) {
  return (state.dragIds || [state.dragId]).some((id) => is_self_or_descendant(id, targetId));
}

/// 複数のタスクをまとめて移動する（1 回の操作として元に戻せる）
async function move_many(ids, placement) {
  await api("POST", "/tasks/move_many", { ids, placement });
  await reload();
  toast(`${ids.length} 件を移動しました（Ctrl+Z で元に戻す）`);
}

/// handle をつかんでドラッグし、rowEl の上に落とすと前後・子に移す
function attach_tree_drag(handle, rowEl, task) {
  handle.draggable = true;
  handle.addEventListener("dragstart", (ev) => {
    // 選んでいる行をつかんだら、選んでいるものをすべて（画面の並び順で）動かす
    const ids = is_selected(task.id) && state.selected.size > 1
      ? visible_task_order().filter((id) => state.selected.has(id))
      : [task.id];
    state.dragId = task.id;
    state.dragIds = ids;
    ev.dataTransfer.effectAllowed = "move";
    ev.dataTransfer.setData("text/plain", ids.join(","));
    if (ids.length > 1) {
      const ghost = el("div", { class: "drag_ghost" }, `${ids.length} 件を移動`);
      document.body.append(ghost);
      ev.dataTransfer.setDragImage(ghost, 10, 10);
      setTimeout(() => ghost.remove(), 0);
    }
  });
  handle.addEventListener("dragend", () => {
    state.dragId = null;
    state.dragIds = null;
    clear_drop_marks();
  });
  rowEl.addEventListener("dragover", (ev) => {
    if (state.dragId === null || dragging_onto_self(task.id)) return;
    ev.preventDefault();
    clear_drop_marks();
    rowEl.classList.add("drop_" + drop_zone(rowEl, ev));
  });
  rowEl.addEventListener("dragleave", () => rowEl.classList.remove("drop_before", "drop_after", "drop_into"));
  rowEl.addEventListener("drop", async (ev) => {
    ev.preventDefault();
    const id = state.dragId;
    const ids = state.dragIds || [id];
    clear_drop_marks();
    if (id === null || dragging_onto_self(task.id)) return;
    const zone = drop_zone(rowEl, ev);
    if (ids.length > 1) {
      if (zone === "into") expand(task.id);
      await move_many(ids, zone === "into" ? { kind: "into", parent_id: task.id, inbox: false } : { kind: zone, ref_id: task.id });
    } else if (zone === "into") {
      expand(task.id);
      await move(id, task.id, null);
    } else {
      await move_relative(id, task.id, zone);
    }
  });
}

/// グループ見出し（タスク / 未分類）に落とすと、その最上位の末尾へ
function attach_group_drop(rowEl, key) {
  rowEl.addEventListener("dragover", (ev) => {
    if (state.dragId === null) return;
    ev.preventDefault();
    clear_drop_marks();
    rowEl.classList.add("drop_into");
  });
  rowEl.addEventListener("dragleave", () => rowEl.classList.remove("drop_into"));
  rowEl.addEventListener("drop", async (ev) => {
    ev.preventDefault();
    clear_drop_marks();
    if (state.dragId === null) return;
    const ids = state.dragIds || [state.dragId];
    if (ids.length > 1) await move_many(ids, { kind: "into", parent_id: null, inbox: key === "inbox" });
    else await move(state.dragId, key, null);
  });
}

// ---------- 詳細パネル ----------
/// タスクを選ぶ。クリックで 1 件、Shift＋クリックで範囲、Ctrl（Mac は ⌘）＋クリックで足す・外す
function select(id, ev) {
  if (id === null) {
    state.selected = new Set();
    state.selectedId = null;
    state.anchorId = null;
  } else if (ev?.shiftKey && state.anchorId !== null) {
    const order = visible_task_order();
    const a = order.indexOf(state.anchorId), b = order.indexOf(id);
    state.selected = a >= 0 && b >= 0 ? new Set(order.slice(Math.min(a, b), Math.max(a, b) + 1)) : new Set([id]);
    state.selectedId = id;
  } else if (ev && (ev.ctrlKey || ev.metaKey)) {
    if (state.selected.has(id)) state.selected.delete(id);
    else state.selected.add(id);
    state.selectedId = state.selected.has(id) ? id : ([...state.selected].pop() ?? null);
    state.anchorId = id;
  } else {
    state.selected = new Set([id]);
    state.selectedId = id;
    state.anchorId = id;
  }
  apply_selection_classes();
  render_detail();
}

function is_selected(id) {
  return state.selected.has(id);
}

/// 今のタブで見えているタスクの並び順（線表 → 未分類のように画面の上から）
function visible_task_order() {
  const ids = [...document.querySelectorAll(`#view_${state.tab} [data-task_id]`)].map((e) => Number(e.dataset.task_id));
  return ids.filter((v, i) => ids.indexOf(v) === i);
}

/// 表は作り直さずに、選択の見た目だけ変える（ダブルクリックやドラッグを妨げないため）
function apply_selection_classes() {
  document.querySelectorAll("[data-task_id].selected").forEach((r) => r.classList.remove("selected"));
  for (const id of state.selected) {
    document.querySelectorAll(`[data-task_id="${id}"]`).forEach((r) => r.classList.add("selected"));
  }
}

let renderedDetailId = null;
let historyKey = null; // 履歴を取り直すかの判定用（id と再読み込みの回数）

function render_detail() {
  const panel = $("detail");
  const task = state.selectedId === null ? null : state.byId.get(state.selectedId);
  panel.querySelector(".detail_header h2").textContent = state.selected.size > 1 ? `詳細（${state.selected.size} 件選択中）` : "詳細";
  panel.hidden = !task;
  if (!task) {
    renderedDetailId = null;
    return;
  }
  // 入力中の欄は上書きしない
  const active = document.activeElement;
  const fill = (id, value) => {
    if (renderedDetailId !== task.id || active !== $(id)) $(id).value = value ?? "";
  };
  fill("detail_title", task.title);
  fill("detail_note", task.note);
  fill("detail_planned_start", task.planned_start);
  fill("detail_planned_end", task.planned_end);
  fill("detail_next_action_date", task.next_action_date);
  fill("detail_next_action", task.next_action);
  $("detail_status").value = task.status;
  fill("detail_jira_key", task.jira_key);
  const jiraInfo = $("detail_jira_info");
  if (task.jira_key) {
    jiraInfo.replaceChildren(jira_key_link(task.jira_key), jira.config?.base_url ? " を Jira で開く" : "（Jira の URL は Jira タブで設定）");
  } else {
    // 自分に紐付けがなくても、親の課題に工数がまとまることを知らせる
    let cur = parent_of(task);
    while (cur && !cur.jira_key) cur = parent_of(cur);
    jiraInfo.replaceChildren(cur ? `工数は親の ${cur.jira_key}（${cur.title}）に送られます` : "");
  }
  fill("detail_estimate", task.estimate_min === null ? "" : fmt_hours(task.estimate_min));
  const actual = subtree_actual(task);
  const parts = [];
  if (actual) parts.push(`記録 ${fmt_effort(actual)}${actual !== task.actual_min ? "（子を含む）" : ""}`);
  if (task.estimate_min) {
    parts.push(`見積もり ${fmt_effort(task.estimate_min)}`);
    if (actual) parts.push(`消化 ${Math.round((actual / task.estimate_min) * 100)}%`);
  }
  $("detail_actual").textContent = parts.join(" ／ ") || "まだ工数の記録はありません（工数記録タブで振り分け）";
  $("detail_actual").classList.toggle("over", Boolean(task.estimate_min && actual > task.estimate_min));
  $("detail_tags").replaceChildren(...task.tags.map((name) => el("span", { class: "tag_chip", style: tag_style(name) },
    name,
    el("button", {
      type: "button",
      class: "remove",
      title: "外す",
      onclick: () => update(task.id, { tags: task.tags.filter((t) => t !== name) }),
    }, "×"))));
  $("tag_options").replaceChildren(...state.tags
    .filter((t) => !task.tags.some((n) => same_tag(n, t.name)))
    .map((t) => el("option", { value: t.name })));

  const meta = [`登録: ${fmt(task.created_at)}`];
  if (task.started_at) meta.push(`着手: ${fmt(task.started_at)}`);
  if (task.completed_at) meta.push(`完了: ${fmt(task.completed_at)}`);
  const path = path_titles(task);
  if (path.length) meta.unshift(`場所: ${path.join(" › ")}`);
  else meta.unshift(`場所: ${task.is_inbox ? "未分類" : "タスク"}`);
  $("detail_meta").textContent = meta.join("\n");
  renderedDetailId = task.id;

  const key = `${task.id}@${reloadCount}`;
  if (historyKey !== key) {
    historyKey = key;
    load_history(task.id, key);
  }
}

async function load_history(id, key) {
  const events = await api("GET", `/tasks/${id}/events`);
  if (historyKey !== key) return; // 取得中に別のタスクへ切り替わった
  $("detail_history").replaceChildren(
    ...events.reverse().map((e) => el("li", {}, el("time", {}, fmt(e.at)), describe_event(e))),
  );
}

function describe_event(e) {
  const o = e.old_value, n = e.new_value;
  const change = (label, conv = (v) => v) =>
    o === null ? `${label}を設定: ${conv(n)}`
    : n === null ? `${label}を取り消し（${conv(o)}）`
    : `${label}を変更: ${conv(o)} → ${conv(n)}`;
  switch (e.kind) {
    case "created": return "登録";
    case "title": return `タイトル: ${o} → ${n}`;
    case "status": return `${STATUS_LABEL[o] ?? o} → ${STATUS_LABEL[n] ?? n}`;
    case "planned_start": return change("開始予定日", short_date);
    case "planned_end": return change("終了予定日", short_date);
    case "next_action": return change("次の予定", (v) => v.replace(/^\d{4}-\d{2}-\d{2}/, short_date));
    case "tags": return change("タグ");
    case "estimate": return change("見積もり", (v) => fmt_hours(Number(v)));
    case "jira": return change("Jira 課題");
    default: return `${e.kind}: ${o ?? ""} → ${n ?? ""}`;
  }
}

function setup_detail() {
  const tagInput = $("detail_tag_input");
  const addTag = () => {
    const task = state.byId.get(state.selectedId);
    const names = tagInput.value.split(/[,、]/).map((s) => s.trim()).filter(Boolean);
    tagInput.value = "";
    if (!task || !names.length) return;
    // 既存のタグと大文字小文字だけ違うときは既存の表記に合わせる
    const fixed = names.map((n) => state.tags.find((t) => same_tag(t.name, n))?.name ?? n);
    update(task.id, { tags: [...task.tags, ...fixed] });
  };
  tagInput.addEventListener("keydown", (ev) => {
    if (ev.isComposing) return;
    if (ev.key === "Enter" || ev.key === ",") {
      ev.preventDefault();
      addTag();
    } else if (ev.key === "Backspace" && tagInput.value === "") {
      const task = state.byId.get(state.selectedId);
      if (task && task.tags.length) update(task.id, { tags: task.tags.slice(0, -1) });
    }
  });
  // 候補リストから選んだときはそのまま追加する
  tagInput.addEventListener("input", (ev) => {
    if (ev.inputType === "insertReplacementText" || ev.inputType === undefined) {
      if (state.tags.some((t) => t.name === tagInput.value)) addTag();
    }
  });
  tagInput.addEventListener("blur", addTag);
  $("detail_tag_box").addEventListener("click", () => tagInput.focus());

  $("detail_close").addEventListener("click", () => select(null));
  $("detail_title").addEventListener("change", (ev) => {
    const v = ev.target.value.trim();
    if (v) update(state.selectedId, { title: v });
  });
  $("detail_status").addEventListener("change", (ev) => update(state.selectedId, { status: ev.target.value }));
  // 日付と次の予定は変更が確定したら保存（空にすると取り消し）
  for (const [id, field] of [
    ["detail_planned_start", "planned_start"],
    ["detail_planned_end", "planned_end"],
    ["detail_next_action_date", "next_action_date"],
    ["detail_next_action", "next_action"],
  ]) {
    $(id).addEventListener("change", (ev) => {
      const value = ev.target.value.trim();
      const task = state.byId.get(state.selectedId);
      if (task && (task[field] ?? "") !== value) {
        update(task.id, { [field]: value || null }).catch(() => {
          // 保存できなかった（開始と終了の逆転など）ときは元の値に戻す
          renderedDetailId = null;
          render_detail();
        });
      }
    });
  }
  $("detail_jira_key").addEventListener("change", (ev) => {
    const task = state.byId.get(state.selectedId);
    const v = ev.target.value.trim();
    if (task && (task.jira_key ?? "") !== v.toUpperCase()) {
      update(task.id, { jira_key: v || null }).catch(() => {
        renderedDetailId = null;
        render_detail();
      });
    }
  });
  $("detail_jira_key").addEventListener("keydown", (ev) => {
    if (ev.key === "Enter" && !ev.isComposing) ev.target.blur();
  });
  $("detail_estimate").addEventListener("change", (ev) => {
    const task = state.byId.get(state.selectedId);
    if (!task) return;
    const min = parse_effort(ev.target.value);
    if (Number.isNaN(min)) {
      toast("見積もりは 1d、4h、1.5d、30m のように入れてください", "error");
      renderedDetailId = null;
      render_detail();
      return;
    }
    if (min !== task.estimate_min) update(task.id, { estimate_min: min });
  });
  $("detail_estimate").addEventListener("keydown", (ev) => {
    if (ev.key === "Enter" && !ev.isComposing) ev.target.blur();
  });
  $("detail_next_action").addEventListener("keydown", (ev) => {
    if (ev.key === "Enter" && !ev.isComposing) ev.target.blur();
  });
  let noteTimer;
  const saveNote = () => {
    clearTimeout(noteTimer);
    const id = state.selectedId;
    const value = $("detail_note").value;
    if (id !== null && state.byId.has(id) && (state.byId.get(id).note || "") !== value) update(id, { note: value });
  };
  $("detail_note").addEventListener("input", () => {
    clearTimeout(noteTimer);
    noteTimer = setTimeout(saveNote, 800);
  });
  $("detail_note").addEventListener("blur", saveNote);
  $("detail_delete").addEventListener("click", () => remove(state.selectedId));
}

// ---------- 起動 ----------
function setup() {
  setup_date_inputs();
  if (!TABS.includes(state.tab)) state.tab = "gantt";
  for (const b of document.querySelectorAll(".tab")) b.addEventListener("click", () => switch_tab(b.dataset.tab));
  $("undo_button").addEventListener("click", () => undo_redo("undo"));
  $("redo_button").addEventListener("click", () => undo_redo("redo"));

  document.addEventListener("keydown", (ev) => {
    const tag = document.activeElement?.tagName;
    const typing = ["INPUT", "TEXTAREA", "SELECT"].includes(tag);
    const mod = ev.ctrlKey || ev.metaKey;
    // 入力欄の中では、文字の取り消しとしてブラウザに任せる
    if (mod && !typing && !ev.altKey) {
      const k = ev.key.toLowerCase();
      if (k === "z" && !ev.shiftKey) {
        ev.preventDefault();
        undo_redo("undo");
        return;
      }
      if (k === "y" || (k === "z" && ev.shiftKey)) {
        ev.preventDefault();
        undo_redo("redo");
        return;
      }
    }
    if (ev.key === "/" && !typing && !mod) {
      ev.preventDefault();
      begin_add("inbox");
    } else if (ev.key === "Escape" && !typing && state.selectedId !== null) {
      select(null);
    }
  });

  // 開いたまま日付が変わっても表示が古くならないように、画面に戻ったら読み直す
  document.addEventListener("visibilitychange", () => {
    if (document.visibilityState === "visible") reload();
  });

  setup_gantt();
  setup_list();
  setup_work();
  setup_jira();
  setup_databases();
  setup_alerts();
  load_jira_config().then(() => render_detail());
  setup_detail();
  reload();
}
