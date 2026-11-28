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
  dragTag: null, // 絞り込みのタグを運んでいるときのタグ名（タスクへ落とすと付ける）
  dragTagFrom: null, // タスクに付いているタグを運んでいるときの、そのタスクの id（ゴミ箱で外す）
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
  prune_tag_filters();
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
  const task = await api("POST", "/tasks", body);
  // 線表で絞り込み中に追加したものが条件に合わなくても、見えなくならないように出し続ける
  if (gantt_filter_active() && !gantt_matches(task)) {
    gantt.pinned.add(task.id);
    toast(`「${task.title}」は絞り込みの条件に合いませんが、追加したので表示しています（絞り込みを変えると見えなくなります）`);
  }
  return task;
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
  // 選んでいる行で、選択が 2 件以上なら選んだものをまとめて消す
  if (is_selected(id) && state.selected.size > 1) {
    await delete_tasks_confirm(visible_task_order().filter((x) => state.selected.has(x)));
    return;
  }
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

/// 複数のタスクを子孫ごとまとめて削除する（確認してから。Ctrl+Z 1 回で戻る）
async function delete_tasks_confirm(ids) {
  ids = ids.filter((id) => state.byId.has(id));
  if (!ids.length) return;
  if (ids.length === 1) return remove(ids[0]);
  // 選んだものの子孫は親と一緒に消えるので、親だけ数える
  const tops = ids.filter((id) => !ids.some((other) => other !== id && is_self_or_descendant(other, id)));
  const kids = tops.reduce((n, id) => n + count_descendants(id), 0);
  const msg = `🗑️ ${tops.length} 件のタスク${kids ? `（と子タスク ${kids} 件）` : ""}を削除しますか？\n（削除したあとでも Ctrl+Z で元に戻せます）`;
  if (!confirm(msg)) return;
  await api("POST", "/tasks/delete_many", { ids });
  state.selected = new Set();
  state.selectedId = null;
  await reload();
  toast(`🗑️ ${tops.length} 件を削除しました（Ctrl+Z で元に戻す）`);
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
/// タスクの行に付いているタグ。taskId を渡すと、ドラッグしてゴミ箱に入れるとそのタスクから外せる
/// （ほかのタスクの行に落とすと、そのタスクにも付く）
function tag_chips(tags, active, onToggle, taskId = null) {
  return tags.map((name) => {
    const chip = el("span", {
      class: "tag_chip" + (active(name) ? " active" : ""),
      style: tag_style(name),
      title: "クリックでこのタグに絞り込む・ダブルクリックでタグの名前を変える（すべてのタスクで）" +
        (taskId === null ? "" : "・ドラッグしてゴミ箱に入れるとこのタスクから外す"),
      onclick: (ev) => {
        ev.stopPropagation();
        tag_click(name, onToggle);
      },
    }, name);
    if (taskId !== null) {
      chip.draggable = true;
      chip.addEventListener("dragstart", (ev) => {
        ev.stopPropagation(); // 行（名前）のドラッグにしない
        ev.dataTransfer.setData(TAG_DRAG_TYPE, name);
        ev.dataTransfer.effectAllowed = "copyMove";
        state.dragTag = name;
        state.dragTagFrom = taskId;
        document.body.classList.add("tag_dragging");
        const row = chip.closest("[data-task_id]");
        if (state.tab === "gantt" && row) show_trash("tag", { row: row.getBoundingClientRect(), x: ev.clientX, y: ev.clientY });
      });
      chip.addEventListener("dragend", end_tag_drag);
    }
    return chip;
  });
}

/// タグのクリック：1 回なら絞り込み、すばやく 2 回なら名前の変更。
/// 1 回目ですぐ絞り込むと行が描き直されて 2 回目が別の所に当たるので、少し待ってから絞り込む
const TAG_DBLCLICK_MS = 250;
let tagClick = { name: null, timer: 0 };
function tag_click(name, onToggle) {
  if (tagClick.name === name && tagClick.timer) {
    clearTimeout(tagClick.timer);
    tagClick = { name: null, timer: 0 };
    rename_tag(name);
    return;
  }
  clearTimeout(tagClick.timer);
  tagClick = {
    name,
    timer: setTimeout(() => {
      tagClick = { name: null, timer: 0 };
      onToggle(name);
    }, TAG_DBLCLICK_MS),
  };
}

/// タグの名前を変える（付いているすべてのタスクで。同じ名前のタグがあればまとめる）
async function rename_tag(name) {
  const to = prompt(`タグ「${name}」の新しい名前（付いているすべてのタスクで変わります。すでにある名前ならまとめます）`, name);
  if (to === null || !to.trim() || to.trim() === name) return;
  const r = await api("POST", "/tags/rename", { from: name, to });
  // 絞り込みで選んでいたら、新しい名前に置き換える
  for (const f of [gantt.filter, list.filter]) {
    if (f.tags.some((n) => same_tag(n, name))) {
      f.tags = f.tags.filter((n) => !same_tag(n, name) && !same_tag(n, to.trim()));
      f.tags.push(to.trim().replace(/^#/, ""));
    }
  }
  save_json("gantt_filter", gantt.filter);
  save_json("list_filter", list.filter);
  await reload();
  toast(`タグ「${name}」を「${to.trim()}」に変えました（${r.changed} 件。Ctrl+Z で元に戻す）`);
}

/// F2：選んでいるものの名前を変える（タグにフォーカスがあればタグ、関連資料ならその名前、それ以外は選んでいるタスク）
function rename_by_f2() {
  const focused = document.activeElement;
  const tagName = focused?.dataset?.tag_name;
  if (tagName) return rename_tag(tagName);
  if (focused?.classList?.contains("link_title")) {
    focused.dispatchEvent(new MouseEvent("dblclick", { bubbles: true }));
    return;
  }
  const id = state.selectedId;
  const task = id === null ? null : state.byId.get(id);
  if (!task) return;
  const label = document.querySelector(`#view_${state.tab} [data-task_id="${id}"] .label, #view_${state.tab} [data-task_id="${id}"] .title`);
  if (label) start_inline_edit(label, task);
  else {
    $("detail_title").focus();
    $("detail_title").select();
  }
}

/// 絞り込みに直接並べるタグの数（多く使われている順）。残りは「ほかのタグ」から選ぶ
const TAG_FILTER_SHOW = 8;

/// 関連資料があるタスクの印（🔗、2 件以上なら件数）。押すと 1 件ならそのリンクを開き、2 件以上なら詳細を開く
function link_mark(t) {
  if (!t.links?.length) return [];
  const n = t.links.length;
  return [el("span", {
    class: "link_mark",
    title: "関連資料：\n" + t.links.map((l) => `・${l.title}`).join("\n") + (n === 1 ? "\n（クリックで開く）" : "\n（クリックで詳細に一覧）"),
    onclick: (ev) => {
      ev.stopPropagation();
      if (n === 1) window.open(t.links[0].url, "_blank", "noopener,noreferrer");
      else select(t.id, ev);
    },
  }, n === 1 ? "🔗" : `🔗${n}`)];
}

function tag_filter_chip(t, selected, onToggle) {
  const on = selected.some((n) => same_tag(n, t.name));
  const chip = el("button", {
    type: "button",
    class: "tag_chip" + (on ? " active" : ""),
    style: tag_style(t.name),
    draggable: "true",
    title: "クリックで絞り込み・タスクの行へドラッグでそのタグを付ける・ダブルクリック（か F2）でタグの名前を変える",
    "data-tag_name": t.name,
    onclick: () => tag_click(t.name, onToggle),
  }, t.name, el("span", { class: "count" }, String(t.count)));
  attach_tag_drag(chip, t.name);
  return chip;
}

// ---------- タグをタスクへドラッグして付ける ----------
const TAG_DRAG_TYPE = "application/x-my-todo-tag";

function attach_tag_drag(chip, name) {
  chip.addEventListener("dragstart", (ev) => {
    ev.dataTransfer.setData(TAG_DRAG_TYPE, name);
    ev.dataTransfer.effectAllowed = "copy";
    state.dragTag = name;
    document.body.classList.add("tag_dragging");
    close_tag_picker();
  });
  chip.addEventListener("dragend", end_tag_drag);
}

function end_tag_drag() {
  if (state.dragTagFrom !== null) show_trash(null);
  state.dragTagFrom = null;
  state.dragTag = null;
  document.body.classList.remove("tag_dragging");
  document.querySelectorAll(".tag_drop_target").forEach((r) => r.classList.remove("tag_drop_target"));
}

/// タグを運んでいる間は、タスクの行（線表・未分類・詳細・工数記録）を落とし先にする。
/// 名前のドラッグ（並べ替え）の処理より先に受け取って、そちらには渡さない
function setup_tag_drop() {
  const rowAt = (ev) => (state.dragTag ? ev.target.closest?.("[data-task_id]") : null);
  const overTrash = (ev) => state.dragTagFrom !== null && ev.target instanceof Element && ev.target.closest("#gantt_trash");
  document.addEventListener("dragover", (ev) => {
    if (!state.dragTag) return;
    $("gantt_trash").classList.toggle("over", Boolean(overTrash(ev)));
    if (overTrash(ev)) {
      // ゴミ箱の上では、下に重なっている行の「ここに付ける」の枠は出さない（外すのか付けるのか迷わないように）
      document.querySelectorAll(".tag_drop_target").forEach((r) => r.classList.remove("tag_drop_target"));
      ev.preventDefault();
      ev.stopPropagation();
      ev.dataTransfer.dropEffect = "move";
      return;
    }
    const row = rowAt(ev);
    document.querySelectorAll(".tag_drop_target").forEach((r) => { if (r !== row) r.classList.remove("tag_drop_target"); });
    ev.stopPropagation();
    if (!row) return;
    ev.preventDefault();
    ev.dataTransfer.dropEffect = "copy";
    row.classList.add("tag_drop_target");
  }, true);
  document.addEventListener("drop", async (ev) => {
    if (!state.dragTag) return;
    if (overTrash(ev)) {
      // タスクに付いているタグをゴミ箱へ：そのタスクから外す（選んでいる行のタグなら、選んでいるタスクすべてから）
      ev.preventDefault();
      ev.stopPropagation();
      const tag = state.dragTag, from = state.dragTagFrom;
      end_tag_drag();
      const ids = state.selected.has(from) && state.selected.size > 1
        ? visible_task_order().filter((x) => state.selected.has(x))
        : [from];
      const r = await api("POST", "/tasks/remove_tag", { ids, tag });
      await reload();
      toast(`タグ「${tag}」を ${r.changed} 件から外しました（Ctrl+Z で元に戻す）`);
      return;
    }
    const tag = state.dragTag;
    const row = rowAt(ev);
    ev.stopPropagation();
    end_tag_drag();
    if (!row) return;
    ev.preventDefault();
    const id = Number(row.dataset.task_id);
    // 選んでいる行に落としたら、選んでいるタスクすべてに付ける
    const ids = state.selected.has(id) && state.selected.size > 1
      ? visible_task_order().filter((x) => state.selected.has(x))
      : [id];
    const r = await api("POST", "/tasks/add_tag", { ids, tag });
    await reload();
    toast(r.changed
      ? `タグ「${tag}」を ${r.changed} 件に付けました（Ctrl+Z で元に戻す）`
      : `タグ「${tag}」はもう付いています`);
  }, true);
}

/// タグの絞り込みボタン（件数付き）。よく使うタグを並べ、選んでいるタグは必ず出す。
/// 並びきらない分は「ほかのタグ」で、名前で探して選べる
/// getSelected は最新の選択（配列）を返す関数（絞り込みの条件オブジェクトは差し替わることがあるため）
function tag_filter_buttons(getSelected, onToggle) {
  const selected = getSelected();
  const byUse = [...state.tags].sort((a, b) => b.count - a.count || a.name.localeCompare(b.name, "ja"));
  const shown = byUse.filter((t, i) => i < TAG_FILTER_SHOW || selected.some((n) => same_tag(n, t.name)));
  const rest = state.tags.length - shown.length;
  const chips = shown.map((t) => tag_filter_chip(t, selected, onToggle));
  if (state.tags.length > TAG_FILTER_SHOW) {
    chips.push(el("button", {
      type: "button",
      class: "chip tag_more",
      title: "すべてのタグから名前で探して選ぶ",
      onclick: (ev) => { ev.stopPropagation(); open_tag_picker(ev.currentTarget, getSelected, onToggle); },
    }, rest > 0 ? `ほかのタグ（${rest}）…` : "タグを探す…"));
  }
  return chips;
}

// ---------- タグを探して選ぶ小窓 ----------
const tagPicker = { el: null, getSelected: null, onToggle: null };

function open_tag_picker(anchor, getSelected, onToggle) {
  if (!tagPicker.el) {
    tagPicker.el = el("div", { class: "tag_picker", hidden: true });
    tagPicker.el.addEventListener("click", (ev) => ev.stopPropagation());
    document.body.append(tagPicker.el);
    document.addEventListener("click", close_tag_picker);
    document.addEventListener("keydown", (ev) => { if (ev.key === "Escape") close_tag_picker(); });
  }
  tagPicker.getSelected = getSelected;
  tagPicker.onToggle = onToggle;
  const input = el("input", { type: "search", class: "tag_picker_search", placeholder: "タグの名前で探す", autocomplete: "off" });
  const listEl = el("div", { class: "tag_picker_list" });
  input.addEventListener("input", () => draw_tag_picker_list(input, listEl));
  input.addEventListener("keydown", (ev) => {
    // Enter で、探した結果の先頭を選ぶ
    if (ev.key === "Enter" && !ev.isComposing) listEl.querySelector("button")?.click();
  });
  tagPicker.el.replaceChildren(input, listEl);
  draw_tag_picker_list(input, listEl);
  tagPicker.el.hidden = false;
  const r = anchor.getBoundingClientRect();
  tagPicker.el.style.top = `${Math.min(r.bottom + 4, window.innerHeight - 340)}px`;
  tagPicker.el.style.left = `${Math.max(16, Math.min(r.left, window.innerWidth - tagPicker.el.offsetWidth - 16))}px`;
  input.focus();
}

function draw_tag_picker_list(input, listEl) {
  const q = input.value.trim().toLowerCase().replace(/^#/, "");
  const selected = tagPicker.getSelected();
  const hits = state.tags.filter((t) => !q || t.name.toLowerCase().includes(q));
  listEl.replaceChildren(...(hits.length
    ? hits.map((t) => {
      const chip = tag_filter_chip(t, selected, (name) => {
        tagPicker.onToggle(name);
        draw_tag_picker_list(input, listEl); // 選んだ状態を描き直す（小窓は開いたまま）
      });
      return chip;
    })
    : [el("p", { class: "empty_hint" }, "そのタグはありません")]));
}

function close_tag_picker() {
  if (tagPicker.el) tagPicker.el.hidden = true;
}

/// 絞り込みで選んでいるタグのうち、もうどのタスクにも付いていないものを外す
/// （外さないと、ボタンは消えたのに絞り込みだけ残り、タスクが全部隠れてしまう）
function prune_tag_filters() {
  const exists = (n) => state.tags.some((t) => same_tag(t.name, n));
  const prune = (f, save) => {
    const kept = f.tags.filter(exists);
    if (kept.length !== f.tags.length) {
      f.tags = kept;
      save();
    }
  };
  prune(gantt.filter, save_gantt_filter);
  prune(list.filter, save_list_filter);
}

function toggle_in(list, name) {
  const i = list.findIndex((t) => same_tag(t, name));
  if (i >= 0) list.splice(i, 1);
  else list.push(name);
}

/// タイトルをその場で編集する
function start_inline_edit(titleEl, task, onDone) {
  if (task.status === "done") {
    toast("完了したタスクの名前は変えられません（未完了に戻すと変えられる）", "error");
    return;
  }
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
// ---------- 見積もりと負荷 ----------
/// 期間の平日の数（両端を含む）。土日だけの期間なら暦日の数
function weekdays_in(start, end) {
  let n = 0, all = 0;
  for (let d = start; d <= end; d = add_days(d, 1)) {
    all++;
    const wd = weekday_of(d);
    if (wd !== 0 && wd !== 6) n++;
    if (all > 3660) break;
  }
  return n || all;
}

/// タスクの見積もり（分）：自分の見積もり、なければ子の見積もりの合計（子にもなければ null）
function task_estimate(t) {
  if (t.estimate_min) return t.estimate_min;
  let sum = 0, any = false;
  for (const c of children_of(t.id)) {
    const e = task_estimate(c);
    if (e) { sum += e; any = true; }
  }
  return any ? sum : null;
}

/// 子孫のどれかに自分の見積もりがあるか（あれば、負荷は子の方で数える）
function has_descendant_estimate(t) {
  return children_of(t.id).some((c) => c.estimate_min || has_descendant_estimate(c));
}

/// 1 日に使える時間（分）：工数記録の設定の「1D」
function day_capacity() {
  return minutes_per_day();
}

/// 負荷：見積もりを期間の平日に均した 1 日あたりの時間。期間がなければ null
function task_load(t) {
  const est = task_estimate(t);
  const { start, end } = span_of(t);
  const s = start || end, e = end || start;
  if (!est || !s || !e || s > e) return null;
  const days = weekdays_in(s, e);
  return { est, days, perDay: est / days, start: s, end: e };
}

/// 見積もりが期間に収まらない（平日 × 1D を超える）か
function too_heavy(t) {
  const l = task_load(t);
  return Boolean(l && t.status !== "done" && l.perDay > day_capacity());
}

/// 見積もりの入っていない、線表に日程のある自分の未完了タスクか（子に見積もりがあれば数えない）
function needs_estimate(t) {
  if (t.status === "done" || t.kind === "external" || (t.parent_id === null && t.is_inbox)) return false;
  if (!t.planned_start && !t.planned_end) return false;
  return !task_estimate(t);
}

/// 詰め込みすぎの期間：その期間の中に期間がまるごと収まるタスクの残りの見積もりの合計が、
/// 「期間の平日 × 1D」を超えるもの。作業を均等に割らず、各タスクの期間の中ならいつやってもよいと考えたうえで、
/// どう割り振っても無理なときだけ知らせる。今日から先だけ（始まっているタスクは今日から、残り＝見積もり − 記録した工数）。
/// 同じ理由で外側の期間も無理になるので、中にほかの無理な期間を含まない、いちばん狭いものだけを返す。
/// 1 件だけで無理なもの（期間に収まらない）は too_heavy で知らせるので除く
function overloaded_periods() {
  const today = today_text();
  const items = [];
  for (const t of state.tasks) {
    if (t.status === "done" || t.kind === "external" || !t.estimate_min || has_descendant_estimate(t)) continue;
    const l = task_load(t);
    if (!l || l.end < today) continue;
    const remaining = Math.max(0, t.estimate_min - (t.actual_min || 0));
    if (!remaining) continue;
    items.push({ t, start: l.start > today ? l.start : today, end: l.end, remaining });
  }
  const starts = [...new Set(items.map((x) => x.start))].sort();
  const ends = [...new Set(items.map((x) => x.end))].sort();
  const bad = [];
  for (const a of starts) {
    for (const b of ends) {
      if (b < a) continue;
      const inside = items.filter((x) => x.start >= a && x.end <= b);
      if (inside.length < 2) continue;
      const total = inside.reduce((n, x) => n + x.remaining, 0);
      const cap = weekdays_in(a, b) * day_capacity();
      if (total > cap + 1) bad.push({ start: a, end: b, total, cap, tasks: inside.map((x) => x.t) });
    }
  }
  return bad.filter((p) => !bad.some((q) => q !== p && q.start >= p.start && q.end <= p.end && (q.start > p.start || q.end < p.end)))
    .sort((p, q) => (p.start < q.start ? -1 : p.start > q.start ? 1 : 0));
}

/// バーの太さの段階：見積もりの大きさ（2h 以下は細い、1D 以下は普通、それより大きいと太い）。見積もりがなければ "unknown"
function load_level(t) {
  // 他の人の作業は見積もりを求めない（いつまでに終わるかだけを見る）ので、太さは普通・点線にもしない
  if (t.kind === "external") return "other";
  const est = task_estimate(t);
  if (!est) return "unknown";
  return est <= 120 ? "light" : est <= day_capacity() ? "medium" : "heavy";
}

// ---------- 依存関係 ----------
/// 依存関係で使う日付：始まり（なければ終わり）と終わり（なければ始まり）。親で自分の日付がなければ子の範囲
function dep_dates(t) {
  let { start, end } = span_of(t);
  if (!start && !end) {
    const sub = subtree_span(t);
    if (sub) ({ start, end } = sub);
  }
  return { start: start || end, end: end || start };
}

/// 順番が逆（先のタスクが終わる予定より前に、後のタスクを始める予定）になっているか。
/// どちらかが完了していれば、もう待つものはないので問題にしない
function dependency_broken(pre, post) {
  if (pre.status === "done" || post.status === "done") return false;
  const a = dep_dates(pre), b = dep_dates(post);
  return Boolean(a.end && b.start && b.start < a.end);
}

/// すべての依存関係 [{pre, post}]
function all_dependencies() {
  const out = [];
  for (const post of state.tasks) {
    for (const preId of post.depends_on || []) {
      const pre = state.byId.get(preId);
      if (pre) out.push({ pre, post });
    }
  }
  return out;
}

async function add_dependency(preId, postId) {
  await api("POST", "/dependencies", { pre_id: preId, post_id: postId });
  await reload();
  toast(`「${state.byId.get(preId)?.title}」→「${state.byId.get(postId)?.title}」をつなぎました（Ctrl+Z で元に戻す）`);
}

async function remove_dependency(preId, postId) {
  await api("DELETE", "/dependencies", { pre_id: preId, post_id: postId });
  await reload();
  toast("つながりを外しました（Ctrl+Z で元に戻す）");
}

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
  $("review_pane").hidden = state.tab !== "review";
  // タブの描画で例外が起きても、詳細パネル（選択の解除・消したタスクを閉じる）は必ず描き直す
  try {
    render_alerts();
    if (state.tab === "gantt") render_gantt();
    else if (state.tab === "list") render_list();
    else if (state.tab === "work") render_work();
    else if (state.tab === "review") render_review();
    else if (state.tab === "jira") render_jira();
    else render_help();
    mark_cut_rows();
  } catch (e) {
    console.error(e);
    toast(`画面の描画でエラーが起きました：${e.message}`, "error");
  }
  render_detail();
}

const TABS = ["gantt", "list", "work", "review", "jira", "help"];

function switch_tab(tab) {
  if (state.tab === tab) return;
  state.tab = tab;
  save_json("tab", tab);
  pendingAdd = null;
  render();
  window.scrollTo(0, 0);
}

// ---------- 連続追加 ----------
let pendingAdd = null; // { key, draft }：親の id / "root" / "inbox" と、打ちかけの文字

function begin_add(key) {
  if (state.tab !== "gantt") switch_tab("gantt");
  pendingAdd = { key, draft: "" };
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
  // 描き直し（追加のあとの読み直し・別の画面から戻ったときの読み直し）で欄が作り直されても、
  // 打ちかけの文字を引き継ぐ
  input.value = pending.draft || "";
  input.addEventListener("input", () => { pending.draft = input.value; });
  input.addEventListener("keydown", async (ev) => {
    if (ev.key === "Escape") {
      pendingAdd = null;
      render();
    } else if (ev.key === "Enter" && !ev.isComposing) {
      const title = input.value.trim();
      if (!title) return;
      input.value = "";
      pending.draft = "";
      await create(title, pending.key);
      await reload();
      const next = document.querySelector("[data-pending_add]");
      if (next && document.activeElement !== next) {
        next.focus();
        next.setSelectionRange(next.value.length, next.value.length);
      }
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
/// 行のどこに落とすか：上の 1/3 は前、下の 1/3 は後ろ、真ん中は子にする
function drop_zone(rowEl, ev) {
  const rect = rowEl.getBoundingClientRect();
  const y = (ev.clientY - rect.top) / rect.height;
  if (y < 1 / 3) return "before";
  if (y > 2 / 3) return "after";
  return "into";
}

let dropMark = { el: null, cls: null }; // いま印を付けている行と印（同じなら付け直さない＝ちらつかない）

function set_drop_mark(el, cls) {
  if (dropMark.el === el && dropMark.cls === cls) return;
  clear_drop_marks();
  if (el) el.classList.add(cls);
  dropMark = { el, cls };
}

function clear_drop_marks() {
  document.querySelectorAll(".drop_before, .drop_after, .drop_into").forEach((e) =>
    e.classList.remove("drop_before", "drop_after", "drop_into"));
  dropMark = { el: null, cls: null };
}

/// 開いている親の「下」に落としたときは、線が親と最初の子の間に出るので、その見た目どおり最初の子の前に入れる
function resolve_drop(task, zone) {
  if (zone !== "after" || state.collapsed.has(task.id)) return { zone, refId: task.id };
  const kids = children_of(task.id).filter((c) => state.tab !== "gantt" || gantt_visible(c));
  return kids.length ? { zone: "before", refId: kids[0].id } : { zone, refId: task.id };
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
    // 選んでいない行をつかんだら、その行を選ぶ（前に選んでいた行が選ばれたままに見えないように）
    if (!is_selected(task.id)) select(task.id);
    // 選んでいる行をつかんだら、選んでいるものをすべて（画面の並び順で）動かす
    const ids = is_selected(task.id) && state.selected.size > 1
      ? visible_task_order().filter((id) => state.selected.has(id))
      : [task.id];
    state.dragId = task.id;
    state.dragIds = ids;
    ev.dataTransfer.effectAllowed = "move";
    ev.dataTransfer.setData("text/plain", ids.join(","));
    show_trash("task", { row: handle.getBoundingClientRect(), x: ev.clientX, y: ev.clientY });
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
    show_trash(null);
  });
  // 行の中の名前・チェック・タグなどの間を動くたびに印を消して付け直すとちらつくので、
  // dragleave では消さず、ほかの行に移ったとき・行の外に出たとき（setup の dragover）に消す
  rowEl.addEventListener("dragover", (ev) => {
    if (state.dragId === null || dragging_onto_self(task.id)) return;
    ev.preventDefault();
    set_drop_mark(rowEl, "drop_" + drop_zone(rowEl, ev));
  });
  rowEl.addEventListener("drop", async (ev) => {
    ev.preventDefault();
    const id = state.dragId;
    const ids = state.dragIds || [id];
    clear_drop_marks();
    if (id === null || dragging_onto_self(task.id)) return;
    const { zone, refId } = resolve_drop(task, drop_zone(rowEl, ev));
    if (zone !== "into" && dragging_onto_self(refId)) return;
    if (ids.length > 1) {
      if (zone === "into") expand(task.id);
      await move_many(ids, zone === "into" ? { kind: "into", parent_id: task.id, inbox: false } : { kind: zone, ref_id: refId });
    } else if (zone === "into") {
      expand(task.id);
      await move(id, task.id, null);
    } else {
      await move_relative(id, refId, zone);
    }
  });
}

/// グループ見出し（タスク / 未分類）に落とすと、その最上位の末尾へ
function attach_group_drop(rowEl, key) {
  rowEl.addEventListener("dragover", (ev) => {
    if (state.dragId === null) return;
    ev.preventDefault();
    set_drop_mark(rowEl, "drop_into");
  });
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

/// 依存関係の欄：先に終わらせるタスク（× で外す）と、このタスクの後に続くタスク
function render_detail_deps(task) {
  const item = (other, onRemove, broken, locked = false) => el("li", { class: broken ? "broken" : "" },
    el("a", { href: "#", class: "link_title", title: "クリックでそのタスクを選ぶ", onclick: (ev) => { ev.preventDefault(); select(other.id); } },
      other.title + (broken ? "（順番が逆）" : "")),
    el("button", {
      type: "button",
      class: "icon_button danger",
      disabled: locked,
      title: locked ? "完了したタスクなので外せません（状態を未完了に戻すと外せる）" : "このつながりを外す",
      onclick: onRemove,
    }, "×"));
  const pres = (task.depends_on || []).map((id) => state.byId.get(id)).filter(Boolean);
  $("detail_deps_pre").replaceChildren(...pres.map((p) => item(p, () => remove_dependency(p.id, task.id), dependency_broken(p, task), task.status === "done")));
  const posts = state.tasks.filter((t) => (t.depends_on || []).includes(task.id));
  $("detail_deps_post").replaceChildren(...(posts.length
    ? [el("span", { class: "field_label" }, "このタスクの後に続くタスク"),
      el("ul", { class: "link_list" }, ...posts.map((p) => item(p, () => remove_dependency(task.id, p.id), dependency_broken(task, p))))]
    : []));
  const taken = new Set([task.id, ...pres.map((p) => p.id)]);
  $("dep_options").replaceChildren(...state.tasks.filter((t) => !taken.has(t.id)).map((t) => el("option", { value: task_option_label(t) })));
}

/// 関連資料の一覧。名前をクリックで開く（新しいタブ）、ダブルクリックで名前を変える、× で外す
function render_detail_links(task) {
  const listEl = $("detail_links");
  // 名前の変更中は描き直さない
  if (listEl.querySelector("input")) return;
  listEl.replaceChildren(...task.links.map((l) => {
    const a = el("a", { href: l.url, target: "_blank", rel: "noopener noreferrer", class: "link_title", title: `${l.url}\n（名前は ✎・ダブルクリック・F2 で変えられる）` }, l.title);
    a.addEventListener("dblclick", (ev) => {
      ev.preventDefault();
      const input = el("input", { type: "text", class: "link_rename" });
      input.value = l.title;
      let done = false;
      const finish = async (save) => {
        if (done) return;
        done = true;
        if (save && input.value.trim() !== l.title) await api("PATCH", `/links/${l.id}`, { title: input.value });
        input.remove();
        await reload();
      };
      input.addEventListener("keydown", (e) => {
        if (e.key === "Enter" && !e.isComposing) finish(true);
        if (e.key === "Escape") finish(false);
      });
      input.addEventListener("blur", () => finish(true));
      a.replaceWith(input);
      input.focus();
      input.select();
    });
    return el("li", {},
      a,
      el("button", {
        type: "button",
        class: "icon_button",
        title: "名前を変える（ダブルクリック・F2 でも）",
        onclick: () => a.dispatchEvent(new MouseEvent("dblclick", { bubbles: true })),
      }, "✎"),
      el("button", {
        type: "button",
        class: "icon_button danger",
        title: "この関連資料を外す",
        onclick: async () => {
          await api("DELETE", `/links/${l.id}`);
          await reload();
          toast(`「${l.title}」を外しました（Ctrl+Z で元に戻す）`);
        },
      }, "×"));
  }));
}

let detailMoreTab = null; // 「その他」の開閉を最後に決めたときのタブ


function render_detail() {
  const panel = $("detail");
  const task = state.selectedId === null ? null : state.byId.get(state.selectedId);
  panel.querySelector(".detail_header h2").textContent = state.selected.size > 1 ? `詳細（${state.selected.size} 件選択中）` : "詳細";
  // 詳細の欄は、タスクを選べるタブでは最初から場所を取っておく（選んだときに左の配置がずれないように）
  panel.hidden = !["gantt", "list", "work"].includes(state.tab);
  $("detail_body").hidden = !task;
  $("detail_empty").hidden = Boolean(task);
  $("detail_close").hidden = !task;
  if (!task) {
    renderedDetailId = null;
    return;
  }
  // 入力中の欄は上書きしない
  const active = document.activeElement;
  const fill = (id, value) => {
    if (renderedDetailId !== task.id || active !== $(id)) $(id).value = value ?? "";
  };
  // 完了したタスクは、名前・日程・見積もり・担当を変えられない（サーバーでも止めている）
  const locked = task.status === "done";
  // 操作できない欄には、カーソルを合わせると理由が出るようにする（無効の欄は自分では反応しないので、囲みにも付ける）
  const why = locked ? "完了したタスクなので変えられません（状態を未完了に戻すと変えられる）" : "";
  for (const id of ["detail_title", "detail_planned_start", "detail_planned_end", "detail_estimate", "detail_kind",
    "detail_next_action_date", "detail_next_action", "detail_dep_input"]) {
    const input = $(id);
    input.disabled = locked;
    if (locked) input.dataset.lockedTitle = "1", input.title = why;
    else if (input.dataset.lockedTitle) { delete input.dataset.lockedTitle; input.removeAttribute("title"); }
    const box = input.closest(".field, .next_action, .links_box");
    if (box) box.title = why;
  }
  // 他の人の作業は見積もりを求めないので、見積もりの欄を出さない
  document.querySelector("#detail .effort_box").hidden = task.kind === "external";
  $("detail_locked").hidden = !locked;
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
  // 「その他」：タブを切り替えたときだけ開閉を決める（詳細タブでは開く、ほかでは畳む）。中身は畳んでいても見出しに要約を出す。
  // 開始・終了予定日は、線表ではバーで決めるので「その他」に、詳細タブでは上に置く
  if (detailMoreTab !== state.tab) {
    detailMoreTab = state.tab;
    $("detail_more").open = state.tab === "list";
    (state.tab === "gantt" ? $("detail_dates_more") : $("detail_dates_main")).append($("detail_dates"));
  }
  const more = [];
  if (state.tab === "gantt" && (task.planned_start || task.planned_end)) {
    more.push(`${task.planned_start ? short_date(task.planned_start) : ""}〜${task.planned_end ? short_date(task.planned_end) : ""}`);
  }
  if (task.jira_key) more.push(task.jira_key);
  $("detail_more_summary").textContent = more.length ? `：${more.join("・")}` : state.tab === "gantt" ? "（日程・Jira 課題）" : "（Jira 課題）";

  render_detail_links(task);
  render_detail_deps(task);
  $("detail_kind").value = task.kind || "self";
  $("detail_tags").replaceChildren(...task.tags.map((name) => el("span", {
    class: "tag_chip",
    style: tag_style(name),
    title: "ダブルクリックでタグの名前を変える（すべてのタスクで）",
    ondblclick: () => rename_tag(name),
  },
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

  // 場所だけを出す（作った日・着手・完了は「変更の履歴」に出るので、ここでは重ねない）
  const path = path_titles(task);
  // 場所と、記録した工数（結果）。記録がなければ工数の行は出さない
  const meta = [`場所：${path.length ? path.join(" › ") : task.is_inbox ? "未分類" : "線表の最上位"}`];
  if (actual) {
    const est = task_estimate(task);
    meta.push(`記録した工数：${fmt_effort(actual)}${actual !== task.actual_min ? "（子を含む）" : ""}` +
      (est ? `・見積もりの ${Math.round((actual / est) * 100)}%` : ""));
  }
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
  const list = events.reverse(); // 新しい順
  $("detail_history").replaceChildren(
    ...list.map((e) => el("li", {}, el("time", {}, fmt(e.at)), describe_event(e))),
  );
  // 閉じたままでも、最後にいつ何をしたかがわかるように見出しに出す
  const last = list[0];
  $("detail_history_summary").textContent = last
    ? `変更の履歴（${list.length} 件。最後：${fmt(last.at)} ${describe_event(last)}）`
    : "変更の履歴";
}

function describe_event(e) {
  const o = e.old_value, n = e.new_value;
  const change = (label, conv = (v) => v) =>
    o === null ? `${label}を設定: ${conv(n)}`
    : n === null ? `${label}を取り消し（${conv(o)}）`
    : `${label}を変更: ${conv(o)} → ${conv(n)}`;
  switch (e.kind) {
    case "created": return "作成";
    case "title": return `タイトルを変更: ${o} → ${n}`;
    case "status": return `状態を変更: ${STATUS_LABEL[o] ?? o} → ${STATUS_LABEL[n] ?? n}`;
    case "planned_start": return change("開始予定日", short_date);
    case "planned_end": return change("終了予定日", short_date);
    case "next_action": return change("次の予定", (v) => v.replace(/^\d{4}-\d{2}-\d{2}/, short_date));
    case "tags": return change("タグ");
    case "estimate": return change("見積もり", (v) => fmt_hours(Number(v)));
    case "jira": return change("Jira 課題");
    case "kind": return `担当を変更: ${o === "external" ? "他の人" : "自分"} → ${n === "external" ? "他の人" : "自分"}`;
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
    noteTimer = null;
    const id = state.selectedId;
    const value = $("detail_note").value;
    if (id !== null && state.byId.has(id) && (state.byId.get(id).note || "") !== value) update(id, { note: value });
  };
  $("detail_note").addEventListener("input", () => {
    clearTimeout(noteTimer);
    noteTimer = setTimeout(saveNote, 800);
  });
  $("detail_note").addEventListener("blur", saveNote);
  // 関連資料：URL を貼り付けたらすぐ足す（Enter でも足せる）。名前はサーバーがリンク先のページから取る
  const linkInput = $("detail_link_input");
  const addLink = async () => {
    const url = linkInput.value.trim();
    const id = state.selectedId;
    if (!url || id === null) return;
    linkInput.disabled = true;
    try {
      await api("POST", `/tasks/${id}/links`, { url });
      linkInput.value = "";
      await reload();
    } finally {
      linkInput.disabled = false;
    }
  };
  linkInput.addEventListener("keydown", (ev) => {
    if (ev.key === "Enter" && !ev.isComposing) addLink();
  });
  linkInput.addEventListener("paste", () => setTimeout(() => {
    if (/^https?:\/\/\S+$/.test(linkInput.value.trim())) addLink();
  }, 0));
  // 打ってすぐ再読み込み・タブを閉じたときも、メモを失わないよう送っておく（keepalive でページが閉じても届く）
  window.addEventListener("pagehide", () => {
    if (!noteTimer) return;
    clearTimeout(noteTimer);
    noteTimer = null;
    const id = state.selectedId;
    const value = $("detail_note").value;
    if (id === null || !state.byId.has(id) || (state.byId.get(id).note || "") === value) return;
    fetch(`/api/tasks/${id}`, {
      method: "PATCH",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ note: value }),
      keepalive: true,
    });
  });
  $("detail_delete").addEventListener("click", () => remove(state.selectedId));
  $("detail_kind").addEventListener("change", (ev) => { if (state.selectedId !== null) update(state.selectedId, { kind: ev.target.value }); });
  $("detail_dep_input").addEventListener("keydown", (ev) => {
    if (ev.key !== "Enter" || ev.isComposing) return;
    const label = ev.target.value.trim();
    const pre = state.tasks.find((t) => task_option_label(t) === label) || state.tasks.find((t) => t.title === label);
    if (!pre) { toast("そのタスクは見つかりません", "error"); return; }
    ev.target.value = "";
    add_dependency(pre.id, state.selectedId);
  });
}

/// 文字を入力する欄か（チェックボックスや選択肢にフォーカスがあるときは、アプリのショートカットを効かせる）
function is_text_entry(node) {
  if (!node) return false;
  if (node.tagName === "TEXTAREA" || node.isContentEditable) return true;
  if (node.tagName !== "INPUT") return false;
  return !["checkbox", "radio", "button", "submit", "reset", "range", "color", "file"].includes(node.type);
}

// ---------- 起動 ----------
function setup() {
  setup_date_inputs();
  if (!TABS.includes(state.tab)) state.tab = "gantt";
  for (const b of document.querySelectorAll(".tab")) b.addEventListener("click", () => switch_tab(b.dataset.tab));
  $("undo_button").addEventListener("click", () => undo_redo("undo"));
  $("redo_button").addEventListener("click", () => undo_redo("redo"));

  document.addEventListener("keydown", (ev) => {
    const typing = is_text_entry(document.activeElement);
    const mod = ev.ctrlKey || ev.metaKey;
    // 入力欄の中では、文字の取り消しとしてブラウザに任せる
    // Ctrl+S：ブラウザの「名前を付けて保存」（HTML のダウンロード）を出さず、自動で保存していることを薄く知らせる。
    // 入力中でも同じ（入力した内容は欄を離れたときに保存される）
    if (mod && !ev.altKey && !ev.shiftKey && ev.key.toLowerCase() === "s") {
      ev.preventDefault();
      toast("このアプリは自動で保存しています（入力中の欄は、欄を離れると保存されます）", "faint");
      return;
    }
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
    } else if (ev.key === "F2" && !typing && !mod) {
      ev.preventDefault();
      rename_by_f2();
    } else if (ev.key === "Delete" && !typing && !mod && state.tab === "gantt" && state.selected.size > 0) {
      ev.preventDefault();
      delete_tasks_confirm(visible_task_order().filter((id) => state.selected.has(id)));
    } else if (ev.key === "Escape" && !typing && clip.mode === "cut") {
      clear_cut();
      toast("切り取りをやめました");
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
  setup_clipboard();
  setup_templates();
  setup_review();
  setup_tag_drop();
  // 落とし先（行・見出し）の外に出たら、前後の線や枠の印を消す
  document.addEventListener("dragover", (ev) => {
    if (dropMark.el && !(ev.target instanceof Element && ev.target.closest("[data-task_id], .gantt_header .gantt_name, #inbox_header"))) clear_drop_marks();
  });
  // 画面を閉じたらアプリに知らせる（ほかに開いている画面がなければ、1 分ほどでアプリが終わる）
  window.addEventListener("pagehide", () => { navigator.sendBeacon("/api/bye"); });
  load_jira_config().then(() => render_detail());
  setup_detail();
  reload();
}
