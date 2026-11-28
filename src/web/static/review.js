"use strict";
// 振り返りタブ。完了したタスクの見積もりと実績を並べ、ずれの理由（候補から選ぶ）と自由記述を残す。
// 日々の画面には何も足さず、時間のあるときにここでまとめて書く。書かなくても困らない。

const REVIEW_REASONS = [
  ["as_estimated", "見積もりどおり"],
  ["easier", "想定より簡単だった"],
  ["investigation", "調査・確認に時間がかかった"],
  ["spec_change", "仕様が変わった"],
  ["rework", "手戻りがあった"],
  ["interruption", "割り込み・待ちがあった"],
  ["missed", "作業が見積もりから漏れていた"],
];
const REVIEW_REASON_LABEL = Object.fromEntries(REVIEW_REASONS);

const review = {
  period: load_json("review_period", "this_month"), // this_month / last_month / last_3_months / all
  showUnestimated: false, // 見積もりのないタスクも出す
  unreviewedOnly: false, // 未記入だけ
  text: "", // キーワードの絞り込み（タスク名・場所・メモ）
  tags: [], // タグの絞り込み（いずれかを含む。親のタグは子にも効く）
  items: [],
  loadedKey: null,
  openId: null, // 選んでいるタスク（右の欄に出す）
};

const REVIEW_PERIODS = [
  ["this_month", "今月"],
  ["last_month", "先月"],
  ["last_3_months", "直近 3 か月"],
  ["all", "すべて"],
];

function review_range() {
  const today = today_text();
  const [y, m] = today.split("-").map(Number);
  const first = (yy, mm) => { const d = new Date(yy, mm - 1, 1); return `${d.getFullYear()}-${pad2(d.getMonth() + 1)}-01`; };
  const last = (yy, mm) => { const d = new Date(yy, mm, 0); return `${d.getFullYear()}-${pad2(d.getMonth() + 1)}-${pad2(d.getDate())}`; };
  switch (review.period) {
    case "last_month": return [first(y, m - 1), last(y, m - 1)];
    case "last_3_months": return [first(y, m - 2), today];
    case "all": return ["2000-01-01", today];
    default: return [first(y, m), today];
  }
}

async function load_reviews(force = false) {
  const [from, to] = review_range();
  const key = `${from}~${to}@${reloadCount}`;
  if (!force && review.loadedKey === key) return;
  review.loadedKey = key;
  const items = await api("GET", `/reviews?from=${from}&to=${to}`);
  if (review.loadedKey !== key) return;
  review.items = items;
  draw_review();
}

function render_review() {
  load_reviews();
  draw_review();
}

/// 実績 ÷ 最初の見積もり（見積もりがなければ null）
function review_ratio(it) {
  return it.first_estimate_min ? it.actual_min / it.first_estimate_min : null;
}

/// 倍率の見方：0.8〜1.25 倍は「ほぼどおり」
function ratio_class(r) {
  if (r === null) return "";
  if (r > 1.25) return "over";
  if (r < 0.8) return "under";
  return "ok";
}

/// 絞り込み（キーワード・タグ・見積もりなし・未記入）に合う行
function review_shown() {
  const q = review.text.trim().toLowerCase();
  return review.items
    .filter((it) => review.showUnestimated || it.first_estimate_min)
    .filter((it) => !review.unreviewedOnly || (!it.reasons.length && !it.note))
    .filter((it) => !q || [it.title, ...it.path, it.note || ""].some((v) => v.toLowerCase().includes(q)))
    .filter((it) => {
      if (!review.tags.length) return true;
      const t = state.byId.get(it.task_id);
      const tags = t ? inherited_tags(t) : [];
      return review.tags.some((n) => tags.some((x) => same_tag(x, n)));
    });
}

