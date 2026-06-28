# Container UI desktop QA

## 確認日

2026-06-27 / 2026-06-28 JST

## 対象環境

- macOS 26.3
- Node.js v22.23.0
- npm 10.9.8
- rustc 1.92.0
- cargo 1.92.0
- `container CLI version 1.0.0 (build: release, commit: ee848e3)`
- `container system status --format json`: `running`

## 指示の解釈

ローカルにインストールされ使用中の `apple/container` を対象に、軽量なデスクトップ管理アプリを実装し、UI 確認を随時行い、最終的にパッケージ済み `.app` で動作確認するタスクとして扱った。

## 使用データ

- 既存 running container: `buildkit`, `agentab-dev-up`
- 一時確認 container: `container-ui-smoke`
  - 作成: `container create --name container-ui-smoke docker.io/library/node:24.18.0-bookworm sleep 1800`
  - UI Start 確認後 state: `running`
  - UI Stop 確認後 state: `stopped`
  - 削除: `container delete container-ui-smoke`

## 実行コマンド

```bash
git fetch --all --prune
npm install
npm audit --audit-level=moderate
npm audit signatures --json
npm run typecheck
npm run lint
npm test
cargo fmt --manifest-path src-tauri/Cargo.toml --check
cargo test --manifest-path src-tauri/Cargo.toml
npm run build
npm run tauri:build
mise run local-check
npm run tauri -- icon src/assets/container-ui-icon.png
npm run dev -- --host 127.0.0.1
open -n "src-tauri/target/release/bundle/macos/Container UI.app"
container system status --format json
container system start
container list --all --format json
container create --name container-ui-smoke docker.io/library/node:24.18.0-bookworm sleep 1800
/opt/homebrew/bin/cliclick c:2076,588
/opt/homebrew/bin/cliclick c:2064,587
printf 'STOP container-ui-smoke' | pbcopy
# app focused: paste into the confirmation field
printf 'QA hashed approval stop verification' | pbcopy
# app focused: paste into the reason field
/opt/homebrew/bin/cliclick c:2066,774
container delete container-ui-smoke
```

## 確認結果

- Vite preview で desktop viewport と mobile viewport を確認した。
- Browser preview では Dashboard / Containers / Images / Activity の主要ナビゲーションを確認した。
- 2026-06-28 にオリジナルのアプリアイコンを生成し、`src/assets/container-ui-icon.png` と `src-tauri/icons/*` に反映した。
- 2026-06-28 に Browser preview で Dashboard / Containers / Images / Activity / Stop dialog を 1280x780 と 980x640 で検査し、control overlap / horizontal overflow / unexpected text overflow が 0 件であることを確認した。
- 2026-06-28 に packaged `.app` を再ビルドし、実アプリのウィンドウキャプチャを `docs/assets/container-ui-containers.png` として保存した。
- 2026-06-28 に Browser preview で Containers 画面を 1280x780 / 1120x720 / 720x720 で再検査し、normal table / detail drawer / stopped preview すべてで control overlap / horizontal overflow / unexpected text overflow / Memory-Action overlap が 0 件であることを確認した。
- 2026-06-28 に `?system=stopped` の Browser preview で `Start container system` 導線を確認し、クリック後に mock status が `running` へ戻ることを確認した。
- `npm audit --audit-level=moderate`: 0 vulnerabilities。
- `npm audit signatures --json`: `invalid: []`, `missing: []`。
- `npm run typecheck`: pass。
- `npm run lint`: pass。
- `npm test`: 1 file / 7 tests pass。
- `cargo fmt --manifest-path src-tauri/Cargo.toml --check`: pass。
- `cargo test --manifest-path src-tauri/Cargo.toml`: 12 tests pass。
- `npm run build`: pass。
- `npm run tauri:build`: pass。
- `mise run local-check`: mise の trust ガードで未実行。ユーザーの trust 設定は変更せず、同タスク内の実コマンドは個別に成功確認済み。
- 生成物:
  - `src-tauri/target/release/bundle/macos/Container UI.app`
  - `src-tauri/target/release/bundle/dmg/Container UI_0.1.0_aarch64.dmg`
