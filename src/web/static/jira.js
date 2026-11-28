"use strict";
// Jira タブ。接続の設定、紐付けとインポート（左にこのアプリのタスク、右に Jira の課題）、
// エクスポート（まだ送っていない工数をまとめて送る）。このアプリがマスターで、Jira からは課題を取り込むだけ。

const JIRA_DEFAULT_JQL = "assignee = currentUser() AND resolution = Unresolved ORDER BY updated DESC";
// エクスポートは期間を区切らず、まだ送っていない分をすべて送る
const JIRA_ALL_FROM = "1970-01-01";
const JIRA_ALL_TO = "9999-12-31";

const jira = {
  config: null, // {base_url, token_source}
  jql: load_json("jira_jql", JIRA_DEFAULT_JQL),
  issues: null, // Jira から取った課題 [{key, summary, status}]
  issuesError: null,
  pickedTaskId: null, // 左で選んだタスク（「← 紐付け」の相手）
  draft: null,
  draftKey: null,
  results: null, // 直前の送信結果
  testResult: "",
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
    ? el("a", { href: url, target: "_blank", rel: "noopener", class: "jira_key", title: "Jira で開く", onclick: (ev) => ev.stopPropagation() }, key)
    : el("span", { class: "jira_key" }, key);
}

// ---------- 描画 ----------
function render_jira() {
  if (!jira.config) {
    load_jira_config().then(render_jira);
    return;
  }
  draw_jira_connection();
  draw_jira_match();
  const key = `all@${reloadCount}`;
  if (jira.draftKey !== key) {
    jira.draftKey = key;
    api("GET", `/jira/draft?from=${JIRA_ALL_FROM}&to=${JIRA_ALL_TO}`).then((d) => {
      if (jira.draftKey !== key) return;
      jira.draft = d;
      draw_jira_draft();
    });
  } else {
    draw_jira_draft();
  }
}

function draw_jira_connection() {
  const c = jira.config;
  const url = $("jira_url");
  if (document.activeElement !== url) url.value = c.base_url || "";
  const src = c.token_source === "env" ? "環境変数 MY_TODO_JIRA_TOKEN から読み込み中"
    : c.token_source === "keyring" ? "OS の資格情報に保存済み"
    : "未設定";
  $("jira_status").replaceChildren(
    el("span", { class: "jira_badge " + (c.base_url ? "ok" : "ng") }, c.base_url ? "URL 設定済み" : "URL 未設定"),
    el("span", { class: "jira_badge " + (c.token_source ? "ok" : "ng") }, `PAT：${src}`),
    el("span", {}, jira.testResult || ""));
  $("jira_token_delete").hidden = c.token_source !== "keyring";
  const jql = $("jira_jql");
  if (document.activeElement !== jql) jql.value = jira.jql;
  const sf = $("jira_start_field");
  if (document.activeElement !== sf) sf.value = c.start_field || "";
}

// ---------- 日程・見積もりの同期 ----------
const SYNC_FIELD_LABEL = { due: "期日（終了予定日）", start: "開始日（開始予定日）", estimate: "見積もり" };

function sync_value(field, v) {
  if (v === null || v === undefined) return "（なし）";
  return field === "estimate" ? fmt_hours(Number(v)) : short_date(v);
}

async function compare_jira_fields() {
  $("jira_fields").replaceChildren(el("p", { class: "empty_hint" }, "Jira と比べています…"));
  try {
    jira.fields = await api("GET", "/jira/fields");
  } catch {
    jira.fields = null;
    $("jira_fields").replaceChildren();
    return;
  }
  draw_jira_fields();
}

