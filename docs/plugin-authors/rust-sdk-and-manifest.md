# Rust SDK と manifest

## 概要

この文書は、RevyCraft plugin を Rust で実装する人向けに、`mc-plugin-api` と `mc-plugin-sdk-rust` の役割分担、正規入口、manifest の書き方を整理したものです。

## 対象読者

- Rust で protocol / gameplay / storage / auth / admin-ui plugin を書く人
- どの module や macro を使うべきか知りたい人
- unsupported path を避けたい人

## この文書でわかること

- `mc-plugin-api` と `mc-plugin-sdk-rust` の違い
- kind ごとの trait と export macro
- `StaticPluginManifest` と descriptor の整合ルール
- plugin 作者が依存してよい surface と避けるべき surface

## 関連資料

- [`plugin-model.md`](plugin-model.md)
- [`../contributors/runtime-and-plugin-architecture.md`](../contributors/runtime-and-plugin-architecture.md)
- [`../../crates/plugin/mc-plugin-api/src/lib.rs`](../../crates/plugin/mc-plugin-api/src/lib.rs)
- [`../../crates/plugin/mc-plugin-sdk-rust/src/lib.rs`](../../crates/plugin/mc-plugin-sdk-rust/src/lib.rs)

## crate の役割分担

| crate | 役割 | 使いどころ |
| --- | --- | --- |
| `mc-plugin-api` | C ABI、manifest struct、host API table、typed codec | host / runtime 実装、ABI 契約の確認、macro が最終的に公開する symbol の理解 |
| `mc-plugin-sdk-rust` | Rust 向け trait、manifest helper、capability helper、export macro | 通常の Rust plugin authoring の正規入口 |

通常の Rust plugin 作者は、まず SDK を使い、ABI の詳細が必要なときだけ API crate を読むのが安全です。

## 正規入口

`mc-plugin-sdk-rust` で常用する module は次です。

- `protocol`
- `gameplay`
- `storage`
- `auth`
- `admin_ui`
- `manifest`
- `capabilities`

kind ごとの trait は次のとおりです。

- protocol
  `RustProtocolPlugin`
- gameplay
  `RustGameplayPlugin` または `PolicyGameplayPlugin`
- storage
  `RustStoragePlugin`
- auth
  `RustAuthPlugin`
- admin-ui
  `RustAdminUiPlugin`

## manifest の基本

Rust からは `StaticPluginManifest` を使うのが基本です。kind ごとの constructor が用意されています。

- `StaticPluginManifest::protocol_with_capabilities(...)`
- `StaticPluginManifest::gameplay(...)`
- `StaticPluginManifest::storage(...)`
- `StaticPluginManifest::auth(...)`
- `StaticPluginManifest::admin_ui(...)`

manifest が持つ主な情報は次です。

- `plugin_id`
- `display_name`
- `plugin_kind`
- `plugin_abi`
- `min_host_abi`
- `max_host_abi`
- `capabilities`

ABI は通常 `CURRENT_PLUGIN_ABI` に揃えます。現行 host ABI は `3.5` です。workspace plugin は rebuild / repackage 前提で扱ってください。

## kind ごとの export パターン

### protocol plugin

protocol plugin では `declare_protocol_plugin!` か `delegate_protocol_adapter!` を使うのが最短です。

```rust
use mc_plugin_sdk_rust::protocol::declare_protocol_plugin;
use mc_proto_je_47::Je47Adapter;

declare_protocol_plugin!(
    Je47ProtocolPlugin,
    Je47Adapter,
    "je-47",
    "JE 1.8.x (Protocol 47) Plugin",
    &["protocol.je", "protocol.je.47", "runtime.reload.protocol"],
    &["runtime.reload.protocol"],
);
```

この macro は adapter の委譲実装と manifest / exported symbol の公開までまとめて行います。

現行の protocol export surface は `ProtocolPluginApiV2` で、exported symbol は `mc_plugin_protocol_api_v2` です。通常は macro を使えば symbol 名を意識する必要はありません。

protocol plugin では次の責務境界を守ってください。

