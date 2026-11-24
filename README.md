# my_todo

自分の工数を自分のために管理する、Rust 製のローカル Web アプリ。開発プランは手元の docs/development_plan.md（リポジトリには入れていない）。

現在はフェーズ 1（土台と気軽な登録）まで実装済み。

## 起動

```
cargo run --release
```

`http://127.0.0.1:8765/` で待ち受け、ブラウザが自動で開く（WSL から起動したときは Windows 側の既定ブラウザ）。127.0.0.1 以外からは接続できない。

Windows では窓（コンソール）を出さない。画面（ブラウザのタブ）を閉じると、ほかに開いている画面がなければ 1 分ほどで自動で終わる（読み直しや別のタブがあれば続く。Jira に送信中は待つ）。起動できないときはダイアログで知らせる。すでに起動していれば、もう一度起動すると画面を開くだけ。

| オプション | 説明 |
| --- | --- |
| `--db <PATH>` | データベースファイル（既定：実行ファイルと同じ場所の `my_todo.db`。`cargo run` のように `target/` 配下から起動したときはカレントディレクトリの `my_todo.db`） |
| `--port <PORT>` | 待ち受けポート（既定：8765） |
| `--no_open` | 起動時にブラウザを開かない |
| `--keep_running` | 画面を閉じても終了しない |

バックアップは `my_todo.db` をコピーすればよい（起動中は `my_todo.db-wal` も合わせてコピーするか、アプリを止めてからコピーする）。

## 画面の操作

上部のタブで「線表」「詳細」「工数記録」「Jira」「ヘルプ」を切り替える。操作の説明はアプリの「ヘルプ」タブにもある。右の詳細パネルはどのタブでも共通。
ヘッダー右の ↶ ↷ か **Ctrl+Z / Ctrl+Y**（Ctrl+Shift+Z も可）で、追加・変更・移動・削除を元に戻す／やり直す（入力欄の中では文字の取り消しになる）。

### 線表（ふだんはここで完結）
- 線表の左上「タスク」の横の「➕ 追加」で線表にタスクを追加。行の「➕」でその下に子を追加。Enter で続けて入力、Esc で終了
- 線表の下に「未分類」のペインがある。思いついたことは `/` キー（または未分類の ➕）でここに入れておき、行をドラッグして線表へ移す。線表の行を未分類の見出しに落とすと未分類へ戻る
- 左の名前欄
  - ドラッグで並べ替え・階層変更：行の上端・下端に落とすと前後に、中央に落とすと子になる。左上の「タスク」に落とすと最上位の末尾へ
  - チェックで完了／未着手に戻す。ダブルクリックで名前を変更。行のタグをクリックすると絞り込み
  - Shift＋クリックで範囲、Ctrl＋クリックで 1 件ずつ選べる。選んだ行をドラッグするとまとめて移動（並び順を保つ。Ctrl+Z 1 回で戻る）
  - Ctrl+C／Ctrl+X／Ctrl+V で、選んだタスク（子も含む）をコピー／切り取り／貼り付け。貼り付け先は選んでいるタスクの後ろ（なければ未分類の末尾）。コピーは複製（状態は未着手、Jira の課題キーは写さない）、切り取りは移動。ほかのアプリでコピーした文字は 1 行 1 タスクになる
- 右の時間軸（「6 週間」「3 か月」「1 年」を切り替え。どれも横にスクロールせず幅に収まるように縮める。ホイールで行の上下スクロール、Shift＋ホイールで前後に動く。「前の週」「次の週」「今日」でも移動）
  - バーは開始予定日（なければ着手日）から終了予定日まで。終了予定日だけなら ◆、次の予定は ▶
  - 色：未着手は薄い青、作業中は青、完了は緑（予定の期間、なければ着手日〜完了日）。終了予定日を過ぎた未完了は赤枠
  - 子を持つ親には、子の範囲を横に倒した I（箱ひげ図のひげ）で出す
  - 開始日・終了日がそろっていない行は、横にドラッグした範囲が開始予定日〜終了予定日になる。バーの中央をドラッグで移動、両端をドラッグで期間を増減。移動中に右下に出るゴミ箱に入れると日程だけを消す
  - 選んだタスクは Delete キーか 🗑️ でまとめて削除（子も一緒に）。名前をドラッグしてゴミ箱に入れるとタスクを削除
