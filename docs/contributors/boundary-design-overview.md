# 境界設計の全体像

この文書は、RevyCraft で「この実装はどこに置くべきか」を判断するための contributors 向け入口です。current implementation を基準に、大きな境界だけを先にそろえます。target crate graph と migration guardrail の正本は [`adr-boundary-redesign.md`](adr-boundary-redesign.md) を参照してください。

## 一枚絵

```text
apps/revy-server
  -> revy-server-runtime
     -> revy-server-config
     -> revy-server-types
     -> mc-plugin-host
     -> revy-voxel-core
        -> revy-voxel-semantic
           -> revy-core

mc-plugin-api / mc-plugin-sdk-rust
  -> revy-voxel-semantic
  -> revy-server-types

mc-proto-common
  -> revy-voxel-semantic

mc-storage-common
  -> revy-voxel-semantic

plugins/*
  -> mc-plugin-api or mc-plugin-sdk-rust
  -> protocol/storage common crate
```

## 迷ったときの境界判断

### app と runtime

`apps/revy-server` に置くのは process-scope の boot、stdio / gRPC admin surface、upgrade 協調です。session や world state の owner は `crates/runtime/revy-server-runtime` に寄せます。理由は、runtime の状態遷移と commit point を `ServerSupervisor` 配下に一本化したいからです。

### runtime と plugin host

runtime は「どの plugin を今の runtime view で使うか」を決めて使います。packaged plugin の discovery、activation、reload、quarantine は `mc-plugin-host` に寄せます。理由は、plugin lifecycle を runtime 本体から分離し、reload と failure policy を一箇所で扱うためです。

### semantic と engine internal

plugin や protocol 共通層が共有してよい型は `revy-voxel-semantic` までです。`revy-voxel-core` と `revy-core` は engine internal として扱います。理由は、plugin 側が engine 実装詳細に引きずられないようにし、共有契約を安定させるためです。

### config と runtime translation

`revy-server-config` は schema、load / normalize / validate、reload plan に寄せます。plugin-host bootstrap や runtime selection への変換は runtime 側の責務です。理由は、config crate を「設定の正規化」に閉じ、host 実装への依存を広げないためです。

### build-time と run-time

実行時の正本は `target/` ではなく `runtime/plugins/<plugin-id>/plugin.toml` を起点にした packaged plugin です。理由は、runtime が「build 済みかどうか」ではなく「package 済みかどうか」を実行条件にするためです。

## plugin kind の見分け方

| kind | plugin が担当するもの | 迷ったら外へ出すもの |
| --- | --- | --- |
| protocol | wire codec、handshake/login/play packet、transport/version 固有 session state | world state や canonical event 生成 |
| gameplay | `GameplayCommand` の評価、transaction journal 生成 | live core への validate/apply |
| storage | `WorldSnapshot` の load / save | transport や protocol version 差分 |
| auth | Java / Bedrock の認証 | session routing や world state |
| admin-surface | console / gRPC など operator surface | runtime の本体状態管理 |

## reloadable boundary の見方

公開 reload surface は `reload runtime artifacts / topology / core / full` の 4 つです。変更がどの境界に触れるかを見ると、reload 影響範囲を判断しやすくなります。

| mode | 切り替えるもの | 主に見る変更 |
| --- | --- | --- |
| `artifacts` | plugin generation | plugin artifact 差し替え |
| `topology` | listener / routing generation | bind 先や network topology 変更 |
| `core` | live core owner | world-semantic state や core migration |
| `full` | selection / topology / core | 上の境界をまたぐ変更 |

## 読む順番

1. [`repository-overview.md`](repository-overview.md)
2. [`runtime-and-plugin-architecture.md`](runtime-and-plugin-architecture.md)
3. [`reload-semantics-and-boundaries.md`](reload-semantics-and-boundaries.md)
4. [`adr-boundary-redesign.md`](adr-boundary-redesign.md)