- `mc-core` へ渡す `CoreCommand::InventoryClick` は semantic target と validation だけにする
- raw slot や version ごとの差分は `decode_play(session, frame)` の時点で解決する
- `encode_play_event(event, session, context)` では session-owned state を見ながら packet を組み立てる
- active window や reject pending のような protocol-owned state がある場合は `session_closed()` と `export_session_state()` / `import_session_state()` で寿命管理する

JE の clicked-item echo / reject-ack gating、Bedrock の active window rewrite / authoritative inventory は protocol plugin 側の責務です。core や runtime に edition-specific な分岐を戻さないでください。

### gameplay / storage / auth / admin-ui plugin

これらは trait 実装と `export_plugin!` の組み合わせが基本です。

```rust
use mc_plugin_sdk_rust::export_plugin;
use mc_plugin_sdk_rust::manifest::StaticPluginManifest;

const MANIFEST: StaticPluginManifest = StaticPluginManifest::gameplay(
    "gameplay-canonical",
    "Canonical Gameplay Plugin",
    &["gameplay.profile:canonical", "runtime.reload.gameplay"],
);

export_plugin!(gameplay, CanonicalGameplayPlugin, MANIFEST);
```

`export_plugin!` は kind ごとに異なる ABI entrypoint を公開します。通常は macro に任せ、手で exported symbol を組み立てないほうが安全です。

## descriptor と manifest の整合チェック

kind ごとに、descriptor と manifest capability を一致させてください。

- gameplay
  `GameplayDescriptor.profile` と `gameplay.profile:<id>`
- storage
  `StorageDescriptor.storage_profile` と `storage.profile:<id>`
- auth
  `AuthDescriptor.auth_profile` / `AuthDescriptor.mode` と `auth.profile:<id>` / `auth.mode:<mode>`
- admin-ui
  `AdminUiDescriptor.ui_profile` と `admin-ui.profile:<id>`

reload を有効にしたい場合は、manifest に `runtime.reload.<kind>` を加え、対応する export / import surface を実装します。

特に protocol plugin は reload 時に session ごとの protocol state も移送されるため、reload 対応を宣言するなら `export_session_state()` / `import_session_state()` が no-op でよいかを明示的に判断してください。

## gameplay host API の扱い

gameplay plugin だけは host callback を受けられます。通常の Rust authoring では ABI の `HostApiTableV1` を直接触らず、SDK の `GameplayHost` trait を使ってください。

読める情報は次です。

- world meta
- player snapshot
- block state
- block entity
- can_edit_block
- log

host callback は同期 invoke の一部として扱われるため、重い処理や不必要な往復を前提にしない設計が向いています。

## unsupported path

次の surface は plugin 作者向けの公開入口として扱いません。

| path | 扱い |
| --- | --- |
| `mc_plugin_sdk_rust::__macro_support` | exported macro の内部実装です |
| `mc_plugin_host::__test_hooks` | workspace 内の shared test crate 用です |
| `mc_proto_je_common::__version_support` | protocol version crate 用の内部 path です |
| `mc_proto_be_common::__version_support` | protocol version crate 用の内部 path です |
| `mc_plugin_api::codec::gameplay::host_blob::*` | runtime / host integration helper として扱い、通常の authoring surface とは分けます |
| `mc_plugin_sdk_rust::test_support` | `in-process-testing` feature を有効にした test / dev build 用です |

これらに依存すると、workspace 内部の都合で壊れやすくなります。

## packaging まで含めた確認項目

実装後は次を確認してください。

1. trait 実装が kind と一致している
2. descriptor と manifest capability が一致している
3. `runtime.reload.*` を宣言したなら export / import surface がある
4. protocol plugin なら `decode_play()` の時点で raw slot を semantic slot へ変換している
5. gameplay plugin なら `GameplaySessionSnapshot.protocol` を見て必要な version 分岐を plugin 側で完結できる
6. shared library が package され、`runtime/plugins/<plugin-id>/plugin.toml` から見える
7. allowlist と profile selection が config から参照されている

workspace 内 plugin なら `cargo run -p xtask -- package-plugins`、全量 package を見たいときは `cargo run -p xtask -- package-all-plugins` を使うと確認しやすくなります。
