"use strict";
// Jira タブ。接続の設定、週ごとの送信（下書き → 送信）、紐付けたタスクの一覧。

const jira = {
  config: null, // {base_url, token_source}
  week: monday_of(today_text()),
  draft: null,
  draftKey: null,
  results: null, // 直前の送信結果
  issues: new Map(), // 課題キー → {summary, status} または {error}
};

const JIRA_ACTION = { create: "新規", update: "更新", delete: "削除", unchanged: "送信済み" };

async function load_jira_config() {
  jira.config = await api("GET", "/jira");
  return jira.config;
}

function jira_browse_url(key) {
  return jira.config?.base_url ? `${jira.config.base_url}/browse/${encodeURIComponent(key)}` : null;
}

/// 課題キーを Jira へのリンクにする（URL 未設定なら文字だけ）
function jira_key_link(key) {
  const url = jira_browse_url(key);
  return url
    ? el("a", { href: url, target: "_blank", rel: "noopener", class: "jira_key", title: "Jira で開く" }, key)
    : el("span", { class: "jira_key" }, key);
}

// ---------- 描画 ----------
function render_jira() {
  if (!jira.config) {
    load_jira_config().then(render_jira);
    return;
  }
  const c = jira.config;
  const url = $("jira_url");
  if (document.activeElement !== url) url.value = c.base_url || "";
  const src = c.token_source === "env" ? "環境変数 MY_TODO_JIRA_TOKEN から読み込み中"
    : c.token_source === "keyring" ? "OS の資格情報に保存済み"
    : "未設定";
  $("jira_status").replaceChildren(
    el("span", { class: "jira_badge " + (c.base_url ? "ok" : "ng") }, c.base_url ? "URL 設定済み" : "URL 未設定"),
    el("span", { class: "jira_badge " + (c.token_source ? "ok" : "ng") }, `PAT：${src}`),
    el("span", { id: "jira_test_result" }, jira.testResult || ""));
  $("jira_token_delete").hidden = c.token_source !== "keyring";

  const from = jira.week, to = add_days(jira.week, 6);
  $("jira_range").textContent = `${short_date(from)} 〜 ${short_date(to)}`;
  const key = `${from}@${reloadCount}`;
  if (jira.draftKey !== key) {
    jira.draftKey = key;
    api("GET", `/jira/draft?from=${from}&to=${to}`).then((d) => {
      if (jira.draftKey !== key) return;
      jira.draft = d;
      draw_jira_draft();
    });
  } else {
    draw_jira_draft();
  }
  draw_jira_links();
}

function draw_jira_draft() {
  const d = jira.draft;
  if (!d) return;
  const box = $("jira_draft");
  const pending = d.lines.filter((l) => l.action !== "unchanged");
  $("jira_send").disabled = pending.length === 0;
  $("jira_send").textContent = pending.length ? `Jira に送信（${pending.length} 件）` : "送るものはありません";

  const result = (l) => jira.results?.find((r) => r.work_date === l.work_date && r.issue_key === l.issue_key);
  const rows = d.lines.map((l) => {
    const r = result(l);
    return el("tr", { class: "jira_line " + l.action },
      el("td", {}, short_date(l.work_date)),
      el("td", {}, jira_key_link(l.issue_key)),
      el("td", { class: "jira_tasks" }, l.tasks.join("、")),
      el("td", { class: "num" }, l.minutes ? fmt_hours(l.minutes) : "0h"),
      el("td", { class: "num muted" }, l.sent_minutes === null ? "—" : fmt_hours(l.sent_minutes)),
      el("td", {}, el("span", { class: "action_badge " + l.action }, JIRA_ACTION[l.action])),
      el("td", { class: "jira_result" }, r ? (r.error ? el("span", { class: "error" }, r.error) : el("span", { class: "ok" }, "送信しました")) : ""));
  });
  const table = el("table", { class: "list_table jira_table" },
    el("thead", {}, el("tr", {},
      el("th", {}, "日付"), el("th", {}, "課題"), el("th", {}, "まとめたタスク"),
      el("th", { class: "num" }, "送る時間"), el("th", { class: "num" }, "送信済み"), el("th", {}, "操作"), el("th", {}, ""))),
    el("tbody", {}, ...(rows.length ? rows : [el("tr", {}, el("td", { colspan: 7, class: "empty_hint" },
      "この週に、Jira 課題を紐付けたタスクの工数はありません"))])));

  const parts = [table];
  if (d.unlinked.length) {
    const total = d.unlinked.reduce((n, u) => n + u.minutes, 0);
    parts.push(el("details", { class: "jira_unlinked" },
      el("summary", {}, `Jira 課題に紐付いていない工数 ${fmt_hours(total)}（送られません）`),
      el("ul", {}, ...d.unlinked.map((u) => el("li", {},
        `${short_date(u.work_date)} ${fmt_hours(u.minutes)} `,
        el("a", { href: "#", onclick: (ev) => { ev.preventDefault(); select(u.task_id); } }, u.task))))));
  }
  parts.push(el("p", { class: "empty_hint" },
    "工数記録タブで振り分けた時間を、日ごと・課題ごとにまとめて送ります。課題キーのない子タスクの時間は、紐付けた一番近い親の課題に入ります。送った後に工数を変えたら、もう一度送ると Jira 側も更新・削除されます。"));
  box.replaceChildren(...parts);
}

