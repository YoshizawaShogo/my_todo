// Windows 向けにビルドするときだけ、アイコンの入ったリソースを exe に組み込む。
// ほかには何もしない（外部のコマンドを動かさず、ファイルも書かず、ネットにもつながない）。
// リソースの作り方は scripts/make_icon.py を参照。
fn main() {
    println!("cargo:rerun-if-changed=assets");
    let env = |key| std::env::var(key).unwrap_or_default();
    if env("CARGO_CFG_TARGET_OS") == "windows" {
        // MSVC のリンカーは .res を、MinGW（GNU ld）は COFF のオブジェクトを受け取る
        let file = if env("CARGO_CFG_TARGET_ENV") == "msvc" {
            "my_todo.res"
        } else {
            "my_todo_res.o"
        };
        println!(
            "cargo:rustc-link-arg-bins={}/assets/{file}",
            env("CARGO_MANIFEST_DIR")
        );
    }
}