- テンプレート：タスク（子を含む）を選んで保存し、「使う」で開始日を選んで作り直す。日程は一番早い日付からの平日の数（土日を除く。祝日は考えない）で残り、使う先でも平日で数える。データベースの中に保存（Ctrl+Z・MCP の対象）
- 絞り込み：キーワード、タグ（いずれかを含む。親のタグは子にも効く）、「完了を隠す」。条件に合うタスクの祖先は薄く表示する
  - タグはよく使う 8 個を並べ、残りは「ほかのタグ」から名前で探す。どのタスクにも付いていないタグは消え、選んでいた絞り込みからも外れる
  - タグをタスクの行へドラッグすると、そのタグを付ける（選んでいる行に落とすと選んでいるタスクすべてに。Ctrl+Z 1 回で戻る）。タスクに付いているタグをゴミ箱へドラッグすると外す
- 詳細パネルの「関連資料」：URL を貼るとリンク先のページの名前（<title>）で表示する。取れないとき（ログインが要るページなど）はアドレスを短くして表示。名前はダブルクリックで変える。http・https だけ。Jira 課題と見積もりは「その他」に畳む（詳細タブでは開く）

### 詳細（すべてのタスクを表で）
- 過去も未来も、完了したものも含めて全タスクを表で出す。見出しをクリックで並べ替え、行をクリックで右に詳細
- プリセット：今日やること・今週・来週・期限切れ・予定なし・完了（今週）・完了（すべて）・すべて
- 条件
  - キーワード（名前・メモ・次の予定・タグ・親の名前）
  - 状態（未着手・作業中・完了。「作業中は日付に関係なく含める」も可）
  - 日付：種類（予定〔開始〜終了・次の予定〕・開始予定日・終了予定日・次の予定・完了日・着手日・登録日）と範囲（期限切れ・今日まで・今日・今週・来週・先週・今月・先月・今日以降・今日より前・日付なし・期間指定）
  - タグ（いずれかを含む・すべて含む・含まない、タグなし）
  - 場所（タスク・未分類、特定の親の下）
- 表示：グループなし・日付ごと・状態ごと・親ごと・タグごと。完了したものは終了予定日と比べた遅れ／前倒しを出す。見積もりと実績（子を含む）の列もある
- 「条件を保存」で名前を付けて保存し、★ のボタンから呼び出せる（このブラウザに保存）

### 工数記録（1 日の勤務時間を割合で振り分ける。記録した工数の確認は詳細タブ）
- 日付を選び（週の帯をクリックしても移動できる）、その日の勤務時間（残業込みの実際の時間）を 0.5 時間刻みで入れる。平日の初期値は標準の勤務時間、土日は 0
- 候補に並ぶのは、線表でその日にバーが重なっているタスクと、その日に完了したタスク（と記録済みのもの）。ほかのタスクは下の欄から追加
- スライダー（または数字）で割合を決めると、勤務時間を 0.5 時間刻みで振り分ける。合計は必ず勤務時間に一致する（端数は最大剰余方式で配る）。動かすと自動で保存
- 1D の時間（既定 8 時間）と標準の勤務時間を設定できる。標準より長く働いた日は「残業 +2h」のように出る
- 記録は時間（内部は分）で持つので、残業した日は合計が 1D を超える（例：10 時間 = 1.25D）

### 日付の入力
- 日付の欄をクリックするとカレンダー（月曜始まり）が出る。文字でも入れられる：`11/25`・`2026-11-25`・`1125`・`今日`・`明日`・`+3`（3 日後）・`-2`

### 詳細パネル
- タイトル、タグ（Enter で追加、Backspace・× で外す）、状態、開始予定日・終了予定日、見積もり（`1d`・`4h`・`1.5d`・`30m`、数字だけなら時間）、次の予定、メモ
- 見積もりの下に、記録した工数（子を含む）と消化率を出す。見積もりを超えたら赤
- 「記録」に、登録・着手・完了の日時と変更の履歴が自動で残る（5 分以内の変え直しは 1 件にまとめる）

## AI から使う（MCP）

`my_todo mcp` で MCP サーバー（標準入出力）として動く。画面と同じ DB を共有し、変更は画面の Ctrl+Z でも戻せる。AI のアプリ側に一度登録するだけで、アプリ側の設定は要らない（設定例はヘルプタブにもある）。

```
claude mcp add my_todo -- C:\tools\my_todo\my_todo.exe mcp --db C:\tools\my_todo\my_todo.db
```

