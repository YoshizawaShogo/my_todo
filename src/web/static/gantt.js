"use strict";
// 線表タブ。左の名前欄で追加・並べ替え・完了・名前変更、右の時間軸で日程をドラッグで決める。

const GANTT_DAY_W = 28; // 1 日の幅（px）
const GANTT_DAYS = 42; // 表示する日数（6 週間）

const gantt = {
  start: null, // 表示開始日（月曜, YYYY-MM-DD）
  filter: { text: "", tags: [], hideDone: false, ...load_json("gantt_filter", {}) },
  matched: new Set(), // 絞り込み条件に合う id
  shown: new Set(), // 出す id（条件に合うものと、その祖先）
};

function gantt_reset() {
  gantt.start = add_days(monday_of(today_text()), -7); // 先週の月曜から
}

function setup_gantt() {
  gantt_reset();
  $("gantt_prev").addEventListener("click", () => { gantt.start = add_days(gantt.start, -7); render_gantt(); });
  $("gantt_next").addEventListener("click", () => { gantt.start = add_days(gantt.start, 7); render_gantt(); });
  $("gantt_today").addEventListener("click", () => { gantt_reset(); render_gantt(); });
  $("inbox_add").addEventListener("click", () => begin_add("inbox"));
  attach_group_drop($("inbox_header"), "inbox");
  setup_gantt_wheel();
  setup_trash();

  let timer;
  $("gfilter_text").addEventListener("input", (ev) => {
    clearTimeout(timer);
    timer = setTimeout(() => { gantt.filter.text = ev.target.value; save_gantt_filter(); }, 150);
  });
  $("gfilter_text").addEventListener("keydown", (ev) => {
    if (ev.key === "Escape") {
      ev.target.value = "";
      gantt.filter.text = "";
      save_gantt_filter();
      ev.target.blur();
    }
  });
  $("gfilter_hide_done").addEventListener("change", (ev) => { gantt.filter.hideDone = ev.target.checked; save_gantt_filter(); });
  $("gfilter_clear").addEventListener("click", () => {
    gantt.filter = { text: "", tags: [], hideDone: gantt.filter.hideDone };
    save_gantt_filter();
  });
}

function save_gantt_filter() {
  save_json("gantt_filter", gantt.filter);
  render_gantt();
}

function gantt_filter_active() {
  return gantt.filter.text.trim() !== "" || gantt.filter.tags.length > 0;
}

function toggle_gantt_tag(name) {
  toggle_in(gantt.filter.tags, name);
  save_gantt_filter();
}

/// 線表の上でのスクロール：普通のホイールは行の上下スクロール（端で止まる。行が少なくて余地がないときはページ全体）。
/// Shift＋ホイール（とトラックパッドの横スクロール）で表示範囲を前後に動かす（下・右へ回すと未来へ）
function setup_gantt_wheel() {
  const PX_PER_DAY = 40; // この量のスクロールで 1 日動く
  let acc = 0;
  let frame = 0;
  $("gantt").addEventListener("wheel", (ev) => {
    if (ev.ctrlKey) return;
    if (document.body.classList.contains("gantt_dragging")) return;
    const horizontal = Math.abs(ev.deltaX) > Math.abs(ev.deltaY);
    const scale = ev.deltaMode === 1 ? 16 : ev.deltaMode === 2 ? 400 : 1; // 行単位・ページ単位のマウスにも対応
    ev.preventDefault();
    if (!ev.shiftKey && !horizontal) {
      // 普通のホイールは縦スクロール。ブラウザに任せると、縦に動かせないとき横に動いてしまうので自分で振り分ける。
      // 線表の中にスクロールする余地があれば、その中だけで動かす（端まで来たら止まり、ページは動かさない）。
      // 行が少なくて余地がまったくないときだけ、ページ全体を動かす
      const box = $("gantt");
      const dy = ev.deltaY * scale;
      if (box.scrollHeight > box.clientHeight + 1) box.scrollTop += dy;
      else window.scrollBy(0, dy);
      return;
    }
    // Shift＋ホイールはブラウザによって deltaX にも deltaY にも来る
    const delta = horizontal ? ev.deltaX : ev.deltaY;
    acc += delta * scale;
    const days = Math.trunc(acc / PX_PER_DAY);
    if (!days) return;
    acc -= days * PX_PER_DAY;
    gantt.start = add_days(gantt.start, days);
    // 描き直しは 1 フレームに 1 回にまとめる
    if (!frame) {
      frame = requestAnimationFrame(() => {
        frame = 0;
        render_gantt();
      });
    }
  }, { passive: false });
}