function draw_review() {
  $("review_periods").replaceChildren(...REVIEW_PERIODS.map(([key, label]) => el("button", {
    type: "button",
    class: "chip" + (review.period === key ? " active" : ""),
    onclick: () => { review.period = key; save_json("review_period", key); review.openId = null; load_reviews(true); draw_review(); },
  }, label)));
  $("review_unestimated").checked = review.showUnestimated;
  $("review_unreviewed").checked = review.unreviewedOnly;
  if (document.activeElement !== $("review_text")) $("review_text").value = review.text;
  review.tags = review.tags.filter((n) => state.tags.some((t) => same_tag(t.name, n)));
  $("review_tags").replaceChildren(...tag_filter_buttons(() => review.tags, (n) => { toggle_in(review.tags, n); draw_review(); }));

  // 上の数字は 1 行にまとめる（場所を取らないように）
  const estimated = review.items.filter((it) => it.first_estimate_min && it.actual_min > 0);
  const ok = estimated.filter((it) => ratio_class(review_ratio(it)) === "ok").length;
  const sumEst = estimated.reduce((a, it) => a + it.first_estimate_min, 0);
  const sumAct = estimated.reduce((a, it) => a + it.actual_min, 0);
  const unreviewed = review.items.filter((it) => !it.reasons.length && !it.note).length;
  $("review_summary").replaceChildren(
    el("span", { title: `うち見積もりと実績があるもの ${estimated.length} 件` }, `完了 ${review.items.length} 件`),
    el("span", { title: "実績が見積もりの 0.8〜1.25 倍だった割合" }, `見積もりどおり ${estimated.length ? Math.round((ok / estimated.length) * 100) + "%" : "—"}`),
    el("span", { title: "実績の合計 ÷ 最初の見積もりの合計" }, `実績 ÷ 見積もり ${sumEst ? (sumAct / sumEst).toFixed(2) + " 倍" : "—"}`),
    el("span", { title: "時間のあるときに" }, `未記入 ${unreviewed} 件`),
  );

  const shown = review_shown();
  if (review.openId !== null && !review.items.some((it) => it.task_id === review.openId)) review.openId = null;
  $("review_list").replaceChildren(...(shown.length ? shown.map(review_row) : [el("p", { class: "empty_hint" },
    review.items.length ? "条件に合うタスクはありません" : "この期間に完了したタスクはありません")]));
  draw_review_pane();
}

function review_row(it) {
  const r = review_ratio(it);
  const est = it.first_estimate_min ? fmt_hours(it.first_estimate_min) + (it.estimate_from_children ? "（子）" : "") : "—";
  const done = it.reasons.length || it.note;
  return el("div", {
    class: "review_row" + (review.openId === it.task_id ? " open" : ""),
    "data-review_id": it.task_id,
    onclick: () => select_review(it.task_id),
    title: "クリックで右に振り返りを出す（↑↓ で前後に移れる）",
  },
    el("span", { class: "date" }, short_date(it.completed_on)),
    el("span", { class: "name" }, el("span", { class: "title" }, it.title), el("span", { class: "crumb" }, it.path.join(" › "))),
    el("span", { class: "num" }, `${est} → ${it.actual_min ? fmt_hours(it.actual_min) : "—"}`),
    el("span", { class: "ratio " + ratio_class(r) }, r === null ? "" : `${r.toFixed(2)} 倍`),
    el("span", { class: "review_mark" + (done ? " done" : ""), title: done ? "記入済み" : "未記入" }, done ? "●" : "○"));
}

function select_review(id) {
  review.openId = id;
  document.querySelectorAll(".review_row.open").forEach((r) => r.classList.remove("open"));
  document.querySelector(`.review_row[data-review_id="${id}"]`)?.classList.add("open");
  draw_review_pane();
}

