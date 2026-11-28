"use strict";
// 線表タブ。左の名前欄で追加・並べ替え・完了・名前変更、右の時間軸で日程をドラッグで決める。

// 表示の縮尺。step は「前へ」「次へ」で動く日数、days_every は日付の数字を出す間隔（0 なら出さず月だけ）
const GANTT_SCALES = {
  weeks: { label: "6 週間", dayW: 6, days: 42, step: 7, stepText: "週", back: 7, days_every: "day", fit: true },
  // どの縮尺も、横にスクロールしなくても全体が見えるよう、線表の幅に合わせて 1 日の幅を決める（dayW は最小値）
  quarter: { label: "3 か月", dayW: 2, days: 98, step: 28, stepText: "4 週", back: 14, days_every: "monday", fit: true },
  year: { label: "1 年", dayW: 0.8, days: 371, step: 91, stepText: "3 か月", back: 28, days_every: "none", fit: true },
};
let GANTT_DAY_W = 28; // 1 日の幅（px）。縮尺で変わる
let GANTT_DAYS = 42; // 表示する日数。縮尺で変わる

const gantt = {
  start: null, // 表示開始日（月曜, YYYY-MM-DD）
  scale: "weeks", // GANTT_SCALES のキー
  filter: { text: "", tags: [], hideDone: false, mineOnly: false, ...load_json("gantt_filter", {}) },
  matched: new Set(), // 絞り込み条件に合う id
  shown: new Set(), // 出す id（条件に合うものと、その祖先）
  pinned: new Set(), // 絞り込み中に追加したタスク（条件に合わなくても、絞り込みを変えるまで出しておく）
};

function gantt_scale() {
  return GANTT_SCALES[gantt.scale];
}

/// 縮尺を変える（今日の位置が見えるように表示範囲も合わせ直す）
function set_gantt_scale(key) {
  if (!GANTT_SCALES[key]) key = "weeks";
  gantt.scale = key;
  GANTT_DAY_W = GANTT_SCALES[key].dayW;
  GANTT_DAYS = GANTT_SCALES[key].days;
  save_json("gantt_scale", key);
  const sc = gantt_scale();
  $("gantt_prev").textContent = `◀ 前の${sc.stepText}`;
  $("gantt_next").textContent = `次の${sc.stepText} ▶`;
  for (const b of document.querySelectorAll("#gantt_scale button")) b.classList.toggle("active", b.dataset.scale === key);
}

/// 「前の週」「前の 3 か月」のように縮尺で文字が変わっても、ボタンの幅（と右にあるボタンの位置）が
/// 変わらないよう、いちばん長い文字に合わせた幅を最初から取っておく
let stepWidthsFixed = false;
function fix_step_button_widths() {
  // 線表のタブが隠れている間は幅を測れないので、見えている描画のときに 1 回だけ測る
  if (stepWidthsFixed || $("gantt_prev").offsetWidth === 0) return;
  stepWidthsFixed = true;
  for (const [id, text] of [["gantt_prev", (sc) => `◀ 前の${sc.stepText}`], ["gantt_next", (sc) => `次の${sc.stepText} ▶`]]) {
    const b = $(id);
    let max = 0;
    for (const sc of Object.values(GANTT_SCALES)) {
      b.textContent = text(sc);
      max = Math.max(max, b.getBoundingClientRect().width);
    }
    b.style.minWidth = `${Math.ceil(max)}px`;
  }
  set_gantt_scale(gantt.scale); // 文字を今の縮尺に戻す
}

function gantt_reset() {
  // 少し前（縮尺に応じて 1〜4 週）の月曜から
  gantt.start = monday_of(add_days(today_text(), -gantt_scale().back));
}