// ---------- ゴミ箱 ----------
/// 右下のゴミ箱を出す・隠す。"bar"（バーを運ぶ＝日程だけ消す）/ "task"（行を運ぶ＝タスクを削除）/ null（隠す）
function show_trash(kind) {
  const t = $("gantt_trash");
  t.hidden = kind === null;
  t.classList.remove("over");
  t.classList.toggle("task_mode", kind === "task");
  if (kind === "bar") t.textContent = "🗑️ ここに入れると日程を消す";
  if (kind === "task") t.textContent = "🗑️ ここに入れるとタスクを削除";
}

/// 行（名前）のドラッグをゴミ箱で受けて、タスクを削除する
function setup_trash() {
  const t = $("gantt_trash");
  t.addEventListener("dragover", (ev) => {
    if (state.dragId === null) return;
    ev.preventDefault();
    t.classList.add("over");
  });
  t.addEventListener("dragleave", () => t.classList.remove("over"));
  t.addEventListener("drop", (ev) => {
    ev.preventDefault();
    const ids = state.dragIds || (state.dragId === null ? [] : [state.dragId]);
    show_trash(null);
    delete_tasks_confirm(ids);
  });
}

// ---------- 絞り込み ----------
function gantt_matches(task) {
  const f = gantt.filter;
  if (f.tags.length) {
    const tags = inherited_tags(task);
    if (!f.tags.some((n) => tags.some((t) => same_tag(t, n)))) return false;
  }
  const words = f.text.trim().toLowerCase().split(/\s+/).filter(Boolean);
  if (words.length) {
    const hay = [task.title, task.note, task.next_action, ...task.tags].filter(Boolean).join("\n").toLowerCase();
    if (!words.every((w) => hay.includes(w))) return false;
  }
  return true;
}

/// 出すのは「条件に合うタスク（完了を隠すなら未完了のみ）」と「出す子孫を持つタスク」
function compute_gantt_filter() {
  gantt.matched = new Set(state.tasks.filter(gantt_matches).map((t) => t.id));
  gantt.shown = new Set();
  const walk = (t) => {
    let childShown = false;
    for (const c of children_of(t.id)) childShown = walk(c) || childShown;
    const self = gantt.matched.has(t.id) && !(gantt.filter.hideDone && t.status === "done");
    const show = self || childShown;
    if (show) gantt.shown.add(t.id);
    return show;
  };
  for (const key of ["root", "inbox"]) children_of(key).forEach(walk);
}

function gantt_visible(t) {
  return gantt.shown.has(t.id);
}

function render_gantt_filter() {
  $("gfilter_tags").replaceChildren(...tag_filter_buttons(gantt.filter.tags, toggle_gantt_tag));
  const input = $("gfilter_text");
  if (document.activeElement !== input) input.value = gantt.filter.text;
  $("gfilter_hide_done").checked = gantt.filter.hideDone;
  $("gfilter_clear").hidden = !gantt_filter_active();
}

// ---------- 期間 ----------
/// バーにする期間。予定（開始予定日・終了予定日）を優先し、なければ実績（着手日・完了日）で補う
function span_of(t) {
  const start = t.planned_start || (t.started_at ? t.started_at.slice(0, 10) : null);
  const end = t.planned_end || (t.status === "done" && t.completed_at ? t.completed_at.slice(0, 10) : null);
  return { start, end };
}

