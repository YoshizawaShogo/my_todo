"use strict";
// 日付の入力欄と、カレンダーの吹き出し。
// ブラウザ標準の日付選択は見出しが長く折り返して崩れるため、自前で描く。
// 入力欄の value は常に YYYY-MM-DD（空なら未設定）。確定したら change を送る。

const datepicker = {
  popup: null,
  input: null, // 開いている入力欄
  month: null, // 表示中の月（YYYY-MM-01）
};

/// 文字の入力を YYYY-MM-DD に直す。空なら ""、読めなければ null
/// 受け付ける形：2026-11-25 / 2026/11/25 / 11/25 / 11-25 / 1125 / 今日 / 明日 / 昨日 / +3 / -2
function parse_date_text(text, base = today_text()) {
  const s = text.trim().replace(/[０-９]/g, (c) => String.fromCharCode(c.charCodeAt(0) - 0xfee0)).replace(/／/g, "/");
  if (!s) return "";
  if (s === "今日" || s.toLowerCase() === "today") return base;
  if (s === "明日") return add_days(base, 1);
  if (s === "昨日") return add_days(base, -1);
  let m = s.match(/^([+-])(\d+)$/);
  if (m) return add_days(base, (m[1] === "-" ? -1 : 1) * Number(m[2]));
  const year = Number(base.slice(0, 4));
  let y, mo, d;
  if ((m = s.match(/^(\d{4})[-/.](\d{1,2})[-/.](\d{1,2})$/))) [y, mo, d] = [m[1], m[2], m[3]].map(Number);
  else if ((m = s.match(/^(\d{1,2})[-/.](\d{1,2})$/))) [y, mo, d] = [year, Number(m[1]), Number(m[2])];
  else if ((m = s.match(/^(\d{2})(\d{2})$/))) [y, mo, d] = [year, Number(m[1]), Number(m[2])];
  else if ((m = s.match(/^(\d{4})(\d{2})(\d{2})$/))) [y, mo, d] = [m[1], m[2], m[3]].map(Number);
  else return null;
  const dt = new Date(y, mo - 1, d);
  if (dt.getFullYear() !== y || dt.getMonth() !== mo - 1 || dt.getDate() !== d) return null; // 2/30 など
  return `${y}-${pad2(mo)}-${pad2(d)}`;
}

/// input[type=date] を自前の日付入力に置き換える
function setup_date_inputs() {
  for (const input of document.querySelectorAll('input[type="date"]')) {
    input.type = "text";
    input.classList.add("date_input");
    input.autocomplete = "off";
    if (!input.placeholder) input.placeholder = "例 11/25";
    if (!input.title) input.title = "クリックでカレンダー。文字でも入れられます（11/25、2026-11-25、今日、明日、+3）";
    input.dataset.lastValid = input.value;
    // ほかの change の処理より先に、文字の入力を YYYY-MM-DD に直す（読めなければ元に戻して止める）
    input.addEventListener("change", (ev) => {
      if (ev.isTrusted === false && input.dataset.fromPicker) return;
      const v = parse_date_text(input.value);
      if (v === null) {
        ev.stopImmediatePropagation();
        toast("日付は 11/25、2026-11-25、明日、+3 のように入れてください", "error");
        input.value = input.dataset.lastValid || "";
        return;
      }
      input.value = v;
      input.dataset.lastValid = v;
    }, true);
    input.addEventListener("focus", () => open_datepicker(input));
    // Tab などで欄からフォーカスが外れたら閉じる（吹き出しの中を押してもフォーカスは外れない）。
    // 開いたままだと下の欄に重なり、そこを押したつもりで日付が変わってしまう
    input.addEventListener("blur", () => {
      setTimeout(() => {
        if (datepicker.input === input && document.activeElement !== input) close_datepicker();
      }, 0);
    });
    input.addEventListener("click", () => open_datepicker(input));
    input.addEventListener("keydown", (ev) => {
      if (ev.key === "Escape") close_datepicker();
      if (ev.key === "Enter" && !ev.isComposing) {
        ev.preventDefault();
        input.blur();
      }
    });
    // 外から value を書き換えられたときにも「最後の正しい値」を追う
    const desc = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value");
    Object.defineProperty(input, "value", {
      get() { return desc.get.call(this); },
      set(v) {
        desc.set.call(this, v);
        if (v === "" || /^\d{4}-\d{2}-\d{2}$/.test(v)) this.dataset.lastValid = v;
        if (datepicker.input === this) draw_datepicker();
      },
    });
  }
  document.addEventListener("mousedown", (ev) => {
    if (!datepicker.popup || datepicker.popup.hidden) return;
    if (ev.target === datepicker.input || datepicker.popup.contains(ev.target)) return;
    close_datepicker();
  });
  window.addEventListener("resize", close_datepicker);
  document.addEventListener("scroll", (ev) => {
    if (datepicker.popup && !datepicker.popup.contains(ev.target)) place_datepicker();
  }, true);
}