function draw_jira_links() {
  const linked = state.tasks.filter((t) => t.jira_key);
  const box = $("jira_links");
  if (!linked.length) {
    box.replaceChildren(el("p", { class: "empty_hint" }, "まだありません。右の詳細パネルの「Jira 課題」に ABC-123 のように入れると紐付きます。"));
    return;
  }
  const rows = linked.map((t) => {
    const info = jira.issues.get(t.jira_key);
    return el("tr", { class: "list_row" + (state.selectedId === t.id ? " selected" : ""), "data-task_id": t.id, onclick: () => select(t.id) },
      el("td", {}, jira_key_link(t.jira_key)),
      el("td", { class: "muted" }, info ? (info.error ? el("span", { class: "error" }, info.error) : `${info.summary}（${info.status}）`) : ""),
      el("td", {}, [...path_titles(t), t.title].join(" › ")),
      el("td", { class: "num" }, subtree_actual(t) ? fmt_hours(subtree_actual(t)) : ""));
  });
  box.replaceChildren(
    el("table", { class: "list_table" },
      el("thead", {}, el("tr", {}, el("th", {}, "課題"), el("th", {}, "Jira 側の件名"), el("th", {}, "タスク"), el("th", { class: "num" }, "記録した工数"))),
      el("tbody", {}, ...rows)),
    el("div", { class: "jira_buttons" },
      el("button", { type: "button", class: "small_button", onclick: fetch_issue_titles }, "Jira 側の件名を確認")));
}

async function fetch_issue_titles() {
  const keys = [...new Set(state.tasks.filter((t) => t.jira_key).map((t) => t.jira_key))];
  for (const k of keys) {
    try {
      const res = await fetch(`/api/jira/issues/${encodeURIComponent(k)}`);
      const body = await res.json();
      jira.issues.set(k, res.ok ? body : { error: body.error || res.statusText });
    } catch (e) {
      jira.issues.set(k, { error: String(e) });
    }
    draw_jira_links();
  }
}

// ---------- 操作 ----------
function setup_jira() {
  $("jira_save").addEventListener("click", async () => {
    const body = { base_url: $("jira_url").value };
    const token = $("jira_token").value;
    if (token) body.token = token;
    jira.config = await api("PATCH", "/jira", body);
    $("jira_token").value = "";
    jira.testResult = "保存しました";
    render();
  });
  $("jira_token_delete").addEventListener("click", async () => {
    if (!confirm("OS の資格情報から PAT を削除しますか？")) return;
    jira.config = await api("PATCH", "/jira", { token: "" });
    jira.testResult = "PAT を削除しました";
    render();
  });
  $("jira_test").addEventListener("click", async () => {
    jira.testResult = "接続中…";
    render();
    try {
      const me = await api("POST", "/jira/test");
      jira.testResult = `✅ ${me.display_name || me.name} さんとして接続できました`;
    } catch (e) {
      jira.testResult = `接続できませんでした：${e.message}`;
    }
    render();
  });
  $("jira_prev").addEventListener("click", () => { jira.week = add_days(jira.week, -7); jira.results = null; render(); });
  $("jira_next").addEventListener("click", () => { jira.week = add_days(jira.week, 7); jira.results = null; render(); });
  $("jira_this_week").addEventListener("click", () => { jira.week = monday_of(today_text()); jira.results = null; render(); });
  $("jira_send").addEventListener("click", async () => {
    const pending = jira.draft?.lines.filter((l) => l.action !== "unchanged") || [];
    if (!pending.length) return;
    const summary = pending.map((l) => `${short_date(l.work_date)} ${l.issue_key} ${JIRA_ACTION[l.action]} ${fmt_hours(l.minutes)}`).join("\n");
    if (!confirm(`次の ${pending.length} 件を Jira に送ります。\n\n${summary}`)) return;
    $("jira_send").disabled = true;
    try {
      jira.results = await api("POST", "/jira/send", { from: jira.week, to: add_days(jira.week, 6) });
      const failed = jira.results.filter((r) => r.error).length;
      toast(failed ? `${jira.results.length - failed} 件送信、${failed} 件失敗しました` : `✅ ${jira.results.length} 件を Jira に送信しました`, failed ? "error" : "info");
    } finally {
      jira.draftKey = null;
      render();
    }
  });
}
