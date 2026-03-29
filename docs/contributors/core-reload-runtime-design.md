# `reload runtime` の設計と `core` 移行

- 対象読者: `reload runtime <mode>`、`ReloadCoordinator`、`core` 移行の内部設計を追いたい contributors
- この文書で扱う範囲: 公開 reload surface、mode ごとの意味論、`consistency_gate`、`CoreRuntimeStateBlob`、rollback policy、acceptance
- この文書で扱わないこと: operator 向けの command 手順、plugin authoring の詳細、target crate split の背景説明
- 次に読む文書: [`core-command-event-flow.md`](core-command-event-flow.md)

この文書は、`ServerCore` を reload 境界の内側へ移し、接続を切らずに live session を保持したまま runtime を更新する contributor 向け正本です。operator 向けの command surface と permission は [`../operators/configuration-and-reload.md`](../operators/configuration-and-reload.md) を参照してください。

## 公開 `reload` surface

外向けの入口は `ServerSupervisor::reload_runtime(mode)` です。

- `reload runtime artifacts`
- `reload runtime topology`
- `reload runtime core`
- `reload runtime full`

`RuntimeReloadMode` は次を持つ前提です。

- `Artifacts`
  active selection を固定したまま artifact 差分だけを reload する
- `Topology`
  最新 config の `network` / `topology` を materialize して listener / routing generation を切り替える
- `Core`
  最新 config を読み、core に投影される差分だけを取り込みつつ `ServerCore` を migration する
- `Full`
  最新 config から selection / topology / core migration をまとめて評価し、成功時のみ一括 commit する

旧 `reload plugins` / `reload generation` / `reload config` は設計上の surface から外します。

## reload の前提

reload は reload-capable supervisor boot が必要です。`server-bootstrap` の通常起動では reload host を伴う boot path を使い、手動 `reload` と watch `reload` を許可します。reload host を持たない custom boot path では手動 `reload` も watch `reload` も使えません。

`plugins.reload_watch` や `topology.reload_watch` は watch trigger であり、実際に実行する処理は `reload runtime full` と同じ意味論を持ちます。

reload の並行実行は `ReloadCoordinator` の `reload_serial` で直列化します。手動 `reload` はここで待機し、watch `reload` は他の reload / upgrade が進行中なら skip して次の poll へ回します。

## `protocol` / `gameplay` / `core` の境界

reload を読むときの責務分割は次です。

- protocol
  wire format、routing、transport 固有 session state、session transfer blob を持つ
- gameplay
  semantic `GameplayCommand` を評価し、callback 単位の `GameplayTransaction` を commit する
- core
  world / entity / inventory / keepalive / dropped item / active mining を含む canonical runtime state を持つ

`core` を reloadable boundary に出すことで、protocol 固有 session blob と gameplay 固有 session blob に加えて、world-semantic な live state も migration 対象へ入ります。

## なぜ現行 `snapshot -> from_snapshot` では足りないか

現行 runtime は [`../../crates/runtime/revy-server-runtime/src/runtime/kernel.rs`](../../crates/runtime/revy-server-runtime/src/runtime/kernel.rs) の `RuntimeKernel` が単一の `ServerCore` を保持し、reload context には `WorldSnapshot` を渡します。

しかし `WorldSnapshot` は永続化向けの形であり、live session を完全移行するには不足しています。

- [`../../crates/core/revy-voxel-core/src/core/mod.rs`](../../crates/core/revy-voxel-core/src/core/mod.rs) の `ServerCore::snapshot()` は online player を persisted player として保存する
- [`../../crates/core/revy-voxel-core/src/core/inventory/lifecycle.rs`](../../crates/core/revy-voxel-core/src/core/inventory/lifecycle.rs) の `persisted_online_player_snapshot_state(...)` は `cursor` や active container の中身を inventory へ畳み込む
- [`../../crates/core/revy-voxel-core/src/core/mod.rs`](../../crates/core/revy-voxel-core/src/core/mod.rs) の `ServerCore::from_snapshot(...)` は world / block_entities / saved_players だけを復元し、online player の entity、session、keepalive、window state は復元しない
- [`../../crates/core/revy-voxel-core/src/world.rs`](../../crates/core/revy-voxel-core/src/world.rs) の `WorldSnapshot` は `meta` / `chunks` / `block_entities` / `players` だけを持ち、dropped item や active mining を表現しない

一方で protocol / gameplay reload は [`../../crates/plugin/mc-plugin-host/src/host/support/reload.rs`](../../crates/plugin/mc-plugin-host/src/host/support/reload.rs) の session transfer blob を export / import して live session を継続できます。`core` だけが同等の migration 口を持たないため、`snapshot -> from_snapshot` をそのまま使うと「接続は残るが core 側では player が offline 扱いになる」状態になります。

## 内部責務の再編

`RuntimeServer` の state owner は次のように読み替えます。

- `SelectionManager`
  active config と reload candidate selection を保持する
