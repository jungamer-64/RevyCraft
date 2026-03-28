# `core` reloadable boundary と `reload runtime` 設計

この文書は、`ServerCore` を reload 境界の内側へ移し、接続を切らずに live session を完全保持したまま runtime を更新する contributors 向け正本です。ここでは、現在の `reload runtime <mode>` 実装が採用している `core` migration の意味論を説明します。旧 `reload plugins` / `reload generation` / `reload config` の公開 surface は扱いません。

operator 向けの command surface と permission は [`../operators/configuration-and-reload.md`](../operators/configuration-and-reload.md)、reload 全体の意味論は [`reload-semantics-and-boundaries.md`](reload-semantics-and-boundaries.md) を参照してください。

## 目的

- `ServerCore` を boot-time state ではなく reloadable runtime state として扱う
- play 中の接続を切らずに `ServerCore` を差し替える
- `reload runtime full` で artifact / topology / core migration を単一 transaction として扱う
- `cursor`、open window、keepalive、dropped item、active mining、view/chunk state を含む live-only state を完全保持する

非目的:

- `static.bootstrap.online_mode`、`level_type`、`world_dir` を live 変更可能にすること
- `static.plugins.*` を live 変更可能にすること
- legacy な remote-admin schema を復活させること
- `storage_profile` を restart なしで切り替えること
- persistent storage schema と `core` migration blob を共通化すること

## なぜ現行 `snapshot -> from_snapshot` では足りないか

現行 runtime は [`../../crates/runtime/revy-server-runtime/src/runtime/kernel.rs`](../../crates/runtime/revy-server-runtime/src/runtime/kernel.rs) の `RuntimeKernel` が単一の `ServerCore` を保持し、reload context には `WorldSnapshot` を渡します。

しかし `WorldSnapshot` は永続化向けの形であり、live session を完全移行するには不足しています。

- [`../../crates/core/mc-core/src/core/mod.rs`](../../crates/core/mc-core/src/core/mod.rs) の `ServerCore::snapshot()` は online player を persisted player として保存する
- [`../../crates/core/mc-core/src/core/inventory/lifecycle.rs`](../../crates/core/mc-core/src/core/inventory/lifecycle.rs) の `persisted_online_player_snapshot_state(...)` は `cursor` や active container の中身を inventory へ畳み込む
- [`../../crates/core/mc-core/src/core/mod.rs`](../../crates/core/mc-core/src/core/mod.rs) の `ServerCore::from_snapshot(...)` は world / block_entities / saved_players だけを復元し、online player の entity、session、keepalive、window state は復元しない
- [`../../crates/core/mc-core/src/world.rs`](../../crates/core/mc-core/src/world.rs) の `WorldSnapshot` は `meta` / `chunks` / `block_entities` / `players` だけを持ち、dropped item や active mining を表現しない

一方で protocol / gameplay reload は [`../../crates/plugin/mc-plugin-host/src/host/support/reload.rs`](../../crates/plugin/mc-plugin-host/src/host/support/reload.rs) の session transfer blob を export / import して live session を継続できます。`core` だけが同等の migration 口を持たないため、`snapshot -> from_snapshot` をそのまま使うと「接続は残るが core 側では player が offline 扱いになる」状態になります。

この設計では `WorldSnapshot` を保存用 schema として残しつつ、reload 専用の process-local state blob を別に導入します。

## 新しい公開 surface

公開入口は `reload runtime <mode>` に一本化します。

- `ServerSupervisor::reload_runtime(mode)`
- local console: `reload runtime artifacts`
- local console: `reload runtime topology`
- local console: `reload runtime core`
- local console: `reload runtime full`
- built-in gRPC: `ReloadRuntime { mode }`
- admin permission: `reload-runtime`

`RuntimeReloadMode` は次を持つ前提にします。

- `Artifacts`
  active selection を固定したまま artifact 差分だけを reload する
- `Topology`
  最新 config の `network` / `topology` を materialize して listener / routing generation を切り替える
- `Core`
  最新 config を読み、core に投影される差分だけを取り込みつつ `ServerCore` を migration する
- `Full`
  最新 config から selection / topology / core migration をまとめて評価し、成功時のみ一括 commit する

