"use strict";
// 線表と未分類での Ctrl+C（コピー）・Ctrl+X（切り取り）・Ctrl+V（貼り付け）。
// ブラウザの copy / cut / paste の仕組みを使うので、OS のクリップボードとも文字をやり取りできる。
// - コピー：選んだタスクを覚え、タスク名を 1 行ずつ文字としてもクリップボードに入れる
// - 切り取り：同じく覚えて行を薄くする。貼り付けたときに移動する（Esc でやめる）
// - 貼り付け：選んでいるタスクの後ろ（なければ未分類の末尾）へ。覚えているものなら複製（切り取りなら移動）、
//   ほかのアプリでコピーした文字なら 1 行 1 タスクで作る
// 入力欄の中や、画面の文字を選んでいるときは、ふつうの文字のコピー・貼り付けのまま

const clip = {
  mode: null, // "copy" / "cut" / null
  ids: [], // 覚えているタスク（画面の並び順）
  text: "", // そのときクリップボードに入れた文字（自分で入れたものか見分ける）
};

/// タスクのコピー・貼り付けとして扱ってよいか
function task_clipboard_active() {
  if (state.tab !== "gantt") return false;
  if (is_text_entry(document.activeElement)) return false;
  const sel = window.getSelection();
  if (sel && !sel.isCollapsed && sel.toString().trim()) return false; // 画面の文字を選んでいる
  return true;
}

function selected_in_order() {
  return visible_task_order().filter((id) => state.selected.has(id));
}

function remember_tasks(mode, ev) {
  if (!task_clipboard_active() || state.selected.size === 0) return;
  ev.preventDefault();
  const ids = selected_in_order();
  const text = ids.map((id) => state.byId.get(id)?.title ?? "").join("\n");
  ev.clipboardData.setData("text/plain", text);
  clip.mode = mode;
  clip.ids = ids;
  clip.text = text;
  mark_cut_rows();
  toast(mode === "cut"
    ? `${ids.length} 件を切り取りました。貼り付ける場所を選んで Ctrl+V（Esc でやめる）`
    : `${ids.length} 件をコピーしました。貼り付ける場所を選んで Ctrl+V`);
}

/// 貼り付ける場所：選んでいるタスク（画面で一番下のもの）の後ろ。選んでいなければ未分類の末尾
function paste_placement() {
  const sel = selected_in_order();
  return sel.length ? { kind: "after", ref_id: sel[sel.length - 1] } : { kind: "into", parent_id: null, inbox: true };
}

async function paste_tasks(ev) {
  if (!task_clipboard_active()) return;
  const text = ev.clipboardData.getData("text/plain");
  if (!text.trim()) return;
  ev.preventDefault();
  const placement = paste_placement();
  const ours = clip.mode !== null && text === clip.text;
  const ids = clip.ids.filter((id) => state.byId.has(id));
  if (ours && ids.length) {
    if (clip.mode === "cut") {
      if (placement.kind === "after" && ids.includes(placement.ref_id)) {
        toast("切り取ったタスク自身の後ろには貼り付けられません。別の場所を選んでください", "error");
        return;
      }
      await api("POST", "/tasks/move_many", { ids, placement });
      clear_cut();
      toast(`${ids.length} 件を移動しました（Ctrl+Z で元に戻す）`);
    } else {
      await api("POST", "/tasks/copy", { ids, placement });
      toast(`${ids.length} 件を貼り付けました（Ctrl+Z で元に戻す）`);
    }
  } else {
    // ほかのアプリでコピーした文字：1 行 1 タスク
    const titles = text.split(/\r?\n/).map((l) => l.trim()).filter(Boolean);
    await api("POST", "/tasks/create_many", { titles, placement });
    toast(`${titles.length} 行をタスクとして貼り付けました（Ctrl+Z で元に戻す）`);
  }
  await reload();
}

function clear_cut() {
  if (clip.mode !== "cut") return;
  clip.mode = null;
  clip.ids = [];
  clip.text = "";
  mark_cut_rows();
}

/// 切り取り中の行を薄く見せる（描き直したあとにも呼ぶ）
function mark_cut_rows() {
  document.querySelectorAll("[data-task_id].cut").forEach((r) => r.classList.remove("cut"));
  if (clip.mode !== "cut") return;
  for (const id of clip.ids) {
    document.querySelectorAll(`[data-task_id="${id}"]`).forEach((r) => r.classList.add("cut"));
  }
}

function setup_clipboard() {
  document.addEventListener("copy", (ev) => remember_tasks("copy", ev));
  document.addEventListener("cut", (ev) => remember_tasks("cut", ev));
  document.addEventListener("paste", (ev) => { paste_tasks(ev).catch(() => {}); });
}