/// 子孫も含めた期間（親の「子の範囲」用）
function subtree_span(t) {
  let lo = null, hi = null;
  const visit = (x) => {
    const s = span_of(x);
    for (const d of [s.start, s.end, x.status === "done" ? null : x.next_action_date]) {
      if (!d) continue;
      if (lo === null || d < lo) lo = d;
      if (hi === null || d > hi) hi = d;
    }
    children_of(x.id).filter(gantt_visible).forEach(visit);
  };
  children_of(t.id).filter(gantt_visible).forEach(visit);
  return lo === null ? null : { start: lo, end: hi };
}

// ---------- 行の並び ----------
function gantt_rows() {
  const rows = [];
  const addRow = (key, depth) => {
    if (pendingAdd && pendingAdd.key === key) rows.push({ add: pendingAdd, depth });
  };
  const walk = (t, depth) => {
    const kids = children_of(t.id).filter(gantt_visible);
    const adding = pendingAdd && pendingAdd.key === t.id;
    rows.push({ task: t, depth, hasKids: kids.length > 0 || adding });
    if (!state.collapsed.has(t.id)) {
      kids.forEach((k) => walk(k, depth + 1));
      addRow(t.id, depth + 1);
    }
  };
  children_of("root").filter(gantt_visible).forEach((t) => walk(t, 0));
  addRow("root", 0);
  return rows;
}

// ---------- 描画 ----------
function render_gantt() {
  compute_gantt_filter();
  render_gantt_filter();
  const start = gantt.start;
  const end = add_days(start, GANTT_DAYS - 1);
  const today = today_text();
  const width = GANTT_DAYS * GANTT_DAY_W;
  const x_of = (ymd) => days_between(start, ymd) * GANTT_DAY_W;
  const ctx = { start, end, today, width, x_of };
  $("gantt_range").textContent = `${short_date(start)} 〜 ${short_date(end)}`;

  // 日付の見出しと背景（週末・週の区切り・今日の線）
  const days = el("div", { class: "gantt_days", style: `width:${width}px` });
  const bg = el("div", { class: "gantt_bg", style: `left:var(--gantt_name_w);width:${width}px` });
  for (let i = 0; i < GANTT_DAYS; i++) {
    const d = add_days(start, i);
    const [y, m, dd] = d.split("-").map(Number);
    const wd = weekday_of(d);
    const weekend = wd === 0 || wd === 6;
    const monthStart = dd === 1 || i === 0;
    const cell = el("div", {
      class: "gantt_day" + (weekend ? " weekend" : "") + (d === today ? " today" : "") + (monthStart ? " month_start" : ""),
      style: `left:${i * GANTT_DAY_W}px;width:${GANTT_DAY_W}px`,
      title: short_date(d),
    });
    if (monthStart) cell.append(el("span", { class: "month" }, `${y === new Date().getFullYear() ? "" : y + "/"}${m}月`));
    cell.append(el("span", { class: "dnum" }, String(dd)));
    days.append(cell);
    if (weekend) bg.append(el("div", { class: "weekend", style: `left:${i * GANTT_DAY_W}px;width:${GANTT_DAY_W}px` }));
    if (wd === 1 && i > 0) bg.append(el("div", { class: "week_line", style: `left:${i * GANTT_DAY_W}px` }));
  }
  if (today >= start && today <= end) {
    bg.append(el("div", { class: "today_line", style: `left:${x_of(today) + GANTT_DAY_W / 2 - 1}px` }));
  }
  // 左上の「タスク」に落とすと、最上位の末尾へ移す
  const corner = el("div", { class: "gantt_name", title: "ここに落とすと最上位の末尾へ" },
    el("span", { class: "corner_label" }, "タスク"),
    el("button", { type: "button", class: "small_button", title: "線表にタスクを追加", onclick: () => begin_add("root") }, "➕ 追加"),
    el("button", {
      type: "button",
      class: "small_button",
      id: "template_button",
      title: "タスクの木を型として保存し、開始日を選んで作り直す",
      onclick: (ev) => { ev.stopPropagation(); toggle_template_panel(); },
    }, "テンプレート"));
  attach_group_drop(corner, "root");
  const header = el("div", { class: "gantt_header" }, corner, days);

  const rowsEl = el("div", { class: "gantt_rows" });
  const rows = gantt_rows();
  for (const r of rows) {
    if (r.add) rowsEl.append(gantt_add_row(r, width));
    else rowsEl.append(gantt_row(r, ctx));
  }
  // タスクが 1 件もないときは案内の行を出し、ここをドロップ先にする
  if (!rows.length) {
    const empty = el("div", { class: "gantt_row empty_row" },
      el("div", { class: "gantt_name" },
        el("span", { class: "empty_hint" }, gantt_filter_active() ? "条件に合うタスクはありません" : "未分類からドラッグするか、➕ 追加でタスクを作る")),
      el("div", { class: "gantt_track", style: `width:${width}px` }));
    attach_group_drop(empty, "root");
    rowsEl.append(empty);
  }

  const grid = el("div", { class: "gantt_grid" }, header, bg, rowsEl);
  const scroll = $("gantt");
  const keepLeft = scroll.scrollLeft, keepTop = scroll.scrollTop;
  scroll.replaceChildren(grid);
  scroll.scrollLeft = keepLeft;
  scroll.scrollTop = keepTop;
  render_inbox_pane();
}

