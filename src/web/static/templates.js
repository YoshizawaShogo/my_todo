"use strict";
// テンプレート：選んだタスク（子を含む）を型として保存し、別の機会に開始日を選んで作り直す。
// 日付は「一番早い日付から何日目か」で保存されるので、開始日を選ぶと同じ間隔のままずれた日程で作られる。
// データベースの中に保存するので、Ctrl+Z で戻せて、データベースごとに別になる。

const tpl = { list: [], open: false };

function toggle_template_panel(open = !tpl.open) {
  tpl.open = open;
  $("template_panel").hidden = !open;
  if (open) {
    place_template_panel();
    if (!$("template_start").value) $("template_start").value = today_text();
    draw_template_panel();
    load_templates();
  }
}

/// ボタンは線表の左上（描き直されるうえ、線表の中はスクロールで切れる）ので、窓は画面に固定してボタンの下に出す
function place_template_panel() {
  const b = $("template_button")?.getBoundingClientRect();
  if (!b) return toggle_template_panel(false);
  const panel = $("template_panel");
  panel.style.top = `${b.bottom + 6}px`;
  panel.style.left = `${Math.max(16, Math.min(b.left, window.innerWidth - panel.offsetWidth - 16))}px`;
}

async function load_templates() {
  tpl.list = await api("GET", "/templates");
  if (tpl.open) draw_template_panel();
}

function draw_template_panel() {
  const sel = visible_task_order().filter((id) => state.selected.has(id));
  $("template_save").disabled = sel.length === 0;
  $("template_save").title = sel.length
    ? `選んだ ${sel.length} 件（子を含む）をテンプレートにする`
    : "線表か未分類でタスクを選ぶと押せます（Shift・Ctrl で複数）";
  $("template_list").replaceChildren(
    tpl.list.length
      ? el("ul", { class: "template_items" }, ...tpl.list.map((t) => el("li", {},
        el("div", { class: "template_info" },
          el("span", { class: "name" }, t.name),
          el("span", { class: "meta" }, `${t.item_count} 件・${!t.span_days ? "日付なし" : t.offset_unit === "workday" ? `平日 ${t.span_days} 日` : `${t.span_days} 日間`}`)),
        el("button", { type: "button", class: "small_button primary", title: "開始日からタスクを作る", onclick: () => apply_template(t) }, "使う"),
        el("button", { type: "button", class: "small_button", onclick: () => rename_template(t) }, "名前を変える"),
        el("button", { type: "button", class: "small_button danger", onclick: () => delete_template(t) }, "削除"))))
      : el("p", { class: "empty_hint" }, "保存したテンプレートはまだありません。上の「保存する」で作れます"),
  );
}

async function save_template() {
  const ids = visible_task_order().filter((id) => state.selected.has(id));
  if (!ids.length) return;
  const name = prompt("テンプレートの名前", state.byId.get(ids[0])?.title ?? "");
  if (name === null || !name.trim()) return;
  const t = await api("POST", "/templates", { name, ids });
  await load_templates();
  toast(`テンプレート『${t.name}』を保存しました（${t.item_count} 件）`);
}

/// 置き場所：選んでいるタスク（画面で一番下のもの）の後ろ。選んでいなければ線表の最上位の末尾
function template_placement() {
  const sel = visible_task_order().filter((id) => state.selected.has(id));
  return sel.length ? { kind: "after", ref_id: sel[sel.length - 1] } : { kind: "into", parent_id: null, inbox: false };
}

async function apply_template(t) {
  const start_date = $("template_start").value || today_text();
  const r = await api("POST", `/templates/${t.id}/apply`, { start_date, placement: template_placement() });
  toggle_template_panel(false);
  await reload();
  toast(`テンプレート『${t.name}』から ${r.created} 件作りました（Ctrl+Z で元に戻す）`);
}

async function rename_template(t) {
  const name = prompt("テンプレートの新しい名前", t.name);
  if (name === null || !name.trim() || name.trim() === t.name) return;
  await api("PATCH", `/templates/${t.id}`, { name });
  await load_templates();
}

async function delete_template(t) {
  if (!confirm(`🗑️ テンプレート『${t.name}』を削除しますか？\n（作ったタスクは消えません。Ctrl+Z で元に戻せます）`)) return;
  await api("DELETE", `/templates/${t.id}`);
  await load_templates();
  toast(`🗑️ テンプレート『${t.name}』を削除しました（Ctrl+Z で元に戻す）`);
}

function setup_templates() {
  $("template_save").addEventListener("click", () => save_template());
  $("template_panel").addEventListener("click", (ev) => ev.stopPropagation());
  document.addEventListener("click", (ev) => {
    if (!tpl.open) return;
    // 開始日のカレンダーの中のクリックでは閉じない。カレンダーは日を選ぶ・月を送るとすぐ描き直すので、
    // 押したボタンはもう画面にない（ev.target から親をたどれない）。押した時点の経路で判定する
    if (datepicker.popup && ev.composedPath().includes(datepicker.popup)) return;
    toggle_template_panel(false);
  });
  window.addEventListener("resize", () => { if (tpl.open) place_template_panel(); });
  document.addEventListener("scroll", (ev) => {
    if (tpl.open && !$("template_panel").contains(ev.target)) place_template_panel();
  }, true);
  document.addEventListener("keydown", (ev) => {
    if (ev.key === "Escape" && tpl.open && !datepicker.input) toggle_template_panel(false);
  });
}