/// 右の欄：選んだタスクの、見積もりと実績の積み上げの棒・ずれの理由・自由記述（変えるとすぐ保存）
function draw_review_pane() {
  const it = review.items.find((x) => x.task_id === review.openId);
  $("review_pane_title").textContent = it ? `振り返り：${it.title}` : "振り返り";
  if (!it) {
    $("review_pane_body").replaceChildren(el("p", { class: "detail_empty" }, "タスクを選ぶと、ここに振り返りが出ます"));
    return;
  }
  const save = async (reasons, note) => {
    await api("PUT", `/tasks/${it.task_id}/review`, { reasons, note });
    it.reasons = reasons;
    it.note = note?.trim() || null;
    draw_review();
    toast("振り返りを保存しました（Ctrl+Z で元に戻す）", "faint");
  };
  const r = review_ratio(it);
  const note = el("textarea", { class: "review_note", rows: "4", placeholder: "気づいたこと（自由に。書かなくてもよい）" });
  note.value = it.note || "";
  note.addEventListener("blur", () => { if ((note.value.trim() || null) !== (it.note || null)) save(it.reasons, note.value); });
  $("review_pane_body").replaceChildren(el("div", { class: "review_pane_body" },
    el("div", { class: "review_pane_meta" },
      `${short_date(it.completed_on)} 完了` + (it.path.length ? `・${it.path.join(" › ")}` : "")),
    el("div", { class: "review_pane_numbers" },
      el("span", {}, `見積もり ${it.first_estimate_min ? fmt_hours(it.first_estimate_min) : "—"}${it.estimate_from_children ? "（子の合計）" : ""}` +
        (it.estimate_min && !it.estimate_from_children && it.estimate_min !== it.first_estimate_min ? `（今は ${fmt_hours(it.estimate_min)}）` : "")),
      el("span", {}, `実績 ${it.actual_min ? fmt_hours(it.actual_min) : "—"}`),
      r === null ? null : el("span", { class: "ratio " + ratio_class(r) }, `${r.toFixed(2)} 倍`)),
    it.breakdown.length ? review_stack(it) : null,
    el("span", { class: "field_label" }, "見積もりとのずれの理由（当てはまるものを押す・複数可）"),
    el("div", { class: "review_reasons" }, ...REVIEW_REASONS.map(([key, label]) => el("button", {
      type: "button",
      class: "chip" + (it.reasons.includes(key) ? " active" : ""),
      onclick: () => save(it.reasons.includes(key) ? it.reasons.filter((k) => k !== key) : [...it.reasons, key], note.value),
    }, label))),
    el("span", { class: "field_label" }, "気づいたこと"),
    note,
  ));
}

/// ↑↓ で、一覧の前後のタスクに移る
function review_move(delta) {
  const ids = review_shown().map((it) => it.task_id);
  if (!ids.length) return;
  const i = ids.indexOf(review.openId);
  const next = ids[Math.max(0, Math.min(ids.length - 1, i < 0 ? 0 : i + delta))];
  select_review(next);
  document.querySelector(`.review_row[data-review_id="${next}"]`)?.scrollIntoView({ block: "nearest" });
}