function draw_jira_fields() {
  const r = jira.fields;
  if (!r) return;
  const box = $("jira_fields");
  const parts = [];
  if (!r.diffs.length) parts.push(el("p", { class: "empty_hint" }, "違いはありません"));
  else {
    parts.push(el("table", { class: "list_table jira_fields_table" },
      el("thead", {}, el("tr", {}, el("th", {}, "タスク"), el("th", {}, "課題"), el("th", {}, "項目"), el("th", {}, "アプリ"), el("th", {}, "Jira"))),
      el("tbody", {}, ...r.diffs.map((d) => el("tr", {},
        el("td", {}, d.title), el("td", {}, jira_key_link(d.issue_key)), el("td", {}, SYNC_FIELD_LABEL[d.field]),
        el("td", {}, sync_value(d.field, d.app)), el("td", {}, sync_value(d.field, d.jira)))))));
    parts.push(el("div", { class: "jira_buttons" },
      el("button", { type: "button", class: "small_button primary", onclick: () => sync_jira_fields("push") }, `アプリの値を Jira に送る（${r.diffs.length} 件）`),
      el("button", { type: "button", class: "small_button", onclick: () => sync_jira_fields("pull") }, `Jira の値をアプリに取り込む（${r.diffs.length} 件）`)));
  }
  if (r.problems.length) {
    parts.push(el("p", { class: "empty_hint warn" }, "比べられなかった課題："),
      el("ul", { class: "jira_problems" }, ...r.problems.map((p) => el("li", {}, `${p.issue_key}（${p.title}）：${p.error}`))));
  }
  box.replaceChildren(...parts);
}

async function sync_jira_fields(direction) {
  const r = jira.fields;
  if (!r?.diffs.length) return;
  const lines = r.diffs.map((d) => `${d.issue_key} ${SYNC_FIELD_LABEL[d.field]}：` +
    (direction === "push" ? `${sync_value(d.field, d.jira)} → ${sync_value(d.field, d.app)}` : `${sync_value(d.field, d.app)} → ${sync_value(d.field, d.jira)}`)).join("\n");
  const msg = direction === "push"
    ? `アプリの値を Jira に送ります（Jira 側が書き換わります）。\n\n${lines}`
    : `Jira の値をアプリに取り込みます（Ctrl+Z で戻せます）。\n\n${lines}`;
  if (!confirm(msg)) return;
  try {
    const res = await api("POST", `/jira/fields/${direction}`, { expected: r.diffs });
    const n = r.diffs.length - res.problems.length;
    toast(res.problems.length
      ? `${n} 件を${direction === "push" ? "送りました" : "取り込みました"}。${res.problems.length} 件はできませんでした：${res.problems.map((p) => p.issue_key).join("、")}`
      : direction === "push" ? `✅ ${n} 件を Jira に送りました` : `✅ ${n} 件を Jira から取り込みました（Ctrl+Z で元に戻す）`,
      res.problems.length ? "error" : "info");
    if (direction === "pull") await reload();
  } finally {
    await compare_jira_fields();
  }
}

// ---------- 紐付けとインポート ----------
function draw_jira_match() {
  draw_jira_tasks();
  draw_jira_issues();
}

/// 左：このアプリのタスク（木のまま。完了したものは紐付いているときだけ出す）
function draw_jira_tasks() {
  const rows = [];
  const walk = (t, depth) => {
    const visible = t.status !== "done" || t.jira_key || children_of(t.id).some((c) => c.status !== "done");
    if (!visible) return;
    rows.push(jira_task_row(t, depth));
    children_of(t.id).forEach((c) => walk(c, depth + 1));
  };
  const roots = [...children_of("root"), ...children_of("inbox")];
  roots.forEach((t) => walk(t, 0));
  $("jira_tasks").replaceChildren(...(rows.length ? rows : [el("p", { class: "empty_hint" }, "タスクがありません")]));
}

