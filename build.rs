//! ビルドした日時を版の印として埋め込む（画面が古い版のままか見分けるため）
fn main() {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    println!("cargo:rustc-env=MY_TODO_BUILD={secs}");
    // ソース・画面・マイグレーションが変わったときだけ付け直す
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-changed=migrations");
    println!("cargo:rerun-if-changed=build.rs");
}
