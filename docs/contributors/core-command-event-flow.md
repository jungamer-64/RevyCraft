# `CoreCommand` / `GameplayCommand` / `GameplayTransaction` / `CoreEvent` の流れ

この文書は、play 処理と login 処理で `CoreCommand`、`GameplayCommand`、`GameplayTransaction`、`CoreEvent` がどうつながるかを見るための正本です。

## 一枚で見る流れ

```text
client packet
  -> protocol plugin decode
  -> CoreCommand
  -> runtime dispatch
     -> direct-core command
        -> ServerCore::apply_command(...)
        -> Vec<TargetedEvent>
     -> gameplay-owned command
        -> GameplayCommand
        -> gameplay plugin callback
        -> GameplayTransaction commit
        -> Vec<TargetedEvent>
  -> runtime dispatch
  -> protocol plugin encode
  -> wire packets
```

login は gameplay transaction の special-case です。

```text
login/auth flow
  -> CoreCommand::LoginStart
  -> GameplayTransaction::begin_login(...)
  -> gameplay plugin HandlePlayerJoin
  -> GameplayTransaction::finalize_login(...)
  -> transaction commit
  -> CoreEvent 群
```

## 型の役割

- `CoreCommand`
  runtime / protocol 境界で使う semantic input です。定義は [`../../crates/core/mc-core/src/events.rs`](../../crates/core/mc-core/src/events.rs) にあります。
- `GameplayCommand`
  gameplay plugin に見せる gameplay-owned command だけを抜き出した入力です。`CoreCommand` から分離されます。定義は [`../../crates/core/mc-core/src/events.rs`](../../crates/core/mc-core/src/events.rs) にあります。
- `GameplayTransaction`
  gameplay callback 単位で host が開始する invocation-scoped transaction です。plugin はここを通じて world / player / inventory / block を読み書きします。`Ok(())` のときだけ commit されます。定義は [`../../crates/core/mc-core/src/core/transaction.rs`](../../crates/core/mc-core/src/core/transaction.rs) にあります。
- `CoreEvent`
  core から外へ出る出力です。最終的に protocol plugin が encode します。定義は [`../../crates/core/mc-core/src/events.rs`](../../crates/core/mc-core/src/events.rs) にあります。
- `TargetedEvent`
  `CoreEvent` に配送先を付けた wrapper です。runtime はこれを session / connection / broadcast へ dispatch します。

## command の分岐点

runtime 側の本体は [`../../crates/runtime/server-runtime/src/runtime/kernel.rs`](../../crates/runtime/server-runtime/src/runtime/kernel.rs) にあります。現在の分岐は大きく 3 種類です。

### login special-case

`CoreCommand::LoginStart` は gameplay profile があれば `handle_player_join(...)` へ入り、transaction の `begin_login(...)` / `finalize_login(...)` を経由します。実装は [`../../crates/plugin/mc-plugin-host/src/plugin_host/profiles/gameplay.rs`](../../crates/plugin/mc-plugin-host/src/plugin_host/profiles/gameplay.rs) と [`../../crates/core/mc-core/src/core/transaction.rs`](../../crates/core/mc-core/src/core/transaction.rs) にあります。

### direct-core command

次の command は gameplay policy を通らず、core が直接処理します。

- `UpdateClientView`
- `ClientStatus`
- `InventoryTransactionAck`
- `InventoryClick`
- `CloseContainer`
- `KeepAliveResponse`
- `Disconnect`

特に `InventoryClick` は [`../../crates/core/mc-core/src/core/inventory/click.rs`](../../crates/core/mc-core/src/core/inventory/click.rs) で直接処理されます。gameplay plugin transaction は経由しません。

### gameplay-owned command

次の command は gameplay policy へ渡されます。

- `MoveIntent`
- `SetHeldSlot`
- `CreativeInventorySet`
- `DigBlock`
- `PlaceBlock`
- `UseBlock`

ここで runtime が `CoreCommand` を `GameplayCommand` へ落とし、gameplay plugin の `handle_command(...)` を transaction 上で実行します。plugin は host mutation API を通じて draft state を更新し、commit 時に canonical `CoreEvent` 群へ反映されます。

## login 時に何が足されるか

`GameplayTransaction::finalize_login(...)` は gameplay callback が成功したあと、runtime bootstrap に必要な event をまとめて積みます。

- `LoginAccepted`
- `PlayBootstrap`
- `ChunkBatch`
- `InventoryContents`
- `SelectedHotbarSlotChanged`
- 既存 player の spawn event

この順番を追いたいときは [`../../crates/core/mc-core/src/core/transaction.rs`](../../crates/core/mc-core/src/core/transaction.rs) を読むのが最短です。

## runtime 側の受け渡し

core の前後で見るべき runtime 側の入口は次です。

- play packet の decode
  [`../../crates/runtime/server-runtime/src/runtime/session/play.rs`](../../crates/runtime/server-runtime/src/runtime/session/play.rs)
- command の適用と event dispatch
  [`../../crates/runtime/server-runtime/src/runtime/core_loop/events.rs`](../../crates/runtime/server-runtime/src/runtime/core_loop/events.rs)
- outgoing packet の encode
  [`../../crates/runtime/server-runtime/src/runtime/session/outgoing.rs`](../../crates/runtime/server-runtime/src/runtime/session/outgoing.rs)

## この分割で守っていること

- raw slot や version ごとの inventory quirks は protocol plugin が吸収する
- core が受け取るのは semantic な `CoreCommand`
- gameplay plugin は `GameplayCommand` と host transaction API だけに集中する
- runtime は dispatch と session orchestration に徹する

protocol / gameplay の責務境界を reload 観点で見たいときは [`reload-semantics-and-boundaries.md`](reload-semantics-and-boundaries.md) を参照してください。