旧 `reload plugins` / `reload generation` / `reload config` は設計上の surface から外します。この文書では互換 alias としても扱いません。

## 内部責務の再編

`RuntimeServer` の state owner は次のように読み替えます。

- `SelectionManager`
  active config と reload candidate selection を保持する
- `TopologyManager`
  active / draining generation と listener worker を保持する
- `RuntimeKernel`
  単なる `ServerCore` owner ではなく、`core` migration の export / materialize / reattach / swap / rollback を担う `core runtime owner` として振る舞う
- `SessionRegistry`
  live session handle と connection-level metadata を保持する
- `ReloadCoordinator`
  config source、consistency gate、shutdown request を保持する

`RuntimeKernel` は次の内部概念を持つ前提にします。

### `CoreRuntimeStateBlob`

`WorldSnapshot` を含みつつ、それだけでは表現できない live-only state を追加した process-local blob です。persistent storage schema ではなく、reload transaction 中だけ有効なメモリ内表現として扱います。

最低限含めるもの:

- world snapshot
- dropped item state
- active mining state
- online player session state
- keepalive scheduler state
- session-scoped inventory window state
- view/chunk tracking state
- world-backed chest / furnace viewer state

### `SessionReattachRecord`

live session を candidate core へ再接続禁止で張り替えるための最小単位です。次を束ねます。

- `connection_id`
- `player_id`
- `entity_id`
- `phase`
- protocol generation
- gameplay generation
- client view
- inventory window state
- `cursor`
- keepalive state
- session-linked furnace / chest state

`SessionReattachRecord` は `WorldSnapshot` の player entry を置き換えるものではなく、saved-player と online-player を分けて扱うための runtime-only metadata とします。

### `CoreMigrationPlan`

reload の途中成果物です。commit まで mutable global state を書き換えず、失敗時に旧 core をそのまま維持できるようにします。

最低限持つもの:

- exported `CoreRuntimeStateBlob`
- candidate `ServerCore`
- reattach 対象 `SessionReattachRecord` 群
- protocol / gameplay へ送る resync event 群
- rollback に必要な error context

## 完全保持の対象

`reload runtime core` と `reload runtime full` は次を保持対象にします。

- player / entity identity
- open window と `window_id`
- `cursor`
- pending keepalive id と timeout scheduling
- dropped item と active mining の進行状態
- client view と loaded chunk state
- world-backed chest / furnace と viewer state
- session から参照される protocol / gameplay generation pin

完全保持は「できれば維持する」ではなく acceptance の基準です。維持できない candidate は rollback 対象とします。

## phase ごとの扱い

- `Status`
  protocol session blob だけで継続する。core reattach は不要。
- `Login`
  protocol / auth / gameplay の phase-local state を維持するが、online player reattach は行わない。
- `Play`
  `SessionReattachRecord` を使って full reattach する。

`LoginAccepted` は再送しません。play 中 session は同一 connection のまま継続し、reattach 後の差分 resync だけを送ります。

## mode ごとの内部動作

### `reload runtime artifacts`

現行 `reload plugins` に近い mode です。

1. consistency write lock を取得
2. live protocol / gameplay session snapshot と `core` runtime blob を固定
3. current selection config で plugin host を reconcile
4. protocol / gameplay / storage generation を migration
5. selection を差し替える

core swap と topology generation swap は行いません。

### `reload runtime topology`

現行 `reload generation` に近い mode です。

1. 最新 config を load する
2. restart-required な static 差分が無いことを確認する
3. `network` / `topology` だけを candidate generation に反映する
4. listener / routing を materialize する
5. active generation を切り替える

selection と core は current state を維持します。既存 session の継続は draining generation で扱います。

### `reload runtime core`

最新 config を load し、selection / topology を固定したまま core に投影される差分だけを反映する mode です。

1. 最新 config を load し、`plan_core_reload` で restart-required ではない core projection だけを抽出する
2. consistency write lock を取得
3. current selection と active topology generation を固定する
4. live runtime から `CoreRuntimeStateBlob` と `SessionReattachRecord` を export する
5. candidate core を materialize する
6. play session を candidate core へ reattach する
7. protocol / gameplay generation へ必要な resync event を発行する
8. すべて成功した場合のみ core owner と active config を swap する
9. 途中で失敗した場合は旧 core を維持し、candidate を破棄する

