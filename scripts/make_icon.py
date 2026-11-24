#!/usr/bin/env python3
"""exe のアイコンを作る（開発するときだけ使う。ビルドでは使わない）。

1. src/web/static/favicon.svg を各サイズの PNG（icon_16.png … icon_256.png）にしておく
   （ブラウザで表示して撮るなど。作者は Playwright の Chromium で撮った）
2. python3 scripts/make_icon.py <PNG のフォルダ>
   → assets/my_todo.ico（PNG を並べただけの ICO）と、
     assets/my_todo.res と assets/my_todo_res.o（ICO を入れた Windows のリソース。mingw の windres で作る）を書き出す

build.rs は Windows 向けのとき、そのどちらかをリンクに渡すだけ。
"""
import struct
import subprocess
import sys
from pathlib import Path

SIZES = [16, 24, 32, 48, 64, 256]
ROOT = Path(__file__).resolve().parent.parent


def main() -> None:
    src = Path(sys.argv[1])
    pngs = [(s, (src / f"icon_{s}.png").read_bytes()) for s in SIZES]
    # ICO：ヘッダー（6 バイト）＋ 画像ごとの目次（16 バイト）＋ PNG のデータ
    header = struct.pack("<HHH", 0, 1, len(pngs))
    offset = 6 + 16 * len(pngs)
    entries, data = b"", b""
    for size, png in pngs:
        wh = 0 if size >= 256 else size  # 256 は 0 と書く決まり
        entries += struct.pack("<BBBBHHII", wh, wh, 0, 0, 1, 32, len(png), offset)
        offset += len(png)
        data += png
    ico = ROOT / "assets" / "my_todo.ico"
    ico.write_bytes(header + entries + data)
    # MSVC のリンカーは .res を、MinGW（GNU ld）は COFF のオブジェクトを受け取るので両方作る
    for fmt, out in [("res", "assets/my_todo.res"), ("coff", "assets/my_todo_res.o")]:
        subprocess.run(
            ["x86_64-w64-mingw32-windres", "-O", fmt, "-i", "assets/my_todo.rc", "-o", out],
            cwd=ROOT,
            check=True,
        )
    print(f"{ico}、assets/my_todo.res、assets/my_todo_res.o を作りました")


if __name__ == "__main__":
    main()