/// 内訳の積み上げの棒：左が見積もり、右が実績。子タスクごとに色を分け、横の表に要素ごとの差を出す
function review_stack(it) {
  const parts = it.breakdown.map((b, i) => ({ title: b.title, est: b.estimate_min || 0, act: b.actual_min, color: `hsl(${(i * 67 + 210) % 360} 55% 62%)` }));
  if (it.own_actual_min) parts.push({ title: "（親に直接）", est: 0, act: it.own_actual_min, color: "var(--muted)" });
  const sumEst = parts.reduce((a, p) => a + p.est, 0);
  const sumAct = parts.reduce((a, p) => a + p.act, 0);
  // 親に直接入れた見積もりが子の合計と違うときは、見積もりの棒に点線で示す
  const parentEst = !it.estimate_from_children && it.first_estimate_min && it.first_estimate_min !== sumEst ? it.first_estimate_min : null;
  const max = Math.max(sumEst, sumAct, parentEst || 0, 1);
  const H = 160;
  const column = (label, key, total) => {
    const bar = el("div", { class: "stack_bar" });
    for (const p of parts) {
      if (!p[key]) continue;
      bar.append(el("div", {
        class: "stack_seg",
        style: `height:${(p[key] / max) * H}px;background:${p.color}`,
        title: `${p.title} ${fmt_hours(p[key])}`,
      }));
    }
    const area = el("div", { class: "stack_area", style: `height:${H}px` }, bar);
    if (key === "est" && parentEst) area.append(el("div", { class: "stack_mark", style: `bottom:${(parentEst / max) * H}px`, title: `親に入れた見積もり ${fmt_hours(parentEst)}` }));
    return el("div", { class: "stack_col" },
      el("div", { class: "stack_total" }, total ? fmt_hours(total) : "—"),
      area,
      el("div", { class: "stack_label" }, label));
  };
  const diff = (p) => {
    if (!p.est) return el("span", { class: "diff" }, p.act ? "見積もりなし" : "");
    const d = p.act - p.est;
    return el("span", { class: "diff " + (d > p.est * 0.25 ? "over" : d < -p.est * 0.2 ? "under" : "ok") }, `${d >= 0 ? "+" : "−"}${fmt_hours(Math.abs(d))}`);
  };
  return el("div", { class: "review_stack" },
    column("見積もり", "est", sumEst),
    column("実績", "act", sumAct),
    el("table", { class: "stack_table" },
      el("thead", {}, el("tr", {}, el("th", {}, ""), el("th", {}, "要素"), el("th", {}, "見積もり"), el("th", {}, "実績"), el("th", {}, "差"))),
      el("tbody", {}, ...parts.map((p) => el("tr", {},
        el("td", {}, el("span", { class: "swatch", style: `background:${p.color}` })),
        el("td", {}, p.title),
        el("td", { class: "num" }, p.est ? fmt_hours(p.est) : "—"),
        el("td", { class: "num" }, p.act ? fmt_hours(p.act) : "—"),
        el("td", { class: "num" }, diff(p))))),
      parentEst ? el("tfoot", {}, el("tr", {}, el("td", { colspan: "5", class: "note" }, `点線は親に入れた見積もり（${fmt_hours(parentEst)}）`))) : null));
}

/// 期間内の振り返りを CSV で書き出す（Excel で開けるよう BOM 付きの UTF-8）
function export_review_csv() {
  const [from, to] = review_range();
  const h = (m) => (m ? (m / 60).toFixed(2) : "");
  const esc = (v) => `"${String(v ?? "").replace(/"/g, '""')}"`;
  const lines = [["完了日", "タスク", "場所", "最初の見積もり(h)", "今の見積もり(h)", "実績(h)", "実績÷最初の見積もり", "内訳", "理由", "メモ"].map(esc).join(",")];
  for (const it of review.items) {
    const r = review_ratio(it);
    lines.push([
      it.completed_on, it.title, it.path.join(" > "), h(it.first_estimate_min), h(it.estimate_min), h(it.actual_min),
      r === null ? "" : r.toFixed(2),
      it.breakdown.map((b) => `${b.title} ${h(b.actual_min)}h`).join(" / "),
      it.reasons.map((k) => REVIEW_REASON_LABEL[k]).join(" / "), it.note,
    ].map(esc).join(","));
  }
  const blob = new Blob(["﻿" + lines.join("\r\n")], { type: "text/csv" });
  const a = el("a", { href: URL.createObjectURL(blob), download: `振り返り_${from}_${to}.csv` });
  document.body.append(a);
  a.click();
  a.remove();
  setTimeout(() => URL.revokeObjectURL(a.href), 1000);
}

function setup_review() {
  $("review_unestimated").addEventListener("change", (ev) => { review.showUnestimated = ev.target.checked; draw_review(); });
  $("review_unreviewed").addEventListener("change", (ev) => { review.unreviewedOnly = ev.target.checked; draw_review(); });
  $("review_csv").addEventListener("click", export_review_csv);
  let timer;
  $("review_text").addEventListener("input", (ev) => {
    clearTimeout(timer);
    timer = setTimeout(() => { review.text = ev.target.value; draw_review(); }, 150);
  });
  document.addEventListener("keydown", (ev) => {
    if (state.tab !== "review" || is_text_entry(document.activeElement) || ev.ctrlKey || ev.metaKey || ev.altKey) return;
    if (ev.key === "ArrowDown" || ev.key === "ArrowUp") {
      ev.preventDefault();
      review_move(ev.key === "ArrowDown" ? 1 : -1);
    }
  });
}