// ---------- 未分類ペイン ----------
/// 線表の下に、未分類のタスクを木のまま並べる。行をドラッグして線表へ移せる
function render_inbox_pane() {
  const list = $("inbox_list");
  const roots = children_of("inbox").filter(gantt_visible);
  const rows = [];
  const walk = (t, depth) => {
    const kids = children_of(t.id).filter(gantt_visible);
    const adding = pendingAdd && pendingAdd.key === t.id;
    rows.push(inbox_row(t, depth, kids.length > 0 || adding));
    if (!state.collapsed.has(t.id)) {
      kids.forEach((k) => walk(k, depth + 1));
      if (adding) rows.push(inbox_add_row(depth + 1));
    }
  };
  roots.forEach((t) => walk(t, 0));
  if (pendingAdd && pendingAdd.key === "inbox") rows.push(inbox_add_row(0));
  if (!rows.length) {
    rows.push(el("p", { class: "empty_hint" },
      gantt_filter_active() ? "条件に合う未分類のタスクはありません" : "未分類のタスクはありません。思いついたことは / キーでここに入れておけます"));
  }
  list.replaceChildren(...rows);
}

function inbox_add_row(depth) {
  return el("div", { class: "inbox_row adding", style: `padding-left:${8 + depth * 16 + 20}px` }, add_input(pendingAdd));
}

function inbox_row(t, depth, hasKids) {
  const done = t.status === "done";
  const row = el("div", {
    class: "inbox_row" + (is_selected(t.id) ? " selected" : "") + (done ? " done" : "") +
      (gantt.matched.has(t.id) ? "" : " dim"),
    "data-task_id": t.id,
    style: `padding-left:${8 + depth * 16}px`,
    onclick: (ev) => select(t.id, ev),
  });
  const twisty = el("span", {
    class: "twisty" + (hasKids ? "" : " leaf"),
    onclick: (ev) => { ev.stopPropagation(); toggle_collapse(t.id); },
  }, state.collapsed.has(t.id) ? "▸" : "▾");
  const check = el("input", {
    type: "checkbox",
    title: done ? "チェックを外すと未着手に戻す" : "完了にする",
    onclick: (ev) => ev.stopPropagation(),
    onchange: (ev) => set_done(t, ev.target.checked),
  });
  check.checked = done;
  const label = el("span", {
    class: "label",
    title: "ダブルクリックで名前を変更・ドラッグで線表へ移す",
    ondblclick: (ev) => { ev.stopPropagation(); start_inline_edit(label, t); },
  }, t.title);
  const dates = [];
  if (t.next_action_date && !done) dates.push(`▶ ${short_date(t.next_action_date)}`);
  if (t.planned_end) dates.push(`〜${short_date(t.planned_end)}`);
  row.append(twisty, check, label,
    ...tag_chips(t.tags, (n) => gantt.filter.tags.some((x) => same_tag(x, n)), toggle_gantt_tag),
    ...(dates.length ? [el("span", { class: "date_chip" }, dates.join(" "))] : []),
    el("span", { class: "row_actions" },
      icon_button("➕", "子タスクを追加", () => begin_add(t.id), "emoji"),
      icon_button("🗑️", "このタスクを削除（子も一緒に消えます）", () => remove(t.id), "emoji danger")));
  attach_tree_drag(row, row, t);
  return row;
}