Claude Desktop などの設定ファイルでは：

```json
{ "mcpServers": { "my_todo": { "command": "C:\\tools\\my_todo\\my_todo.exe", "args": ["mcp", "--db", "C:\\tools\\my_todo\\my_todo.db"] } } }
```

ツール：`get_schedule`（今日・今週の予定）、`list_tasks`、`get_task`、`add_todo`、`update_task`、`move_task`、`delete_task`、`get_work_day`、`log_work_day`、`list_tags`、`list_templates`、`apply_template`（テンプレートから作る）、`undo`、`redo`

## Jira 連携

- Jira（Data Center / Server 8.14 以降）に PAT で接続する（`Authorization: Bearer`、REST API v2）。Cloud 版は `JiraApi` トレイトに実装を足せば対応できる作り
- PAT は OS の資格情報（Windows の資格情報マネージャーなど）に、Jira の URL（scheme://host:port）ごとに保存し、DB には保存しない。DB ごとに URL を持つので、人からもらった DB に別の URL が書かれていても PAT は送られない。環境変数 `MY_TODO_JIRA_TOKEN` があればそちらを優先する（URL を問わない）
- Jira の URL は https:// だけ（http:// は localhost・127.0.0.1 のみ。URL として読んでホスト名で判定する）
- このアプリがマスター。Jira とのやり取りはインポートとエクスポートの 2 つだけ
  - 紐付けとインポート：左にこのアプリのタスク、右に Jira の課題（JQL で取得。既定は自分が担当の未解決）。右の課題を左のタスクにドラッグすると紐付く。対応するタスクがない課題は「取り込む」で未分類にタスクとして作る
  - エクスポート：まだ送っていない工数を、日ごと・課題ごとにまとめて worklog として送る。課題キーのない子タスクは、紐付けた一番近い親の課題に合算する。送った worklog を覚えておき、工数が変われば更新、0 になれば削除する（二重送信しない）。画面で確認した内容とサーバーで作り直した内容が食い違ったら送らない。作成の応答が届かなかったときに備え、作る前に同じ worklog（自分・その日の 9:00 開始・同じ時間）がないか確かめる。送信は画面を閉じても最後まで続く

## データベースの切り替え

- 右上のファイル名（📂）から、別のデータベースを開ける（最近開いたファイルか、Windows ならエクスプローラーの「ファイルを開く」画面で選ぶか、場所を入れて開く。まだない場所なら新しく作るか聞く）
- 開いたファイルはユーザーごとの設定ファイルに記録し、次に起動したときは前回のファイルを開く（`--db` を指定したときはそれが優先）。記録先：Windows は `%APPDATA%\my_todo\config.json`、それ以外は `~/.config/my_todo/config.json`
- MCP（`my_todo mcp`）も、`--db` がなければ前回のファイルを使う

## Windows 向けビルド

単一の `my_todo.exe` になる（SQLite と画面は .exe に同梱。必要な DLL は Windows 標準のものだけ）。リリースビルドはシンボルを除いて約 13MB。

- WSL / Linux からクロスビルド（mingw-w64 の C コンパイラが要る）：
  ```
  rustup target add x86_64-pc-windows-gnu
  sudo apt install mingw-w64
  cargo build --release --target x86_64-pc-windows-gnu
  ```
  → `target/x86_64-pc-windows-gnu/release/my_todo.exe`
- Windows 上でビルド：`cargo build --release` → `target\release\my_todo.exe`

使い方：好きなフォルダに `my_todo.exe` を置いてダブルクリック（DB は同じフォルダの `my_todo.db` に作られる）。

## 構成

```
migrations/             SQLite のマイグレーション（PRAGMA user_version で管理）
src/main.rs             引数の解析と起動
src/db.rs               DB の接続とマイグレーション
src/model.rs            タスクなどの型
src/service/            サービス層（画面・MCP・Jira 連携の共通ロジック。通信に依存しない）
src/web/                HTTP サーバーと REST API
src/config.rs           ユーザーごとの設定（最近開いたデータベース）
src/mcp.rs              MCP サーバー（my_todo mcp）
src/jira.rs             Jira との通信と PAT の保管
src/web/static/         画面（共通は app.js、線表は gantt.js、詳細タブは list.js、工数記録は work.js、Jira は jira.js、ヘルプは help.js、日付の入力は datepicker.js）
```

### REST API

