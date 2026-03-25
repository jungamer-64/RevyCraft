# `CoreCommand` / `GameplayEffect` / `CoreEvent` の流れ

この文書は、play 処理と login 処理で `CoreCommand`、`GameplayJoinEffect`、`GameplayEffect`、`CoreEvent` がどうつながるかを見るための正本です。

## 一枚で見る流れ

```text
client packet
  -> protocol plugin decode
  -> CoreCommand
  -> ServerCore::apply_command_with_policy(...)
     -> direct-core command
        -> Vec<TargetedEvent>
     -> gameplay-owned command
        -> GameplayPolicyResolver::handle_command(...)
        -> GameplayEffect
        -> ServerCore::apply_gameplay_effect(...)
        -> Vec<TargetedEvent>
  -> runtime dispatch
  -> protocol plugin encode
  -> wire packets
```

login だけは別経路で `GameplayJoinEffect` を使います。

```text
login/auth flow
  -> CoreCommand::LoginStart
  -> ServerCore::login_player_with_policy(...)
  -> GameplayPolicyResolver::handle_player_join(...)
  -> GameplayJoinEffect
  -> login_initial_events + join events
  -> CoreEvent 群
```

## 型の役割

- `CoreCommand`
  core へ入る入力です。定義は [`../../crates/core/mc-core/src/events.rs`](../../crates/core/mc-core/src/events.rs) にあります。
- `GameplayJoinEffect`
  join 時だけ使う gameplay 側の初期化結果です。inventory や selected hotbar slot を差し替えられます。定義は [`../../crates/core/mc-core/src/gameplay.rs`](../../crates/core/mc-core/src/gameplay.rs) にあります。
- `GameplayEffect`
  gameplay policy が返す中間結果です。mutation と emitted event を持ちます。定義は [`../../crates/core/mc-core/src/gameplay.rs`](../../crates/core/mc-core/src/gameplay.rs) にあります。
- `CoreEvent`
  core から外へ出る出力です。最終的に protocol plugin が encode します。定義は [`../../crates/core/mc-core/src/events.rs`](../../crates/core/mc-core/src/events.rs) にあります。
- `TargetedEvent`
  `CoreEvent` に配送先を付けた wrapper です。runtime はこれを session / connection / broadcast へ dispatch します。

## command の分岐点

`ServerCore::apply_command_with_policy(...)` の本体は [`../../crates/core/mc-core/src/core/command.rs`](../../crates/core/mc-core/src/core/command.rs) にあります。現在の分岐は大きく 3 種類です。

### login special-case

`CoreCommand::LoginStart` は `apply_login_command_with_policy(...)` へ入り、`GameplayJoinEffect` を経由します。実装は [`../../crates/core/mc-core/src/core/login.rs`](../../crates/core/mc-core/src/core/login.rs) にあります。

### direct-core command

次の command は gameplay policy を通らず、core が直接処理します。

- `UpdateClientView`
- `ClientStatus`
- `InventoryTransactionAck`
- `InventoryClick`
- `CloseContainer`
- `KeepAliveResponse`
- `Disconnect`

特に `InventoryClick` は [`../../crates/core/mc-core/src/core/inventory/click.rs`](../../crates/core/mc-core/src/core/inventory/click.rs) で直接処理されます。`GameplayEffect` は経由しません。

### gameplay-owned command

次の command は gameplay policy へ渡されます。

- `MoveIntent`
- `SetHeldSlot`
- `CreativeInventorySet`
- `DigBlock`
- `PlaceBlock`
- `UseBlock`

ここで `GameplayPolicyResolver::handle_command(...)` が `GameplayEffect` を返し、その後 [`../../crates/core/mc-core/src/core/mutation.rs`](../../crates/core/mc-core/src/core/mutation.rs) が mutation と emitted event を消費します。

## login 時に何が足されるか

`login_player_with_policy(...)` は gameplay 側の join effect を適用したあと、runtime bootstrap に必要な event をまとめて積みます。

- `LoginAccepted`
- `PlayBootstrap`
- `ChunkBatch`
- `InventoryContents`
- `SelectedHotbarSlotChanged`
- 既存 player の spawn event

この順番を追いたいときは [`../../crates/core/mc-core/src/core/login.rs`](../../crates/core/mc-core/src/core/login.rs) を読むのが最短です。

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
- gameplay plugin は semantic policy だけに集中する
- runtime は dispatch と session orchestration に徹する

protocol / gameplay の責務境界を reload 観点で見たいときは [`reload-semantics-and-boundaries.md`](reload-semantics-and-boundaries.md) を参照してください。