- `TopologyManager`
  active / draining generation と listener worker を保持する
- `RuntimeKernel`
  `core` migration の export / materialize / reattach / swap / rollback を担う `core runtime owner` として振る舞う
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
- view / chunk tracking state
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

## mode ごとの内部動作

mode ごとの config 射影と restart-required 判定の正本は `revy-server-config` の `ServerConfig::plan_topology_reload` / `plan_core_reload` / `plan_full_reload` です。runtime 側はこの plan を実行する責務に寄せます。

### `reload runtime artifacts`

1. `reload_serial` 下で modified plugin を stage する
2. write consistency lock を取得する
3. live protocol / gameplay session snapshot と `core` runtime blob を固定する
4. staged candidate を live runtime snapshot に対して finalize する
5. selection を差し替える

core swap と topology generation swap は行いません。

### `reload runtime topology`

1. restart-required な static 差分が無いことを確認する
2. current config を clone する
3. loaded config から `network` / `topology` だけ差し替える
4. candidate topology generation を materialize する
5. active generation を切り替え、旧 generation を draining へ移す

selection と core は current state を維持します。

### `reload runtime core`

1. `reload_serial` 下で candidate config plan を確定する
2. write consistency lock を取得する
3. current selection と active topology generation を固定する
4. live runtime から `CoreRuntimeStateBlob` を export する
5. candidate core を materialize する
6. play session を candidate core へ reattach する
7. protocol / gameplay へ必要な resync event を発行する
8. 成功時のみ core owner を swap する

失敗時は旧 core を維持し、session を切断しません。

### `reload runtime full`

1. `reload_serial` 下で config plan、plugin-host candidate、topology candidate を stage する
2. write consistency lock を取得する
3. live runtime snapshot に対して staged plugin candidate を finalize する
4. `CoreRuntimeStateBlob` を export して candidate core を materialize する
5. plugin generation migration と session reattach を実行する
6. commit 条件がそろった場合のみ selection / topology / core を一括反映する

`full` は `config-scoped reload` の別名ではなく、artifact / topology / core をまとめた公開 mode です。

## `reload_serial` と `consistency_gate`

reload orchestration には 2 つの同期原語があります。

- `reload_serial`
  reload / upgrade staging の多重実行を防ぐ mutex
- `consistency_gate`
  quiescent な live snapshot と commit point を守る async `RwLock<()>`

`consistency_gate` は次の目的に使います。

- session spawn、command dispatch、event dispatch、tick 側は read lock を取る
- reload commit / upgrade freeze 側は write lock を取る

結果として次が成り立ちます。

- in-flight の reader がいるあいだ reload commit は待機する
- reload が write lock を持っているあいだ、新しい session command の進行は止まる
- heavy な plugin load / candidate staging は gate の外で進められる
- `full` は selection / topology / core の commit point を同じ write lock の中で完結する

## `generation` と移行の境界

runtime には少なくとも 2 種類の世代があります。

- topology generation
  listener と routing の世代
- plugin generation
  protocol / gameplay / storage / auth / admin-surface plugin の世代

`core` migration は topology generation のような別番号を持つ公開概念ではなく、live session を同一 connection / entity identity のまま新しい core owner に張り替える内部 operation として扱います。

## phase ごとの扱い

- `Status`
  protocol session blob だけで継続する。core reattach は不要。
- `Login`
  protocol / auth / gameplay の phase-local state を維持するが、online player reattach は行わない。
- `Play`
  `SessionReattachRecord` を使って full reattach する。

`LoginAccepted` は再送しません。play 中 session は同一 connection のまま継続し、reattach 後の差分 resync だけを送ります。

## 移行アルゴリズム

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

## `failure policy` と互換境界

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

fail-fast は rollback 不可能な不整合に限ります。通常の candidate failure では session を切断しません。

### blob schema の扱い

`CoreRuntimeStateBlob` は process 内専用です。

- persistent storage schema と共通化しない
- `storage` plugin の `load_snapshot` / `save_snapshot` に露出しない
- `storage` reload の `import_runtime_state` とは役割を分ける

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

## テストと受け入れ条件

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

1. [`runtime-and-plugin-architecture.md`](runtime-and-plugin-architecture.md)
2. [`../../crates/runtime/revy-server-runtime/src/runtime/reload_coordinator.rs`](../../crates/runtime/revy-server-runtime/src/runtime/reload_coordinator.rs)
3. [`../../crates/runtime/revy-server-runtime/src/runtime/core_loop/reload.rs`](../../crates/runtime/revy-server-runtime/src/runtime/core_loop/reload.rs)
4. [`../../crates/runtime/revy-server-runtime/src/runtime/topology_manager.rs`](../../crates/runtime/revy-server-runtime/src/runtime/topology_manager.rs)
5. [`../../crates/plugin/mc-plugin-host/src/host/support/reload.rs`](../../crates/plugin/mc-plugin-host/src/host/support/reload.rs)
