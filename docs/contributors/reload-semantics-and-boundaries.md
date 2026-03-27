# reload の意味論と reloadable boundary

この文書は、RevyCraft の reload を contributors 向けに整理した正本です。ここでは、現在実装されている `reload runtime <mode>` の意味論を扱います。旧 `reload plugins` / `reload generation` / `reload config` の公開 surface は扱いません。operator 向けの command 説明は [`../operators/configuration-and-reload.md`](../operators/configuration-and-reload.md)、`core` migration の詳細設計は [`core-reload-runtime-design.md`](core-reload-runtime-design.md) を参照してください。

## 公開 reload surface

外向けの入口は `ServerSupervisor::reload_runtime(mode)` です。

- `reload runtime artifacts`
- `reload runtime topology`
- `reload runtime core`
- `reload runtime full`

admin surface、gRPC、permission もこの 4 mode を前提にそろえます。

## reload の前提

reload は reload-capable supervisor boot が必要です。`server-bootstrap` の通常起動では reload host を伴う boot path を使い、manual reload と watch reload を許可します。reload host を持たない custom boot path では manual reload も watch reload も使えません。

`plugins.reload_watch` や `topology.reload_watch` は watch trigger であり、実際に実行する処理は `reload runtime full` と同じ意味論を持ちます。

reload の並行実行は `ReloadCoordinator` の `reload_serial` で直列化します。manual reload はここで待機し、watch reload は他の reload / upgrade が進行中なら skip して次の poll へ回します。

## mode ごとの内部動作

mode ごとの config 射影と restart-required 判定の正本は `server-config` の
`ServerConfig::plan_topology_reload` / `plan_core_reload` / `plan_full_reload` です。runtime
側はこの plan を実行する責務に寄せます。

### `artifacts`

active selection を固定したまま managed plugin の artifact 差分だけを reload します。

流れ:

1. `reload_serial` 下で modified plugin を stage する
2. write consistency lock を取得
3. live protocol / gameplay session snapshot と `core` runtime blob を固定
4. staged candidate を live runtime snapshot に対して finalize する
5. selection を差し替える

core swap と topology generation swap は行いません。

### `topology`

最新 config を読みますが、candidate に反映するのは `network` と `topology` だけです。

流れ:

1. restart-required な static 差分が無いことを確認
2. current config を clone
3. loaded config から `network` / `topology` だけ差し替える
4. candidate topology generation を materialize
5. active generation を切り替え、旧 generation を draining へ移す

selection と core は current state を維持します。

### `core`

selection / topology / transport を変えずに `ServerCore` だけを migration します。

流れ:

1. `reload_serial` 下で candidate config plan を確定する
2. write consistency lock を取得
3. current selection と active topology generation を固定
4. live runtime から `CoreRuntimeStateBlob` を export
5. candidate core を materialize
6. play session を candidate core へ reattach
7. protocol / gameplay へ必要な resync event を発行
8. 成功時のみ core owner を swap

失敗時は旧 core を維持し、session を切断しません。

### `full`

selection / topology / core migration を単一 transaction として扱います。

流れ:

1. `reload_serial` 下で config plan、plugin-host candidate、topology candidate を stage する
2. write consistency lock を取得
3. live runtime snapshot に対して staged plugin candidate を finalize する
4. `CoreRuntimeStateBlob` を export して candidate core を materializeする
5. plugin generation migration と session reattach を実行する
6. commit 条件がそろった場合のみ selection / topology / core を一括反映する

`full` は `config-scoped reload` の別名ではなく、artifact / topology / core をまとめた公開 mode です。

## reload serial と consistency gate

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

この性質は protocol reload 系テストで検証されている性質をそのまま設計の基盤にし、`core` migration でも同じ gate を使います。

gameplay callback の decontention は別の internal refactor で扱います。`RuntimeKernel` は gameplay callback を snapshot clone 上の journal transaction として lock 外で実行しますが、`consistency_gate` の read/write scope 自体はここでは変えません。

## rollback と transaction 境界

`artifacts` と `topology` は現行実装に近い best-effort reload ですが、`core` と `full` は rollback-first で扱います。

- `artifacts`
  candidate generation failure は旧 selection を維持する
- `topology`
  candidate listener / routing materialize failure は旧 generation を維持する
- `core`
  candidate core materialize failure、reattach failure、resync failure は old core 維持で rollback する
- `full`
  core migration が失敗した時点では selection / topology も commit しない

特に `full` は旧 `reload config` より transaction 性を強く持ちます。

## failure policy の意味

plugin kind ごとの failure policy は引き続き次です。

- protocol = `quarantine`
- gameplay = `quarantine`
- storage = `fail-fast`
- auth = `skip`
- admin-ui = `skip`

読み方:

- `skip`
  壊れた candidate を見送り、旧 generation を維持する
- `quarantine`
  壊れた candidate artifact や active plugin を隔離する
- `fail-fast`
  runtime 全体の重大障害として扱う

ただし `core` migration failure は plugin failure policy ではなく runtime rollback policy で扱います。既定動作は rollback-first で、rollback 不可能な不整合だけを fail-fast 条件にします。

## generation と migration の境界

runtime には少なくとも 2 種類の世代があります。

- topology generation
  listener と routing の世代
- plugin generation
  protocol / gameplay / storage / auth / admin-ui plugin の世代

`core` migration は topology generation のような別番号を持つ公開概念ではなく、live session を同一 connection / entity identity のまま新しい core owner に張り替える内部 operation として扱います。

## protocol / gameplay / core の境界

reload を読むときの責務分割は次です。

- protocol
  wire format、routing、transport 固有 session state、session transfer blob を持つ
- gameplay
  semantic `GameplayCommand` を評価し、callback 単位の `GameplayTransaction` を commit する
- core
  world / entity / inventory / keepalive / dropped item / active mining を含む canonical runtime state を持つ

`core` を reloadable boundary に出すことで、protocol 固有 session blob と gameplay 固有 session blob に加えて、world-semantic な live state も migration 対象へ入ります。

## 読む順番

1. [`core-reload-runtime-design.md`](core-reload-runtime-design.md)
2. [`../../crates/runtime/server-runtime/src/runtime/reload_coordinator.rs`](../../crates/runtime/server-runtime/src/runtime/reload_coordinator.rs)
3. [`../../crates/runtime/server-runtime/src/runtime/core_loop/reload.rs`](../../crates/runtime/server-runtime/src/runtime/core_loop/reload.rs)
4. [`../../crates/runtime/server-runtime/src/runtime/topology_manager.rs`](../../crates/runtime/server-runtime/src/runtime/topology_manager.rs)
5. [`runtime-and-plugin-architecture.md`](runtime-and-plugin-architecture.md)