function open_datepicker(input) {
  if (!datepicker.popup) {
    datepicker.popup = el("div", { class: "datepicker", hidden: true });
    // 吹き出しの中をクリックしても入力欄からフォーカスを外さない
    datepicker.popup.addEventListener("mousedown", (ev) => ev.preventDefault());
    document.body.append(datepicker.popup);
  }
  datepicker.input = input;
  const v = /^\d{4}-\d{2}-\d{2}$/.test(input.value) ? input.value : today_text();
  datepicker.month = v.slice(0, 8) + "01";
  datepicker.popup.hidden = false;
  draw_datepicker();
  place_datepicker();
}

function close_datepicker() {
  if (datepicker.popup) datepicker.popup.hidden = true;
  datepicker.input = null;
}

function place_datepicker() {
  const { popup, input } = datepicker;
  if (!popup || popup.hidden || !input) return;
  const r = input.getBoundingClientRect();
  const h = popup.offsetHeight, w = popup.offsetWidth;
  // 下に入らなければ上に出す。右にはみ出すなら左へ寄せる
  const top = r.bottom + 4 + h > window.innerHeight && r.top - 4 - h > 0 ? r.top - 4 - h : r.bottom + 4;
  const left = Math.max(8, Math.min(r.left, window.innerWidth - w - 8));
  popup.style.top = `${top}px`;
  popup.style.left = `${left}px`;
}

/// 選んだ日を入力欄に入れて change を送る
function pick_date(v) {
  const input = datepicker.input;
  if (!input) return;
  input.value = v;
  input.dataset.fromPicker = "1";
  input.dispatchEvent(new Event("change", { bubbles: true }));
  delete input.dataset.fromPicker;
  close_datepicker();
  input.blur();
}

function draw_datepicker() {
  const { popup, input } = datepicker;
  if (!popup || !input) return;
  const month = datepicker.month;
  const [y, m] = month.split("-").map(Number);
  const today = today_text();
  const selected = input.value;
  const shift = (n) => {
    const d = new Date(y, m - 1 + n, 1);
    datepicker.month = `${d.getFullYear()}-${pad2(d.getMonth() + 1)}-01`;
    draw_datepicker();
  };
  const header = el("div", { class: "dp_header" },
    el("button", { type: "button", class: "dp_nav", title: "前の月", onclick: () => shift(-1) }, "◀"),
    el("span", { class: "dp_title" }, `${y}年${m}月`),
    el("button", { type: "button", class: "dp_nav", title: "次の月", onclick: () => shift(1) }, "▶"));

  // 月曜始まり（アプリの「週」と同じ）
  const grid = el("div", { class: "dp_grid" });
  ["月", "火", "水", "木", "金", "土", "日"].forEach((w, i) =>
    grid.append(el("span", { class: "dp_wd" + (i === 5 ? " sat" : i === 6 ? " sun" : "") }, w)));
  const first = monday_of(month);
  for (let i = 0; i < 42; i++) {
    const d = add_days(first, i);
    const wd = weekday_of(d);
    grid.append(el("button", {
      type: "button",
      class: "dp_day" + (d.slice(0, 7) !== month.slice(0, 7) ? " other" : "") + (d === today ? " today" : "") +
        (d === selected ? " selected" : "") + (wd === 6 ? " sat" : wd === 0 ? " sun" : ""),
      title: short_date(d),
      onclick: () => pick_date(d),
    }, String(Number(d.slice(8)))));
  }
  const footer = el("div", { class: "dp_footer" },
    el("button", { type: "button", class: "small_button", onclick: () => pick_date("") }, "クリア"),
    el("button", { type: "button", class: "small_button", onclick: () => pick_date(today) }, "今日"));
  popup.replaceChildren(header, grid, footer);
}