この mode で反映される config 差分は `level_name` / `game_mode` / `difficulty` / `view_distance` / `max_players` に限られます。

### `reload runtime full`

artifact / topology / core の全体更新を単一 transaction として扱う mode です。

1. consistency write lock を取得
2. 最新 config を load し、restart-required な static 差分が無いことを確認する
3. candidate selection を resolve する
4. candidate topology generation を materialize する
5. `CoreRuntimeStateBlob` を export し、candidate core を materialize する
6. protocol / gameplay / storage generation migration と play session reattach を行う
7. selection / topology / core の commit 条件がそろったときだけ一括反映する
8. core migration が失敗した場合は topology / selection も commit しない

`full` は「途中まで切り替わる best-effort reload」ではなく、`core` swap を含む commit point までは transaction として扱います。

## migration algorithm

`core` migration の順序は固定します。

1. consistency write lock を取得する
2. protocol / gameplay / storage selection を固定する
3. live runtime から `CoreRuntimeStateBlob` を export する
4. candidate core を materialize する
5. live session を candidate core へ reattach する
6. protocol / gameplay 側へ必要な resync event を発行する
7. 成功時のみ core owner を swap する
8. 失敗時は旧 core を維持し、candidate を破棄する

設計上の要点:

- `ServerCore::from_snapshot(...)` は saved-player を戻す helper として残すが、online player reattach には使わない
- online player を saved-player として戻す path を通さない
- `entity_id` は export 前後で不変とする
- keepalive scheduler は `pending_keep_alive_id`、`last_keep_alive_sent_at`、`next_keep_alive_at` を含めてそのまま移す
- world-backed chest / furnace は viewer set と block entity の両方を同期する

## failure policy と互換境界

### restart-required のまま残るもの

- `static.bootstrap.online_mode`
- `static.bootstrap.level_type`
- `static.bootstrap.world_dir`
- `static.plugins.*`
- `storage_profile` の切替

`core` mode と `full` mode はこれらを跨ぎません。

### rollback と fail-fast

基本方針は rollback-first です。

- candidate core materialize failure
  旧 core を維持する
- session reattach failure
  旧 core を維持する
- protocol / gameplay session blob と `core` blob の version mismatch
  旧 core を維持する

fail-fast は rollback 不可能な不整合に限ります。たとえば「旧 core へ戻せないまま protocol / gameplay generation の active state が破損した」ようなケースだけを対象にします。通常の candidate failure では session を切断しません。

### blob schema の扱い

`CoreRuntimeStateBlob` は process 内専用です。

- persistent storage schema と共通化しない
- `storage` plugin の `load_snapshot` / `save_snapshot` に露出しない
- `storage` reload の `import_runtime_state` とは役割を分ける

## testing と acceptance

最低限の acceptance は次です。

- play 中の Java / Bedrock session が `reload runtime core` 後も切断されず継続する
- selected hotbar、`cursor`、open chest / furnace、`window_id`、container contents が維持される
- pending keepalive id と timeout scheduling が維持される
- dropped item と active mining の進行状態が維持される
- `reload runtime full` で artifact / topology / core がまとめて切り替わる
- candidate core materialize failure で old core が維持される
- reattach failure で rollback され、接続が継続する
- consistency gate 中は session command が停止し、完了後に再開する
- `Status` / `Login` / `Play` が phase ごとに正しく扱われる
- old API 前提の operator docs / permission / proto が残っていない

## 読む順番

1. [`reload-semantics-and-boundaries.md`](reload-semantics-and-boundaries.md)
2. [`runtime-and-plugin-architecture.md`](runtime-and-plugin-architecture.md)
3. [`../../crates/runtime/revy-server-runtime/src/runtime/kernel.rs`](../../crates/runtime/revy-server-runtime/src/runtime/kernel.rs)
4. [`../../crates/runtime/revy-server-runtime/src/runtime/core_loop/reload.rs`](../../crates/runtime/revy-server-runtime/src/runtime/core_loop/reload.rs)
5. [`../../crates/plugin/mc-plugin-host/src/host/support/reload.rs`](../../crates/plugin/mc-plugin-host/src/host/support/reload.rs)