function jira_task_row(t, depth) {
  const row = el("div", {
    class: "jira_task" + (jira.pickedTaskId === t.id ? " picked" : "") + (t.status === "done" ? " done" : ""),
    style: `padding-left:${8 + depth * 16}px`,
    title: "クリックで選ぶ（右の「← 紐付け」の相手になる）。右の課題をここにドラッグしても紐付く",
    onclick: () => {
      jira.pickedTaskId = jira.pickedTaskId === t.id ? null : t.id;
      draw_jira_match();
    },
  }, el("span", { class: "title" }, t.title));
  if (t.jira_key) {
    row.append(el("span", { class: "jira_link_chip" },
      jira_key_link(t.jira_key),
      el("button", {
        type: "button",
        class: "remove",
        title: "紐付けを外す",
        onclick: (ev) => { ev.stopPropagation(); link_issue(t.id, null); },
      }, "×")));
  }
  // 右の課題を落とすと紐付く
  row.addEventListener("dragover", (ev) => {
    if (!ev.dataTransfer.types.includes("application/x-jira-key")) return;
    ev.preventDefault();
    row.classList.add("drop_into");
  });
  row.addEventListener("dragleave", () => row.classList.remove("drop_into"));
  row.addEventListener("drop", (ev) => {
    ev.preventDefault();
    row.classList.remove("drop_into");
    const key = ev.dataTransfer.getData("application/x-jira-key");
    if (key) link_issue(t.id, key);
  });
  return row;
}

/// 右：Jira の課題
function draw_jira_issues() {
  const box = $("jira_issues");
  $("jira_import_all").hidden = true;
  if (jira.issuesError) {
    box.replaceChildren(el("p", { class: "empty_hint warn" }, jira.issuesError));
    return;
  }
  if (!jira.issues) {
    box.replaceChildren(el("p", { class: "empty_hint" }, "「Jira から取得」で課題を読み込みます"));
    return;
  }
  if (!jira.issues.length) {
    box.replaceChildren(el("p", { class: "empty_hint" }, "条件に合う課題はありません"));
    return;
  }
  const linked = new Map(state.tasks.filter((t) => t.jira_key).map((t) => [t.jira_key, t]));
  const unlinked = jira.issues.filter((i) => !linked.has(i.key));
  $("jira_import_all").hidden = unlinked.length === 0;
  $("jira_import_all").textContent = `未紐付けをすべて取り込む（${unlinked.length} 件）`;
  const picked = jira.pickedTaskId === null ? null : state.byId.get(jira.pickedTaskId);
  box.replaceChildren(...jira.issues.map((i) => {
    const t = linked.get(i.key);
    const row = el("div", { class: "jira_issue" + (t ? " linked" : ""), draggable: "true", title: "左のタスクにドラッグすると紐付く" },
      el("div", { class: "jira_issue_head" }, jira_key_link(i.key), el("span", { class: "status" }, i.status)),
      el("div", { class: "summary" }, i.summary));
    const foot = el("div", { class: "jira_issue_foot" });
    if (t) {
      foot.append(el("span", { class: "linked_to" }, `→ ${[...path_titles(t), t.title].join(" › ")}`));
    } else {
      if (picked) {
        foot.append(el("button", { type: "button", class: "small_button", onclick: () => link_issue(picked.id, i.key) }, `← 「${picked.title}」に紐付け`));
      }
      foot.append(el("button", { type: "button", class: "small_button", onclick: () => import_issues([i]) }, "取り込む"));
    }
    row.append(foot);
    row.addEventListener("dragstart", (ev) => {
      ev.dataTransfer.setData("application/x-jira-key", i.key);
      ev.dataTransfer.effectAllowed = "link";
    });
    return row;
  }));
}

async function link_issue(taskId, key) {
  await update(taskId, { jira_key: key });
  jira.pickedTaskId = null;
  if (key) toast(`${key} を紐付けました`);
}

async function import_issues(issues) {
  if (!issues.length) return;
  await api("POST", "/jira/import", { issues: issues.map((i) => ({ key: i.key, summary: i.summary })) });
  await reload();
  toast(`Jira から ${issues.length} 件を未分類に取り込みました`);
}

async function fetch_jira_issues() {
  jira.jql = $("jira_jql").value.trim() || JIRA_DEFAULT_JQL;
  save_json("jira_jql", jira.jql);
  jira.issuesError = null;
  $("jira_fetch").disabled = true;
  try {
    const res = await fetch(`/api/jira/search?jql=${encodeURIComponent(jira.jql)}`);
    const body = await res.json().catch(() => ({}));
    if (res.ok) jira.issues = body;
    else jira.issuesError = body.error || res.statusText;
  } catch (e) {
    jira.issuesError = String(e);
  } finally {
    $("jira_fetch").disabled = false;
  }
  draw_jira_issues();
}

