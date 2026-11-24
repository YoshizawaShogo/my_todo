"use strict";
// 開いているデータベースの切り替え。ヘッダーのボタンから、最近開いたファイルか場所の入力で
// 別のデータベースを開ける（ない場所なら新しく作るか聞く）。開いたファイルはユーザーの設定ファイルに記録される。

const dbs = { info: null, open: false };

async function load_databases() {
  dbs.info = await api("GET", "/databases");
  draw_db_button();
  if (dbs.open) draw_db_panel();
}

function draw_db_button() {
  const cur = dbs.info?.current;
  $("db_button").textContent = cur ? `📂 ${cur.name}` : "📂";
  $("db_button").title = cur ? `開いているデータベース：${cur.path}\nクリックで切り替え` : "";
}

function toggle_db_panel(open = !dbs.open) {
  dbs.open = open;
  $("db_panel").hidden = !open;
  if (open) {
    draw_db_panel();
    load_databases();
  }
}

function draw_db_panel() {
  const info = dbs.info;
  if (!info) return;
  // 描き直しても、入力中の場所とフォーカスは残す
  const old = document.querySelector(".db_path_input");
  const input = el("input", { type: "text", class: "db_path_input", placeholder: "ファイルかフォルダの場所（例 C:\\Users\\me\\my_todo\\my_todo.db）" });
  if (old) input.value = old.value;
  const hadFocus = old && document.activeElement === old;
  input.addEventListener("keydown", (ev) => {
    if (ev.key === "Enter" && !ev.isComposing) open_database(input.value);
    if (ev.key === "Escape") toggle_db_panel(false);
  });
  const recent = info.recent.filter((r) => !r.current);
  $("db_panel").replaceChildren(
    el("div", { class: "db_current" },
      el("div", { class: "db_label" }, "開いているデータベース"),
      el("div", { class: "db_path" }, info.current.path)),
    el("div", { class: "db_label" }, "最近開いたファイル"),
    recent.length
      ? el("ul", { class: "db_recent" }, ...recent.map((r) => el("li", { class: r.exists ? "" : "missing" },
        el("button", {
          type: "button",
          class: "db_recent_open",
          title: r.exists ? `${r.path} を開く` : "ファイルが見つかりません（移動・削除されたかもしれません）",
          disabled: !r.exists,
          onclick: () => open_database(r.path),
        }, el("span", { class: "name" }, r.name), el("span", { class: "path" }, r.path)),
        el("button", {
          type: "button",
          class: "icon_button",
          title: "一覧から外す（ファイルは消えません）",
          onclick: async () => {
            dbs.info = await api("POST", "/databases/forget", { path: r.path });
            draw_db_panel();
          },
        }, "×"))))
      : el("p", { class: "empty_hint" }, "ほかに開いたファイルはありません"),
    ...(info.can_pick ? [
      el("div", { class: "db_label" }, "エクスプローラーで選ぶ"),
      el("div", { class: "db_open_row" },
        el("button", {
          type: "button",
          class: "small_button",
          title: "ファイルを開く画面で選ぶ（ないファイル名を入れると新しく作れます）",
          onclick: pick_database,
        }, "📂 ファイルを選んで開く…")),
    ] : []),
    el("div", { class: "db_label" }, "場所を指定して開く"),
    el("div", { class: "db_open_row" },
      input,
      el("button", { type: "button", class: "small_button primary", onclick: () => open_database(input.value) }, "開く")),
  );
  if (hadFocus) input.focus();
}

/// エクスプローラーの画面（Windows のファイルを開くダイアログ）で選んで開く
async function pick_database() {
  toast("ファイルを選ぶ画面を開きました（見当たらなければタスクバーを確認してください）");
  const r = await api("POST", "/databases/pick");
  if (r.path) await open_database(r.path);
}

/// 別のデータベースに切り替える。ファイルがなければ、新しく作るか聞く
async function open_database(path, create = false) {
  path = (path || "").trim();
  if (!path) return;
  const res = await fetch("/api/databases/open", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ path, create }),
  });
  const body = await res.json().catch(() => ({}));
  if (!res.ok) {
    if (body.missing && confirm(`${body.path} はまだありません。新しいデータベースとして作りますか？`)) {
      return open_database(body.path, true);
    }
    if (!body.missing) toast(body.error || res.statusText, "error");
    return;
  }
  dbs.info = body;
  toggle_db_panel(false);
  after_database_switch();
  toast(`📂 ${body.current.name} を開きました`);
}

/// データベースが変わったので、画面の状態をすべて読み直す
function after_database_switch() {
  state.selectedId = null;
  state.selected = new Set();
  state.anchorId = null;
  // 切り取り・コピーと折りたたみはタスクの id で覚えている。別のデータベースでは同じ id が別のタスクなので忘れる
  clip.mode = null;
  clip.ids = [];
  clip.text = "";
  state.collapsed = new Set();
  gantt.pinned.clear();
  save_json("collapsed", []);
  tpl.list = [];
  if (tpl.open) toggle_template_panel(false);
  pendingAdd = null;
  jira.config = null;
  jira.draft = null;
  jira.draftKey = null;
  jira.results = null;
  jira.issues = null;
  jira.issuesError = null;
  jira.pickedTaskId = null;
  work.day = null;
  work.rows = [];
  work.loadedKey = null;
  helpAbout = null;
  list.filter.parentId = null;
  draw_db_button();
  load_jira_config().then(() => reload());
}

function setup_databases() {
  $("db_button").addEventListener("click", (ev) => {
    ev.stopPropagation();
    toggle_db_panel();
  });
  $("db_panel").addEventListener("click", (ev) => ev.stopPropagation());
  document.addEventListener("click", () => { if (dbs.open) toggle_db_panel(false); });
  load_databases();
}