function setup_gantt() {
  $("gantt_scale").replaceChildren(...Object.entries(GANTT_SCALES).map(([key, sc]) =>
    el("button", {
      type: "button",
      class: "small_button",
      "data-scale": key,
      title: `${sc.label}を表示する`,
      onclick: () => { set_gantt_scale(key); gantt_reset(); render_gantt(); },
    }, sc.label)));
  set_gantt_scale(load_json("gantt_scale", "weeks"));
  gantt_reset();
  $("gantt_prev").addEventListener("click", () => { gantt.start = add_days(gantt.start, -gantt_scale().step); render_gantt(); });
  $("gantt_next").addEventListener("click", () => { gantt.start = add_days(gantt.start, gantt_scale().step); render_gantt(); });
  $("gantt_today").addEventListener("click", () => { gantt_reset(); render_gantt(); });
  $("inbox_add").addEventListener("click", () => begin_add("inbox"));
  attach_group_drop($("inbox_header"), "inbox");
  setup_gantt_wheel();
  setup_trash();
  // 幅に合わせる縮尺では、線表の幅が変わったら（窓の大きさ・右の詳細パネルの開閉）描き直す
  let resizeFrame = 0;
  let lastWidth = 0;
  new ResizeObserver(() => {
    const w = $("gantt").clientWidth;
    if (w === lastWidth) return;
    lastWidth = w;
    if (state.tab !== "gantt" || !gantt_scale().fit || resizeFrame) return;
    resizeFrame = requestAnimationFrame(() => { resizeFrame = 0; render_gantt(); });
  }).observe($("gantt"));

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
  $("gfilter_mine_only").addEventListener("change", (ev) => { gantt.filter.mineOnly = ev.target.checked; save_gantt_filter(); });
  $("gfilter_clear").addEventListener("click", () => {
    gantt.filter = { text: "", tags: [], hideDone: gantt.filter.hideDone, mineOnly: gantt.filter.mineOnly };
    save_gantt_filter();
  });
}

function save_gantt_filter() {
  gantt.pinned.clear(); // 絞り込みを変えたら、追加したタスクを出し続けるのはやめる
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
  // この量のスクロールで 1 日動く（縮尺を小さくするほど 1 回で多くの日数が動く）
  const px_per_day = () => Math.max(1, (40 * GANTT_DAY_W) / 28);
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
    const days = Math.trunc(acc / px_per_day());
    if (!days) return;
    acc -= days * px_per_day();
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
function show_trash(kind, from = null) {
  const t = $("gantt_trash");
  t.hidden = kind === null;
  t.classList.remove("over");
  t.classList.toggle("task_mode", kind === "task");
  t.removeAttribute("style");
  if (kind === "bar") t.textContent = "🗑️ ここに入れると日程を消す";
  if (kind === "task" || kind === "tag") {
    t.textContent = kind === "task" ? "🗑️ ここに入れるとタスクを削除" : "🗑️ ここに入れるとタグを外す";
    t.classList.add("task_mode"); // HTML5 のドラッグを受け取る
    if (from) place_task_trash(t, from);
  }
}

/// 名前を運ぶときは、つかんだ行の高さで、少し右に小さなゴミ箱を出す（画面の右下だと遠すぎるため）。
/// つかんだ所のすぐ横に出すと、少し動かしただけで誤って入れてしまうので、マウスから 100px 以上離す。
/// 線表の中の行なら、時間軸の側（名前の欄の右）に出して名前の上には重ねない
function place_task_trash(t, { row, x, y }) {
  const W = 210, H = 64, GAP = 100, MARGIN = 12;
  const box = $("gantt").getBoundingClientRect();
  const inGantt = y >= box.top && y <= box.bottom && x >= box.left && x <= box.right;
  const nameW = parseFloat(getComputedStyle($("gantt")).getPropertyValue("--gantt_name_w")) || 0;
  let left = x + GAP;
  if (inGantt) left = Math.max(left, box.left + nameW + 24);
  if (left + W > window.innerWidth - MARGIN) left = Math.max(MARGIN, x - GAP - W); // 右に入らなければ左へ
  const top = Math.max(MARGIN, Math.min(row.top + row.height / 2 - H / 2, window.innerHeight - H - MARGIN));
  Object.assign(t.style, {
    left: `${left}px`,
    top: `${top}px`,
    width: `${W}px`,
    height: `${H}px`,
    right: "auto",
    bottom: "auto",
    padding: "6px 10px",
    fontSize: "12px",
  });
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
    const hidden = (gantt.filter.hideDone && t.status === "done") || (gantt.filter.mineOnly && t.kind === "external");
    const self = (gantt.matched.has(t.id) && !hidden) || gantt.pinned.has(t.id);
    const show = self || childShown;
    if (show) gantt.shown.add(t.id);
    return show;
  };
  for (const key of ["root", "inbox"]) children_of(key).forEach(walk);
}

/// 絞り込みの条件に合わないのに、追加したばかりなので出しているタスクか
function gantt_pinned_only(t) {
  return gantt.pinned.has(t.id) && !gantt.matched.has(t.id);
}