function gantt_add_row(r, width) {
  return el("div", { class: "gantt_row adding" },
    el("div", { class: "gantt_name", style: `padding-left:${8 + r.depth * 16 + 20}px` }, add_input(r.add)),
    el("div", { class: "gantt_track", style: `width:${width}px` }));
}

function gantt_row(r, ctx) {
  const t = r.task;
  const { start, end, today, width, x_of } = ctx;
  const done = t.status === "done";
  const row = el("div", {
    class: "gantt_row" + (is_selected(t.id) ? " selected" : "") + (done ? " done" : "") +
      (gantt.matched.has(t.id) ? "" : " dim"),
    "data-task_id": t.id,
    onclick: (ev) => select(t.id, ev),
  });

  // 名前欄：開閉・完了チェック・名前・タグ・操作
  const twisty = el("span", {
    class: "twisty" + (r.hasKids ? "" : " leaf"),
    onclick: (ev) => { ev.stopPropagation(); toggle_collapse(t.id); },
  }, state.collapsed.has(t.id) ? "▸" : "▾");
  const check = el("input", {
    type: "checkbox",
    title: done ? "チェックを外すと未着手に戻す" : "完了にする",
    onclick: (ev) => ev.stopPropagation(),
    onchange: (ev) => set_done(t, ev.target.checked),
  });
  check.checked = done;
  const label = el("span", {
    class: "label",
    title: [...path_titles(t), t.title].join(" › ") + "\nダブルクリックで名前を変更・ドラッグで移動",
    ondblclick: (ev) => { ev.stopPropagation(); start_inline_edit(label, t); },
  }, t.title);
  const name = el("div", { class: "gantt_name", style: `padding-left:${8 + r.depth * 16}px` },
    twisty, check, label,
    ...tag_chips(t.tags, (n) => gantt.filter.tags.some((x) => same_tag(x, n)), toggle_gantt_tag),
    el("span", { class: "row_actions" },
      icon_button("➕", "子タスクを追加", () => begin_add(t.id), "emoji"),
      icon_button("🗑️", "このタスクを削除（子も一緒に消えます）", () => remove(t.id), "emoji danger")));
  attach_tree_drag(name, row, t);
  row.append(name);

  // 時間軸
  const track = el("div", { class: "gantt_track", style: `width:${width}px` });
  const clipped = { left: false, right: false };
  // 範囲外を切り詰めて配置する
  const place = (node, s, e) => {
    if (e < start) { clipped.left = true; return; }
    if (s > end) { clipped.right = true; return; }
    const s2 = s < start ? start : s;
    const e2 = e > end ? end : e;
    node.style.left = `${x_of(s2) + 1}px`;
    node.style.width = `${(days_between(s2, e2) + 1) * GANTT_DAY_W - 2}px`;
    track.append(node);
  };
  const mark = (cls, ch, d, title) => {
    if (d < start) { clipped.left = true; return; }
    if (d > end) { clipped.right = true; return; }
    track.append(el("span", { class: "mark " + cls, style: `left:${x_of(d)}px;width:${GANTT_DAY_W}px`, title }, ch));
  };

  const span = span_of(t);
  const alerts = task_alerts(t, today);
  const overdue = alerts.overdue;
  const alertClass = overdue ? " overdue" : alerts.late_start ? " late_start" : "";
  const fromActual = done && !t.planned_end && span.end;
  const tip = [
    t.title,
    span.start && `開始 ${short_date(span.start)}${t.planned_start ? "" : "（着手日）"}`,
    span.end && `${fromActual ? "完了" : "終了予定"} ${short_date(span.end)}`,
    done && t.completed_at && !fromActual && `完了 ${short_date(t.completed_at.slice(0, 10))}`,
  ].filter(Boolean).join("\n");
  const hasSpan = Boolean(span.start && span.end && span.start <= span.end);
  if (hasSpan) {
    const bar = el("div", {
      class: `bar own ${t.status}` + alertClass,
      title: tip + (overdue ? "\n終了予定日を過ぎています" : alerts.late_start ? "\n開始予定日を過ぎても未着手です" : "") + "\nドラッグで移動、端をドラッグで期間を変更",
    });
    // 表示範囲で切れている側には端のつまみを付けない
    if (span.start >= start) bar.append(el("span", { class: "bar_handle left" }));
    if (span.end <= end) bar.append(el("span", { class: "bar_handle right" }));
    place(bar, span.start, span.end);
  } else if (r.hasKids && subtree_span(t)) {
    // 親は子の範囲を出し、自分の終了日があれば ◆ を重ねる
    const sub = subtree_span(t);
    place(el("div", { class: "bar summary", title: `${t.title}\n子の範囲 ${short_date(sub.start)} 〜 ${short_date(sub.end)}` }), sub.start, sub.end);
    if (span.end) mark(done ? "done" : overdue ? "overdue" : "", "◆", span.end, tip);
  } else if (span.end) {
    mark(done ? "done" : overdue ? "overdue" : "", "◆", span.end, tip);
  } else if (span.start) {
    // 終わりが決まっていない：開始から今日まで（未来の開始なら 1 日分）を薄く出す
    const e = !done && today > span.start ? today : span.start;
    place(el("div", { class: `bar ${t.status} open_end` + alertClass, title: tip + "\n終了予定日は未設定" + (alerts.late_start ? "\n開始予定日を過ぎています" : "") }), span.start, e);
  }
  if (t.next_action_date && !done) {
    mark("next" + (alerts.next_overdue ? " overdue" : ""), "▶", t.next_action_date,
      `次の予定 ${short_date(t.next_action_date)}${t.next_action ? " " + t.next_action : ""}${alerts.next_overdue ? "（過ぎています）" : ""}`);
  }
  if (clipped.left) track.append(el("span", { class: "clip_left", title: "表示範囲より前に予定あり" }, "«"));
  if (clipped.right) track.append(el("span", { class: "clip_right", title: "表示範囲より後に予定あり" }, "»"));
  if (!hasSpan) {
    track.classList.add("creatable");
    track.title = "横にドラッグして開始予定日と終了予定日を決める";
  }
  attach_gantt_drag(track, t, hasSpan ? span : null, ctx);
  row.append(track);
  return row;
}

