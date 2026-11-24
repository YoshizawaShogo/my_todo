//! Windows の「ファイルを開く」ダイアログ（エクスプローラーの画面）でデータベースのファイルを選ぶ。
//! 新しいクレートは足さず、すでに使っている windows-sys で Windows の API を直接呼ぶ。
//! Windows 以外では使えない（None を返す）。

use std::path::{Path, PathBuf};

/// ダイアログを出して、選んだファイルの場所を返す（やめたら None）。
/// ないファイル名を入れてもよい（呼び出し側で新しく作るか聞く）。ダイアログを閉じるまで戻らない
#[cfg(windows)]
#[allow(unsafe_code)] // Windows の API（CreateWindowExW・GetOpenFileNameW・DestroyWindow）を呼ぶため
pub fn pick_database(initial: Option<&Path>) -> Option<PathBuf> {
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use windows_sys::Win32::UI::Controls::Dialogs::{
        GetOpenFileNameW, OFN_EXPLORER, OFN_HIDEREADONLY, OFN_NOCHANGEDIR, OFN_PATHMUSTEXIST,
        OPENFILENAMEW,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, SetForegroundWindow, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
        WS_POPUP,
    };

    let wide = |s: &str| -> Vec<u16> { s.encode_utf16().chain([0]).collect() };
    // このアプリには窓がないので、ダイアログがブラウザの裏に隠れないよう、
    // 見えない「いちばん手前」の窓を作って持ち主にする
    let class = wide("STATIC");
    // SAFETY: 文字列は 0 で終わる UTF-16 で呼び出しの間生きている。ほかの引数は定数か null
    let owner = unsafe {
        CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
            class.as_ptr(),
            std::ptr::null(),
            WS_POPUP,
            0,
            0,
            0,
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null(),
        )
    };
    if !owner.is_null() {
        // SAFETY: いま作った窓のハンドル
        unsafe { SetForegroundWindow(owner) };
    }

    // 結果を受け取る領域（初期値として今のファイルの場所を入れておく）
    let mut file = vec![0u16; 4096];
    if let Some(p) = initial {
        let init: Vec<u16> = p.as_os_str().encode_wide().take(file.len() - 1).collect();
        file[..init.len()].copy_from_slice(&init);
    }
    let filter: Vec<u16> = "my_todo のデータベース (*.db)\0*.db\0すべてのファイル (*.*)\0*.*\0\0"
        .encode_utf16()
        .collect();
    let title = wide("データベースを開く（ないファイル名を入れると新しく作れます）");
    let def_ext = wide("db");
    let mut ofn = OPENFILENAMEW {
        lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32,
        hwndOwner: owner,
        lpstrFilter: filter.as_ptr(),
        nFilterIndex: 1,
        lpstrFile: file.as_mut_ptr(),
        nMaxFile: file.len() as u32,
        lpstrTitle: title.as_ptr(),
        lpstrDefExt: def_ext.as_ptr(),
        Flags: OFN_EXPLORER | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR | OFN_HIDEREADONLY,
        ..Default::default()
    };
    // SAFETY: ofn の中のポインタはすべて、この関数の中で生きている配列を指している
    let ok = unsafe { GetOpenFileNameW(&mut ofn) } != 0;
    if !owner.is_null() {
        // SAFETY: いま作った窓のハンドル（ダイアログはもう閉じている）
        unsafe { DestroyWindow(owner) };
    }
    if !ok {
        return None;
    }
    let len = file.iter().position(|&c| c == 0).unwrap_or(file.len());
    Some(PathBuf::from(std::ffi::OsString::from_wide(&file[..len])))
}

#[cfg(not(windows))]
pub fn pick_database(_initial: Option<&Path>) -> Option<PathBuf> {
    None
}