- パッケージ済み `.app` 起動後、activity log に次の実 CLI 呼び出し成功が記録された。
  - `container --version`
  - `container system status --format json`
  - `container list --all --format json`
  - `container image list --format json --verbose`
  - `container volume list --format json`
  - `container network list --format json`
  - `container stats --format json --no-stream`
  - `container inspect buildkit`
  - `container logs -n 200 buildkit`
- パッケージ済み `.app` で `container-ui-smoke` の `Start` ボタンをクリックし、`container_start` が activity log に記録され、CLI 状態が `running` になった。
- `buildkit` は managed container として表示され、Stop ボタンが無効化されていることを確認した。
- パッケージ済み `.app` で `container-ui-smoke` の `Stop` ボタンをクリックし、承認ダイアログで次を入力した。
  - Required phrase: `STOP container-ui-smoke`
  - Reason: `QA hashed approval stop verification`
- 承認付き Stop 後、CLI 状態が `stopped` になった。
- activity log に `container_stop_approval_requested`、`container_stop_pending`、`container_stop` が同じ hashed `approvalId` で記録された。
- `container_stop` は `requestedBy: local-user:yoshihide`、`approvalReason: QA hashed approval stop verification` 付きで記録された。
- activity log は最新 150 件に圧縮されるため、後続の自動更新で古い検証レコードはローテーションされる。
- `container-ui-smoke` は停止後に CLI で削除し、`container list --all --format json` に残っていないことを確認した。
- 2026-06-28 の再撮影時に `container system status --format json` が一時的に `unregistered` を返したため、`container system start` で復旧し、CLI が `running` を返すことを確認してから packaged `.app` を再撮影した。
- 2026-06-28 の追加対応では、作業中のローカル container service を実際に停止する副作用を避けるため、`container system stop` 状態の UI は Browser preview mock で再現した。backend 側は `container system start` が固定引数 `["system", "start"]` だけを使うことを Rust test で確認した。

## 発見した不具合と対応

- 初期テーブルでは操作列が視認しづらかった。
  - 対応: 行アクションをアイコン単独から `Start` / `Stop` ラベル付きボタンに変更した。
- npm audit で Vitest 2 系由来の脆弱性が出た。
  - 対応: Vitest 4 系に更新し、audit 0 件を確認した。
- Stop 失敗時のエラー表示が `token` という通常語まで secret と判定してマスクされ、原因が読めなかった。
  - 対応: secret マスクを `TOKEN=` / `PASSWORD=` などのキー形式に限定し、通常の承認エラーメッセージは表示できるようにした。
- `cliclick` の直接タイプはキーボード配列の影響で文字が崩れる場合があった。
  - 対応: QA 操作ではクリップボード貼り付けで確認文と理由を正確に入力した。
- 実アプリキャプチャで `stopped` の State pill が `stoppe` までしか表示されなかった。
  - 対応: Containers table を `Name / State / Workload / Memory / Action` に再設計し、State 列を広げて `stopped` が収まることを確認した。
- Activity log の app data path 解決失敗と compaction path 解決失敗が空ログ/成功扱いに見える余地があった。
  - 対応: `required_activity_path` を読み込み・圧縮にも使い、読み込み失敗は failed activity record、required audit write/compaction 失敗は操作失敗として表面化するようにした。
- `container system stop` 相当の停止状態では、エラーが複数バナーとして並び `container system start` の導線がなかった。
  - 対応: system status が `running` 以外の場合は専用の system panel を出し、固定 `container system start` を実行するボタンを追加した。実行は activity log 必須書き込みとして記録する。
- Containers 画面で Memory 表示と Start ボタンが近く、状態によって重なる余地があった。
  - 対応: Containers table の最小幅と列配分、`.mini-meter` の幅制約を調整し、1280px / 1120px / 720px で Memory-Action overlap 0 件を確認した。
- Containers エリアと詳細エリアが横並びで互いに狭く見えた。
  - 対応: 選択中 container の詳細を右側 drawer 表示に変更し、Containers table は主画面幅を使えるようにした。

## 関連 Issue / PR

- Issue: https://github.com/yoshhiide/container-ui/issues/1
- PR: 未作成