// ---------- ドラッグで日程を決める・動かす ----------
/// span があればバーの移動（中央）・期間の変更（端）、なければドラッグした範囲で新しく決める。
/// 動かさずに離したときはただのクリック（行の選択）として扱う
function attach_gantt_drag(track, t, span, ctx) {
  track.addEventListener("pointerdown", (ev) => {
    if (ev.button !== 0) return;
    // 上に重なるマーク（▶ など）をつかんでも効くように、要素ではなく位置で判定する
    const original = track.querySelector(".bar.own");
    let mode = null;
    if (!span) {
      mode = "create";
    } else if (original) {
      const r = original.getBoundingClientRect();
      const x = ev.clientX;
      const edge = Math.min(8, r.width / 3);
      if (x >= r.left - 3 && x <= r.right + 3) {
        if (x <= r.left + edge && original.querySelector(".bar_handle.left")) mode = "start";
        else if (x >= r.right - edge && original.querySelector(".bar_handle.right")) mode = "end";
        else mode = "move";
      }
    }
    if (!mode) return; // バーのある行の空いている所は選択だけ

    ev.preventDefault();
    track.setPointerCapture(ev.pointerId);
    const rect = track.getBoundingClientRect();
    const day_at = (x) => Math.max(0, Math.min(GANTT_DAYS - 1, Math.floor((x - rect.left) / GANTT_DAY_W)));
    const x0 = ev.clientX;
    const day0 = day_at(x0);
    let moved = false;
    let range = null;
    let preview = null;
    let overTrash = false; // バーをゴミ箱の上まで運んだか（移動のときだけ）
    const label = el("div", { class: "drag_label" });

    const compute = (x) => {
      if (mode === "create") {
        const d = day_at(x);
        return [add_days(ctx.start, Math.min(day0, d)), add_days(ctx.start, Math.max(day0, d))];
      }
      const delta = Math.round((x - x0) / GANTT_DAY_W);
      if (mode === "move") return [add_days(span.start, delta), add_days(span.end, delta)];
      if (mode === "start") {
        const s = add_days(span.start, delta);
        return [s > span.end ? span.end : s, span.end];
      }
      const e = add_days(span.end, delta);
      return [span.start, e < span.start ? span.start : e];
    };

    const on_move = (e) => {
      if (!moved && Math.abs(e.clientX - x0) < 4) return;
      if (!moved) {
        moved = true;
        document.body.classList.add("gantt_dragging", "drag_" + mode);
        preview = el("div", { class: `bar drag_preview ${t.status}` });
        track.append(preview);
        document.body.append(label);
        original?.classList.add("drag_source");
        if (mode === "move") show_trash("bar");
      }
      if (mode === "move") {
        const r = $("gantt_trash").getBoundingClientRect();
        overTrash = e.clientX >= r.left && e.clientX <= r.right && e.clientY >= r.top && e.clientY <= r.bottom;
        $("gantt_trash").classList.toggle("over", overTrash);
        if (overTrash) {
          preview.hidden = true;
          label.textContent = "離すと日程（開始予定日・終了予定日）を消します";
          label.style.left = `${e.clientX - 260}px`;
          label.style.top = `${e.clientY - 30}px`;
          return;
        }
      }
      range = compute(e.clientX);
      const [s, en] = range;
      // プレビューは表示範囲に切り詰めて描く
      const s2 = s < ctx.start ? ctx.start : s;
      const e2 = en > ctx.end ? ctx.end : en;
      preview.hidden = e2 < s2;
      if (e2 >= s2) {
        preview.style.left = `${ctx.x_of(s2) + 1}px`;
        preview.style.width = `${(days_between(s2, e2) + 1) * GANTT_DAY_W - 2}px`;
      }
      label.textContent = `${short_date(s)} 〜 ${short_date(en)}（${days_between(s, en) + 1} 日）`;
      label.style.left = `${e.clientX + 14}px`;
      label.style.top = `${e.clientY - 30}px`;
    };

    const finish = (commit) => {
      track.removeEventListener("pointermove", on_move);
      track.removeEventListener("pointerup", on_up);
      track.removeEventListener("pointercancel", on_cancel);
      document.body.classList.remove("gantt_dragging", "drag_" + mode);
      label.remove();
      preview?.remove();
      original?.classList.remove("drag_source");
      show_trash(null);
      if (commit && moved && overTrash) {
        // ゴミ箱に入れた：日程だけ消す（タスクは残る）
        update(t.id, { planned_start: null, planned_end: null })
          .then(() => toast(`「${t.title}」の日程を消しました（Ctrl+Z で元に戻す）`))
          .catch(() => render());
        return;
      }
      if (!commit || !moved || !range) return;
      const [s, e] = range;
      // 変化なし（実績から引いたバーは、予定として確定させるため保存する）
      if (span && s === span.start && e === span.end && t.planned_start && t.planned_end) return;
      update(t.id, { planned_start: s, planned_end: e }).catch(() => render());
    };
    const on_up = () => finish(true);
    const on_cancel = () => finish(false);
    track.addEventListener("pointermove", on_move);
    track.addEventListener("pointerup", on_up);
    track.addEventListener("pointercancel", on_cancel);
  });
}
