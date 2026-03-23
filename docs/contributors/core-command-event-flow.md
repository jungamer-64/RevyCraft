# `CoreCommand` / `CoreEvent` / `GameplayEffect` の流れ

## 概要

この文書は、RevyCraft の play 処理で `CoreCommand`、`GameplayEffect`、`CoreEvent` がどうつながるかを contributors 向けに整理したものです。型定義だけでは見えにくい「どこで入力になり、どこで中間結果になり、どこで client-facing な出力になるか」を 1 ページで追えるようにします。

## 対象読者

- `mc-core` と `server-runtime` の接続点を把握したい contributors
- protocol / gameplay / core の責務分離を型ベースで理解したい人

## この文書でわかること

- `CoreCommand`、`GameplayEffect`、`CoreEvent` の役割の違い
- play 処理と login 処理で型の流れがどう分かれるか
- gameplay-owned command と direct-core command の違い
- `TargetedEvent` が runtime でどのように dispatch されるか

## 関連資料

- [`repository-overview.md`](repository-overview.md)
- [`runtime-and-plugin-architecture.md`](runtime-and-plugin-architecture.md)
- [`reload-semantics-and-boundaries.md`](reload-semantics-and-boundaries.md)

## 先に押さえる結論

- `CoreCommand` は core へ入る入力表現です。多くは protocol plugin が decode して作りますが、`LoginStart` や `Disconnect` のように runtime が組み立てるものもあります。
- `GameplayEffect` は gameplay policy の評価結果です。packet ではなく、中間的な「mutation + 追加 event」の束です。
- `CoreEvent` は core が外へ出す出力表現です。最終的に runtime が `TargetedEvent` として dispatch し、protocol plugin が packet に encode します。
- login 時だけは `GameplayEffect` ではなく `GameplayJoinEffect` を使います。join 時の inventory や selected hotbar slot を初期化するためです。

## 一枚で見る流れ

```text
client packet
  -> protocol plugin `decode_play()`
  -> `CoreCommand`
  -> `RuntimeServer::apply_command(...)`
  -> `ServerCore::apply_command_with_policy(...)`
     -> direct-core command
        -> そのまま `Vec<TargetedEvent>`
     -> gameplay-owned command
        -> `GameplayPolicyResolver::handle_command(...)`
        -> `GameplayEffect`
        -> `ServerCore::apply_gameplay_effect(...)`
        -> `Vec<TargetedEvent>`
  -> `RuntimeServer::dispatch_events(...)`
  -> session outgoing
     -> `encode_login_success()` / `encode_disconnect()`
     -> or `encode_play_event(CoreEvent, ...)`
  -> wire packets
```

login は少しだけ別経路です。

```text
login/auth flow
  -> `CoreCommand::LoginStart`
  -> `ServerCore::login_player_with_policy(...)`
  -> `GameplayPolicyResolver::handle_player_join(...)`
  -> `GameplayJoinEffect`
  -> player 初期 state 更新 + join 時 event
  -> `Vec<TargetedEvent>`
```

## 型ごとの役割

### `CoreCommand`