| メソッド | パス | 内容 |
| --- | --- | --- |
| GET | `/api/tasks` | 全タスク |
| POST | `/api/tasks` | 登録 `{title, parent_id?, note?, inbox?}`。親なしは未分類へ（`inbox: false` でタスク一覧へ） |
| GET | `/api/tasks/{id}` | 1 件取得 |
| PATCH | `/api/tasks/{id}` | 更新 `{title?, status?, note?, planned_start?, planned_end?, next_action?, next_action_date?, tags?, estimate_min?}`。tags は一覧で置き換え。status は `todo` / `doing` / `done`、日付は `YYYY-MM-DD`、null で取り消し |
| POST | `/api/tasks/{id}/move` | 移動 `{parent_id, inbox, index?}` |
| POST | `/api/tasks/copy` | 子孫ごと複製して貼り付け `{ids, placement}` |
| POST | `/api/tasks/create_many` | 1 行 1 タスクで作る `{titles, placement}` |
| POST | `/api/tasks/move_many` | まとめて移動 `{ids, placement: {kind: "before"/"after", ref_id} / {kind: "into", parent_id?, inbox}}` |
| DELETE | `/api/tasks/{id}` | 子孫ごと削除 |
| GET | `/api/agenda?today=YYYY-MM-DD` | 予定一覧（期限切れ・今日・今週・来週）。today 省略時はサーバーの今日 |
| GET | `/api/tags` | 使われているタグと件数 |
| GET | `/api/work_days?from=&to=` | 期間内に記録した日（勤務時間と振り分け済みの合計） |
| GET | `/api/work_days/{date}` | その日の勤務時間・振り分け・候補のタスク |
| PUT | `/api/work_days/{date}` | `{worked_min, allocations: [{task_id, weight}]}` で保存（30 分刻みで振り分け直す） |
| GET / PATCH | `/api/settings` | `{minutes_per_day, default_work_min}`（1D の時間、標準の勤務時間） |
| GET / PATCH | `/api/jira` | Jira の URL と PAT の保存先（PATCH は `{base_url?, token?}`。token は返さない） |
| POST | `/api/jira/test` | 接続テスト（自分のユーザー情報） |
| GET | `/api/jira/issues/{key}` | 課題の件名と状態 |
| GET | `/api/jira/draft?from=&to=` | 送る内容の下書き |
| GET | `/api/jira/search?jql=` | JQL で課題を探す |
| POST | `/api/jira/import` | `{issues: [{key, summary}]}` を未分類のタスクとして取り込む |
| GET | `/api/databases` | 開いているデータベースと最近開いたファイル |
| POST | `/api/databases/open` | `{path, create?}` に切り替える |
| POST | `/api/databases/forget` | 最近開いたファイルの一覧から外す |
| POST | `/api/jira/send` | `{from, to}` の下書きを送る（差分だけ） |
| GET | `/api/about` | 版（ビルドの印）と開いている DB |
| POST | `/api/undo` | 直前の操作を元に戻す。`{label}`（戻せなければ null） |
| POST | `/api/redo` | 元に戻した操作をやり直す |
| GET | `/api/tasks/{id}/events` | 変更履歴（登録・タイトル・状態・終了予定日・次の予定） |

## 確認

```
cargo test
cargo clippy --all-targets
cargo deny check
```

書き方の決まり（コンパイル時に引っかかるように）：

- DB や JSON に文字列で保存する値（状態・履歴の種類・設定のキー・工数の記録元・MCP のツール名など）は、`str_enum!`（`src/str_enum.rs`）で enum として定義する。対応表は 1 か所だけで、変換・SQLite・JSON の読み書きはそこから作られる。知らない値は黙って既定値にせずエラーにする
- enum の match では「その他すべて（`_`）」を使わない（`clippy::wildcard_enum_match_arm` を deny）。種類を足したときの対応漏れがコンパイルエラーになる
- unsafe は使わない（`unsafe_code` を deny。Windows の API を呼ぶ 1 か所だけ理由を書いて許可）

`cargo deny check` は依存クレートのライセンス（寛容なものだけ許可）・脆弱性（RustSec）・重複・取得元（crates.io のみ）を確かめる。設定は `deny.toml`。

## ライセンス

MIT または Apache-2.0 のどちらか（[LICENSE-MIT](LICENSE-MIT)・[LICENSE-APACHE](LICENSE-APACHE)）。