// ---------- エクスポート ----------
function draw_jira_draft() {
  const d = jira.draft;
  if (!d) return;
  const box = $("jira_draft");
  const pending = d.lines.filter((l) => l.action !== "unchanged");
  $("jira_send").disabled = pending.length === 0;
  $("jira_send").textContent = pending.length ? `Jira に送信（${pending.length} 件）` : "送るものはありません";

  const result = (l) => jira.results?.find((r) => r.work_date === l.work_date && r.issue_key === l.issue_key);
  // 送信済みで変わっていない行は出さない（直前に送った結果だけは残す）
  const shown = d.lines.filter((l) => l.action !== "unchanged" || result(l));
  const rows = shown.map((l) => {
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
  const parts = [];
  if (rows.length) {
    parts.push(el("table", { class: "list_table jira_table" },
      el("thead", {}, el("tr", {},
        el("th", {}, "日付"), el("th", {}, "課題"), el("th", {}, "まとめたタスク"),
        el("th", { class: "num" }, "送る時間"), el("th", { class: "num" }, "送信済み"), el("th", {}, "操作"), el("th", {}, ""))),
      el("tbody", {}, ...rows)));
  } else {
    parts.push(el("p", { class: "empty_hint" }, "まだ送っていない工数はありません"));
  }
  if (d.unlinked.length) {
    const total = d.unlinked.reduce((n, u) => n + u.minutes, 0);
    parts.push(el("details", { class: "jira_unlinked" },
      el("summary", {}, `Jira 課題に紐付いていない工数 ${fmt_hours(total)}（送られません）`),
      el("ul", {}, ...d.unlinked.map((u) => el("li", {},
        `${short_date(u.work_date)} ${fmt_hours(u.minutes)} `,
        el("a", { href: "#", onclick: (ev) => { ev.preventDefault(); select(u.task_id); } }, u.task))))));
  }
  box.replaceChildren(...parts);
}

// ---------- 操作 ----------
function setup_jira() {
  $("jira_fields_compare").addEventListener("click", compare_jira_fields);
  $("jira_save").addEventListener("click", async () => {
    const body = { base_url: $("jira_url").value, start_field: $("jira_start_field").value };
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
  $("jira_fetch").addEventListener("click", fetch_jira_issues);
  $("jira_jql").addEventListener("keydown", (ev) => {
    if (ev.key === "Enter" && !ev.isComposing) fetch_jira_issues();
  });
  $("jira_import_all").addEventListener("click", () => {
    const linked = new Set(state.tasks.filter((t) => t.jira_key).map((t) => t.jira_key));
    const unlinked = (jira.issues || []).filter((i) => !linked.has(i.key));
    if (unlinked.length && confirm(`${unlinked.length} 件の課題を、未分類のタスクとして取り込みますか？`)) import_issues(unlinked);
  });
  $("jira_send").addEventListener("click", async () => {
    const pending = jira.draft?.lines.filter((l) => l.action !== "unchanged") || [];
    if (!pending.length) return;
    const summary = pending.map((l) => `${short_date(l.work_date)} ${l.issue_key} ${JIRA_ACTION[l.action]} ${fmt_hours(l.minutes)}`).join("\n");
    if (!confirm(`次の ${pending.length} 件を Jira に送ります。\n\n${summary}`)) return;
    $("jira_send").disabled = true;
    try {
      // 確認した内容も送り、サーバーで作り直した内容と食い違ったら送らない（その場合は下書きを読み直す）
      const expected = pending.map((l) => ({ work_date: l.work_date, issue_key: l.issue_key, action: l.action, minutes: l.minutes }));
      jira.results = await api("POST", "/jira/send", { from: JIRA_ALL_FROM, to: JIRA_ALL_TO, expected });
      const failed = jira.results.filter((r) => r.error).length;
      toast(failed ? `${jira.results.length - failed} 件送信、${failed} 件失敗しました` : `✅ ${jira.results.length} 件を Jira に送信しました`, failed ? "error" : "info");
    } finally {
      jira.draftKey = null;
      render();
    }
  });
}