`CoreCommand` は core に対する入力です。定義は [`crates/core/mc-core/src/events.rs`](/home/jgm64/デスクトップ/RenyCraftServer/crates/core/mc-core/src/events.rs#L27) にあります。

主な特徴は次です。

- protocol plugin が play packet を decode して作る
- runtime が login / disconnect などの lifecycle 操作として作ることもある
- そのまま state mutation を意味するのではなく、gameplay policy を通して解釈されることがある

### `GameplayEffect`

`GameplayEffect` は gameplay policy が返す評価結果です。定義は [`crates/core/mc-core/src/gameplay.rs`](/home/jgm64/デスクトップ/RenyCraftServer/crates/core/mc-core/src/gameplay.rs#L17) にあります。

中身は 2 つです。

- `mutations`
  core state を変更するための `GameplayMutation`
- `emitted_events`
  mutation を介さずにそのまま送りたい `TargetedEvent`

つまり `GameplayEffect` は「何を state に反映するか」と「どの event を直接出すか」をまとめた中間表現です。

### `CoreEvent`

`CoreEvent` は core から外へ出る出力です。定義は [`crates/core/mc-core/src/events.rs`](/home/jgm64/デスクトップ/RenyCraftServer/crates/core/mc-core/src/events.rs#L112) にあります。

主な役割は次です。

- login 完了や disconnect の通知
- inventory / block / entity / keepalive などの client-visible state change の通知
- runtime が session へ dispatch し、protocol plugin が wire packet へ encode する材料

### `TargetedEvent`

runtime が扱う実際の単位は `CoreEvent` 単体ではなく `TargetedEvent` です。`EventTarget` により、接続単位、player 単位、または `EveryoneExcept` を指定します。

そのため `CoreEvent` は「何が起きたか」、`TargetedEvent` は「誰に見せるか」を表します。

## command の分類

`ServerCore::apply_command_with_policy(...)` の分岐は [`crates/core/mc-core/src/core/command.rs`](/home/jgm64/デスクトップ/RenyCraftServer/crates/core/mc-core/src/core/command.rs#L26) にまとまっています。ここが関係理解の中心です。

| 分類 | 例 | 経路 | 補足 |
| --- | --- | --- | --- |
| login special-case | `LoginStart` | `handle_player_join()` -> `GameplayJoinEffect` | join 時だけ別の effect 型を使います |
| gameplay-owned | `MoveIntent`, `SetHeldSlot`, `CreativeInventorySet`, `DigBlock`, `PlaceBlock` | `handle_command()` -> `GameplayEffect` -> `apply_gameplay_effect()` | gameplay policy が意味づけします |
| direct-core inventory | `InventoryClick`, `InventoryTransactionAck` | core inventory logic が直接 `TargetedEvent` を返す | gameplay を通しません |
| direct-core maintenance | `UpdateClientView`, `ClientStatus`, `KeepAliveResponse`, `Disconnect` | core helper が直接処理 | transport / lifecycle 寄りです |

この分類を見ると、`CoreCommand` すべてが gameplay policy を通るわけではないことがわかります。

## `GameplayEffect` が `CoreEvent` になるまで

`GameplayEffect` の消費点は [`crates/core/mc-core/src/core/mutation.rs`](/home/jgm64/デスクトップ/RenyCraftServer/crates/core/mc-core/src/core/mutation.rs#L10) にあります。

流れは次のとおりです。

1. gameplay policy が `GameplayEffect` を返す
2. core が `effect.mutations` を順に適用する
3. 各 mutation の適用結果として `TargetedEvent<CoreEvent>` を生成する
4. 最後に `effect.emitted_events` をそのまま末尾へ足す

重要なのは、`GameplayEffect` 自体が wire 出力ではないことです。client に届くのは、mutation 適用の結果として生成された `CoreEvent` と、effect が直接 emit した `TargetedEvent` です。

## 例 1: `MoveIntent`

`MoveIntent` は gameplay-owned command です。

1. protocol plugin が移動 packet を `CoreCommand::MoveIntent` に decode する
2. gameplay policy が `GameplayMutation::PlayerPose` を含む `GameplayEffect` を返す
3. core が player snapshot を更新する
4. その結果として、他プレイヤー向けの `CoreEvent::EntityMoved` や、新しく可視になった chunk 向けの `CoreEvent::ChunkBatch` を生成する

つまり `MoveIntent` は、command 自体がそのまま event になるのではなく、「policy -> mutation -> event」という 2 段階を通ります。

## 例 2: `SetHeldSlot` の reject

`SetHeldSlot` も gameplay-owned command ですが、常に mutation を返すわけではありません。canonical policy は不正な slot を受けたとき、mutation なしで `CoreEvent::SelectedHotbarSlotChanged` を player 向けに emit します。

このケースでは、

- state mutation は起きない
- `GameplayEffect.emitted_events` だけが使われる

という形になります。`GameplayEffect` は「mutation 専用」ではなく、「re-sync event だけを返す容器」でもあります。

## 例 3: `InventoryClick`

`InventoryClick` は direct-core command です。[`crates/core/mc-core/src/core/inventory.rs`](/home/jgm64/デスクトップ/RenyCraftServer/crates/core/mc-core/src/core/inventory.rs#L20) で直接処理され、`GameplayEffect` を経由しません。

この経路では core が直接次のような event を返します。

- `InventoryTransactionProcessed`
- `InventoryContents`
- `InventorySlotChanged`
- `CursorChanged`

このため、「client の入力をいったん必ず gameplay へ渡す設計」ではなく、「inventory のように core が直接整合性を持つ領域もある」ことがわかります。

## 例 4: `LoginStart` と `GameplayJoinEffect`

`LoginStart` は gameplay-owned command ではなく、join special-case です。[`crates/core/mc-core/src/core/login.rs`](/home/jgm64/デスクトップ/RenyCraftServer/crates/core/mc-core/src/core/login.rs#L8) では次の順で処理されます。

1. runtime / auth flow が `CoreCommand::LoginStart` を作る
2. core が saved player または default player snapshot を用意する
3. gameplay policy が `handle_player_join()` で `GameplayJoinEffect` を返す
4. join effect が inventory や selected hotbar slot を player snapshot に反映する
5. そのあと `LoginAccepted`、`PlayBootstrap`、`ChunkBatch` などの初期 event を生成する

join で `GameplayJoinEffect` が別型になっているのは、「まだ play 中ではない player の初期 state を整える」責務があるためです。

## runtime での dispatch と encode

runtime 側では、play packet の decode は [`crates/runtime/server-runtime/src/runtime/session/play.rs`](/home/jgm64/デスクトップ/RenyCraftServer/crates/runtime/server-runtime/src/runtime/session/play.rs#L6)、command の適用と event dispatch は [`crates/runtime/server-runtime/src/runtime/core_loop/events.rs`](/home/jgm64/デスクトップ/RenyCraftServer/crates/runtime/server-runtime/src/runtime/core_loop/events.rs#L7)、wire packet への encode は [`crates/runtime/server-runtime/src/runtime/session/outgoing.rs`](/home/jgm64/デスクトップ/RenyCraftServer/crates/runtime/server-runtime/src/runtime/session/outgoing.rs#L8) にあります。

ここでのポイントは次です。

- runtime は `CoreCommand` を core に渡す
- core から返るのは `Vec<TargetedEvent>`
- runtime は target ごとに session を選び、`CoreEvent` を protocol adapter に encode させる
- `LoginAccepted` と `Disconnect` だけは専用 encode を使い、それ以外の play 中 event は `encode_play_event()` に流す

## 補足: `GameplayEffect` は command 専用ではない

`GameplayEffect` は `handle_command()` だけでなく `handle_tick()` でも使われます。tick path では `CoreCommand` を経由せず、gameplay policy が直接 `GameplayEffect` を返し、それが同じ `apply_gameplay_effect()` に流れます。

このため、型の関係を一番短く言うなら次です。

- `CoreCommand` は「入力の 1 つ」
- `GameplayEffect` は「gameplay policy の一般的な出力」
- `CoreEvent` は「core から runtime へ渡す出力」

## まとめ

- `CoreCommand` は core への入力です。
- `GameplayEffect` は gameplay policy の中間出力で、mutation と direct event を持てます。
- `CoreEvent` は client-visible な出力で、runtime が dispatch し protocol plugin が encode します。
- `GameplayJoinEffect` は login 時だけの特別な初期化用 effect です。
- `InventoryClick` のように gameplay を通らない command もあるため、command ごとの所有境界を意識して読むのが大事です。