/// バーの説明に付ける、見積もりと 1 日あたりの重さ
function load_tip(t) {
  const l = task_load(t);
  if (!l) return t.kind === "external" ? "" : "\n見積もりなし（太さは仮）";
  return `\n見積もり ${fmt_hours(l.est)}（太さは見積もりの大きさ）` +
    (too_heavy(t) ? `\n期間に収まりません（平日 ${l.days} 日 × ${fmt_hours(day_capacity())} を超える）` : "");
}

/// 他の人の作業の印（行の名前の横）
function kind_badge(t) {
  return t.kind === "external" ? [el("span", { class: "kind_badge", title: "他の人の作業（工数記録の候補に出ず、要確認にも出ない）" }, "他の人")] : [];
}

/// その印（行の名前の横）
function pinned_note(t) {
  return gantt_pinned_only(t)
    ? [el("span", { class: "pinned_note", title: "絞り込み中に追加したので出しています。絞り込みを変えると見えなくなります" }, "絞り込みの対象外")]
    : [];
}

function gantt_visible(t) {
  return gantt.shown.has(t.id);
}

function render_gantt_filter() {
  $("gfilter_tags").replaceChildren(...tag_filter_buttons(() => gantt.filter.tags, toggle_gantt_tag));
  const input = $("gfilter_text");
  if (document.activeElement !== input) input.value = gantt.filter.text;
  $("gfilter_hide_done").checked = gantt.filter.hideDone;
  $("gfilter_mine_only").checked = Boolean(gantt.filter.mineOnly);
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
function days_in_month(y, m) {
  return new Date(y, m, 0).getDate();
}

/// 縮尺が「幅に合わせる」なら、いまの線表の幅から 1 日の幅を決める
function fit_gantt_day_width() {
  const sc = gantt_scale();
  if (!sc.fit) return;
  const box = $("gantt");
  const nameW = parseFloat(getComputedStyle(box).getPropertyValue("--gantt_name_w")) || 0;
  // 時間軸の中身ははみ出しを切っているので、枠線の分だけ残す
  const avail = box.clientWidth - nameW - 2;
  GANTT_DAY_W = Math.max(sc.dayW, Math.floor((avail / GANTT_DAYS) * 100) / 100);
}

let ganttRenderPending = false;
function render_gantt() {
  // バーをドラッグしている間は描き直さない（つかんでいる要素が消えてドラッグが途切れるため）。終わってから描き直す
  if (document.body.classList.contains("gantt_dragging")) {
    ganttRenderPending = true;
    return;
  }
  ganttRenderPending = false;
  fix_step_button_widths();
  fit_gantt_day_width();
  compute_gantt_filter();
  render_gantt_filter();
  const start = gantt.start;
  const end = add_days(start, GANTT_DAYS - 1);
  const today = today_text();
  const width = GANTT_DAYS * GANTT_DAY_W;
  const x_of = (ymd) => days_between(start, ymd) * GANTT_DAY_W;
  const ctx = { start, end, today, width, x_of };
  $("gantt_range").textContent = `${short_date(start)} 〜 ${short_date(end)}`;

  // 日付の見出しと背景（週末・週の区切り・今日の線）。縮尺が小さいときは週末・週の線を省き、月の線を引く
  const sc = gantt_scale();
  const fine = GANTT_DAY_W >= 8;
  // 詰め込みすぎの期間に入っている日（見出しを赤くする）
  const crowdedDays = new Map();
  for (const p of overloaded_periods()) {
    for (let d = p.start; d <= p.end; d = add_days(d, 1)) if (!crowdedDays.has(d)) crowdedDays.set(d, p);
  }
  const days = el("div", { class: "gantt_days scale_" + gantt.scale + (GANTT_DAY_W < 20 ? " narrow" : ""), style: `width:${width}px` });
  const bg = el("div", { class: "gantt_bg", style: `left:var(--gantt_name_w);width:${width}px` });
  for (let i = 0; i < GANTT_DAYS; i++) {
    const d = add_days(start, i);
    const [y, m, dd] = d.split("-").map(Number);
    const wd = weekday_of(d);
    const weekend = wd === 0 || wd === 6;
    const monthStart = dd === 1 || i === 0;
    const period = crowdedDays.get(d);
    const over = Boolean(period);
    const cell = el("div", {
      class: "gantt_day" + (weekend && fine ? " weekend" : "") + (d === today ? " today" : "") + (monthStart ? " month_start" : "") + (over ? " overload" : ""),
      style: `left:${i * GANTT_DAY_W}px;width:${GANTT_DAY_W}px`,
      title: short_date(d) + (period
        ? `\n${short_date(period.start)}〜${short_date(period.end)} に収めるタスクの残りが ${fmt_hours(Math.round(period.total))}、平日に入るのは ${fmt_hours(period.cap)}：` +
          period.tasks.map((t) => `\n・${t.title}`).join("")
        : ""),
    });
    // 月の見出し。1 年表示は幅が狭いので「1月」だけ（1 月は年を出す）。
    // 縮尺が小さいとき、最初の列が月の終わりなら、すぐ次の月の見出しと重なるので出さない
    const label = gantt.scale === "year"
      ? (m === 1 ? `${y}年` : `${m}月`)
      : `${y === new Date().getFullYear() ? "" : y + "/"}${m}月`;
    const crowded = i === 0 && dd !== 1 && !fine && days_in_month(y, m) - dd < 10;
    if (monthStart && !crowded) cell.append(el("span", { class: "month" }, label));
    if (sc.days_every === "day" || (sc.days_every === "monday" && wd === 1)) cell.append(el("span", { class: "dnum" }, String(dd)));
    days.append(cell);
    if (weekend && fine) bg.append(el("div", { class: "weekend", style: `left:${i * GANTT_DAY_W}px;width:${GANTT_DAY_W}px` }));
    if (fine ? wd === 1 && i > 0 : dd === 1 && i > 0) bg.append(el("div", { class: "week_line", style: `left:${i * GANTT_DAY_W}px` }));
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
  draw_dependency_arrows(grid, rowsEl, ctx);
  render_inbox_pane();
}

// ---------- 依存関係の矢印 ----------
const SVG_NS = "http://www.w3.org/2000/svg";
function svg(tag, attrs = {}) {
  const e = document.createElementNS(SVG_NS, tag);
  for (const [k, v] of Object.entries(attrs)) e.setAttribute(k, v);
  return e;
}

const ARROW_LEN = 7; // 矢印の先の長さ（px）
const ARROW_HALF = 4; // 矢印の先の幅の半分

/// 2 点を結ぶ線。横向きに出て、横向きに (x2, y2) に入る。
/// 戻る向き（back）は、一度右へ出て行の間を左へ戻り、後のバーの始まりよりさらに左まで戻ってから、
/// 左から入るはっきりした S 字にする（なだらかな曲線だとバーに重なって見づらいため）
function curve_path(x1, y1, x2, y2, back = false) {
  if (back) {
    const k = 14;
    const ym = (y1 + y2) / 2;
    const xa = x2 - Math.max(16, GANTT_DAY_W); // 入る前に戻る位置
    return `M ${x1} ${y1} C ${x1 + k} ${y1}, ${x1 + k} ${ym}, ${x1} ${ym} L ${xa} ${ym} ` +
      `C ${xa - k} ${ym}, ${xa - k} ${y2}, ${xa} ${y2} L ${x2} ${y2}`;
  }
  const dx = Math.max(24, Math.abs(x2 - x1) / 2);
  return `M ${x1} ${y1} C ${x1 + dx} ${y1}, ${x2 - dx} ${y2}, ${x2} ${y2}`;
}

/// 右向きの矢印の先（先端が (x, y)）
function arrow_head(x, y, cls) {
  return svg("path", { d: `M ${x - ARROW_LEN} ${y - ARROW_HALF} L ${x} ${y} L ${x - ARROW_LEN} ${y + ARROW_HALF} z`, class: cls });
}

/// 先のタスクのバーの終わりから、後のタスクのバーの始まりへ矢印を引く。順番が逆なら赤
function draw_dependency_arrows(grid, rowsEl, ctx) {
  const rowY = new Map();
  for (const r of rowsEl.querySelectorAll(".gantt_row[data-task_id]")) {
    rowY.set(Number(r.dataset.task_id), r.offsetTop + r.offsetHeight / 2);
  }
  const layer = svg("svg", { class: "dep_layer", width: ctx.width, height: rowsEl.offsetHeight });
  layer.style.left = "var(--gantt_name_w)";
  layer.style.top = `${rowsEl.offsetTop}px`;
  for (const { pre, post } of all_dependencies()) {
    if (!rowY.has(pre.id) || !rowY.has(post.id)) continue;
    const a = dep_dates(pre), b = dep_dates(post);
    if (!a.end || !b.start) continue;
    const broken = dependency_broken(pre, post);
    const x1 = ctx.x_of(a.end) + GANTT_DAY_W;
    // 矢印は必ず後のバーの始まりに向ける。始まりが先のバーの終わりより左なら（同じ日の引き継ぎや、先が完了して
    // 問題でなくなったものも含め）戻る S 字で入る。赤くするのは問題があるとき（broken）だけ
    const x2 = ctx.x_of(b.start);
    const back = x2 - ARROW_LEN < x1;
    // 線は矢印の先の付け根の真ん中で終え、その先に矢印の先を描く（線が矢印の先に重ならないように）
    const y2 = rowY.get(post.id);
    const group = svg("g", { class: "dep" + (broken ? " broken" : "") });
    const path = svg("path", {
      d: curve_path(x1, rowY.get(pre.id), x2 - ARROW_LEN, y2, back),
      class: "dep_line" + (broken ? " broken" : ""),
    });
    group.append(path, arrow_head(x2, y2, "dep_head" + (broken ? " broken" : "")));
    const title = svg("title");
    title.textContent = `「${pre.title}」が終わってから「${post.title}」` +
      (broken ? "\n順番が逆です（先のタスクが終わる前に始まる予定）" : "") + "\nクリックで右に詳しく出す（そこで外せる）";
    group.append(title);
    if (state.selectedDep && state.selectedDep.pre === pre.id && state.selectedDep.post === post.id) group.classList.add("selected");
    // クリックでこのつながりを選び、右の欄に「つながり」を出す
    group.addEventListener("pointerdown", (ev) => ev.stopPropagation());
    group.addEventListener("click", (ev) => {
      ev.stopPropagation();
      select_dependency(pre.id, post.id);
    });
    layer.append(group);
  }
  grid.append(layer);
}

/// バーの端の外側の丸。ほかの行（線表・未分類）へドラッグして離すと、依存関係をつなぐ
function dep_dot(t, side) {
  const dot = el("span", {
    class: `dep_dot ${side}`,
    title: side === "right"
      ? "ドラッグして、ほかのバーの左の丸（前）につなぐ（このタスクが終わってから、そのタスク）"
      : "ドラッグして、ほかのバーの右の丸（後ろ）につなぐ（そのタスクが終わってから、このタスク）",
  });
  dot.addEventListener("pointerdown", (ev) => {
    if (ev.button !== 0) return;
    ev.stopPropagation(); // バーの移動にしない
    ev.preventDefault();
    const layer = document.querySelector("#gantt .dep_layer");
    const rect = dot.getBoundingClientRect();
    const lrect = layer?.getBoundingClientRect();
    const x0 = rect.left + rect.width / 2, y0 = rect.top + rect.height / 2;
    const temp = layer ? svg("path", { class: "dep_line temp" }) : null;
    if (temp) layer.append(temp);
    // つなげるのは「後ろ（右の丸）→ 前（左の丸）」だけ。運んでいる間は反対側の丸だけを出す
    const want = side === "right" ? "left" : "right";
    document.body.classList.add("dep_linking", `want_${want}`);
    dot.classList.add("dep_origin");
    let target = null;
    const dotAt = (e) => {
      const d = document.elementFromPoint(e.clientX, e.clientY)?.closest?.(`.dep_dot.${want}`);
      const row = d?.closest("[data-task_id]");
      return d && row && Number(row.dataset.task_id) !== t.id ? d : null;
    };
    const move = (e) => {
      const next = dotAt(e);
      if (temp) {
        // 反対側の丸の上では、線をその丸の中心にぴたりと付ける
        let [bx, by] = [e.clientX, e.clientY];
        if (next) { const r = next.getBoundingClientRect(); [bx, by] = [r.left + r.width / 2, r.top + r.height / 2]; }
        const [a, b] = [[x0, y0], [bx, by]].map(([x, y]) => [x - lrect.left, y - lrect.top]);
        temp.setAttribute("d", side === "right" ? curve_path(a[0], a[1], b[0], b[1]) : curve_path(b[0], b[1], a[0], a[1]));
      }
      if (next !== target) {
        target?.classList.remove("dep_dot_target");
        target?.closest("[data-task_id]")?.classList.remove("dep_target");
        next?.classList.add("dep_dot_target");
        next?.closest("[data-task_id]")?.classList.add("dep_target");
        target = next;
      }
    };
    const up = (e) => {
      document.removeEventListener("pointermove", move);
      document.removeEventListener("pointerup", up);
      document.body.classList.remove("dep_linking", `want_${want}`);
      dot.classList.remove("dep_origin");
      temp?.remove();
      target?.classList.remove("dep_dot_target");
      target?.closest("[data-task_id]")?.classList.remove("dep_target");
      const d = dotAt(e);
      if (!d) return;
      const id = Number(d.closest("[data-task_id]").dataset.task_id);
      if (side === "right") add_dependency(t.id, id);
      else add_dependency(id, t.id);
    };
    document.addEventListener("pointermove", move);
    document.addEventListener("pointerup", up);
  });
  return dot;
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
      (gantt.matched.has(t.id) || gantt.pinned.has(t.id) ? "" : " dim"),
    "data-task_id": t.id,
    style: `padding-left:${8 + depth * 16}px`,
    onclick: (ev) => select(t.id, ev),
  });
  const twisty = el("span", {
    class: "twisty" + (hasKids ? "" : " leaf"),
    onclick: (ev) => { ev.stopPropagation(); toggle_collapse(t.id); },
  }, state.collapsed.has(t.id) ? "▸" : "▾");
  // 完了のチェックボックスの代わりに、状態の印（表示だけ。状態は詳細欄で変える）
  const check = status_mark(t);
  const label = el("span", {
    class: "label",
    title: "ダブルクリックで名前を変更・ドラッグで線表へ移す",
    ondblclick: (ev) => { ev.stopPropagation(); start_inline_edit(label, t); },
  }, t.title);
  const dates = [];
  if (t.next_action_date && !done) dates.push(`▶ ${short_date(t.next_action_date)}`);
  if (t.planned_end) dates.push(`〜${short_date(t.planned_end)}`);
  row.append(twisty, check, label,
    ...pinned_note(t),
    ...kind_badge(t),
    ...estimate_badge(t),
    ...link_mark(t),
    ...tag_chips(t.tags, (n) => gantt.filter.tags.some((x) => same_tag(x, n)), toggle_gantt_tag, t.id),
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
      (gantt.matched.has(t.id) || gantt.pinned.has(t.id) ? "" : " dim"),
    "data-task_id": t.id,
    onclick: (ev) => select(t.id, ev),
  });

  // 名前欄：開閉・完了チェック・名前・タグ・操作
  const twisty = el("span", {
    class: "twisty" + (r.hasKids ? "" : " leaf"),
    onclick: (ev) => { ev.stopPropagation(); toggle_collapse(t.id); },
  }, state.collapsed.has(t.id) ? "▸" : "▾");
  // 完了のチェックボックスの代わりに、状態の印（表示だけ。状態は詳細欄で変える）
  const check = status_mark(t);
  const label = el("span", {
    class: "label",
    title: [...path_titles(t), t.title].join(" › ") + "\nダブルクリックで名前を変更・ドラッグで移動",
    ondblclick: (ev) => { ev.stopPropagation(); start_inline_edit(label, t); },
  }, t.title);
  const name = el("div", { class: "gantt_name", style: `padding-left:${8 + r.depth * 16}px` },
    twisty, check, label,
    ...pinned_note(t),
    ...kind_badge(t),
    ...estimate_badge(t),
    ...link_mark(t),
    ...tag_chips(t.tags, (n) => gantt.filter.tags.some((x) => same_tag(x, n)), toggle_gantt_tag, t.id),
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
    // 縮尺が小さいと 1 日のバーが細くなりすぎるので、最低 3px にする
    node.style.width = `${Math.max(3, (days_between(s2, e2) + 1) * GANTT_DAY_W - 2)}px`;
    track.append(node);
  };
  const mark = (cls, ch, d, title) => {
    if (d < start) { clipped.left = true; return; }
    if (d > end) { clipped.right = true; return; }
    // ◆・▶ は 1 日の幅が狭くても読める幅で、その日の中央に置く
    const w = Math.max(GANTT_DAY_W, 14);
    track.append(el("span", { class: "mark " + cls, style: `left:${x_of(d) + GANTT_DAY_W / 2 - w / 2}px;width:${w}px`, title }, ch));
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
      class: `bar own ${t.status}` + alertClass + (t.kind === "external" ? " other" : "") + ` load_${load_level(t)}` + (too_heavy(t) ? " too_heavy" : ""),
      title: tip + load_tip(t) + (overdue ? "\n終了予定日を過ぎています" : alerts.late_start ? "\n開始予定日を過ぎても未着手です" : "") +
        (done ? "\n完了したので日程は動かせません" : "\nドラッグで移動、端をドラッグで期間を変更"),
    });
    // 表示範囲で切れている側には端のつまみを付けない。完了したタスクは日程を動かせないので付けない
    if (!done && span.start >= start) bar.append(el("span", { class: "bar_handle left" }));
    if (!done && span.end <= end) bar.append(el("span", { class: "bar_handle right" }));
    // つまみの外側の丸：ほかの行へドラッグすると依存関係をつなぐ（右の丸から＝このタスクが先、左の丸から＝このタスクが後）
    // 完了したタスクの前（左の丸）にはつなげない（後ろにはつなげる）
    bar.append(dep_dot(t, "right"));
    if (!done) bar.append(dep_dot(t, "left"));
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
    place(el("div", { class: `bar ${t.status} open_end` + alertClass + (t.kind === "external" ? " other" : "") + ` load_${load_level(t)}`, title: tip + "\n終了予定日は未設定" + (alerts.late_start ? "\n開始予定日を過ぎています" : "") }), span.start, e);
  }
  if (t.next_action_date && !done) {
    mark("next" + (alerts.next_overdue ? " overdue" : ""), "▶", t.next_action_date,
      `次の予定 ${short_date(t.next_action_date)}${t.next_action ? " " + t.next_action : ""}${alerts.next_overdue ? "（過ぎています）" : ""}`);
  }
  if (clipped.left) track.append(el("span", { class: "clip_left", title: "表示範囲より前に予定あり" }, "«"));
  if (clipped.right) track.append(el("span", { class: "clip_right", title: "表示範囲より後に予定あり" }, "»"));
  // 完了したタスクは日程を動かせない（バーの移動・期間の変更・新しく決めるのもしない）
  if (done) {
    track.classList.add("locked");
  } else {
    if (!hasSpan) {
      track.classList.add("creatable");
      track.title = "横にドラッグして開始予定日と終了予定日を決める";
    }
    attach_gantt_drag(track, t, hasSpan ? span : null, ctx);
  }
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

    // 入力中の欄（詳細パネルなど）を先に確定させる。preventDefault で欄からフォーカスが外れず、
    // 変更が保存されないまま別のタスクに切り替わってしまうため
    if (document.activeElement instanceof HTMLElement && document.activeElement !== document.body) document.activeElement.blur();
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
        preview.style.width = `${Math.max(3, (days_between(s2, e2) + 1) * GANTT_DAY_W - 2)}px`;
      }
      label.textContent = `${short_date(s)} 〜 ${short_date(en)}（${days_between(s, en) + 1} 日）`;
      label.style.left = `${e.clientX + 14}px`;
      label.style.top = `${e.clientY - 30}px`;
    };

    let finished = false;
    const finish = (commit) => {
      if (finished) return; // pointerup のあとに lostpointercapture も来るので 1 回だけ
      finished = true;
      track.removeEventListener("pointermove", on_move);
      track.removeEventListener("lostpointercapture", on_lost);
      track.removeEventListener("pointerup", on_up);
      track.removeEventListener("pointercancel", on_cancel);
      document.body.classList.remove("gantt_dragging", "drag_" + mode);
      label.remove();
      preview?.remove();
      original?.classList.remove("drag_source");
      show_trash(null);
      if (ganttRenderPending) setTimeout(render_gantt, 0); // ドラッグ中に止めていた描き直し
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
    // ドラッグ中に線表が描き直される（定期の読み直し・幅の変化など）と、つかんでいた要素が消えて pointerup が届かない。
    // そのときは捕まえていたポインターが外れるので、それを合図に片付ける（日付の吹き出しが残らないように）
    const on_lost = () => finish(false);
    track.addEventListener("pointermove", on_move);
    track.addEventListener("pointerup", on_up);
    track.addEventListener("pointercancel", on_cancel);
    track.addEventListener("lostpointercapture", on_lost);
  });
}
