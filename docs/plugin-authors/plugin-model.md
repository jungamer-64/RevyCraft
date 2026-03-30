# プラグインモデルと Rust 実装

- 対象読者: RevyCraft に plugin を追加したい人、Rust で plugin を実装したい人
- この文書で扱う範囲: plugin の種別、package 形式、`manifest` / `descriptor` / `capability`、Rust SDK、最小実装、packaging checklist
- この文書で扱わないこと: runtime 内部の state owner、reload coordinator の実装詳細、workspace 全体の boundary 設計
- 次に読む文書: [`../contributors/runtime-and-plugin-architecture.md`](../contributors/runtime-and-plugin-architecture.md)

## 種別ごとの役割

| kind | 主な責務 | config との結びつき |
| --- | --- | --- |
| `protocol` | handshake routing、status / login / play packet の decode / encode | `live.topology` の adapter selection |
| `gameplay` | semantic な `GameplayCommand` を評価し、host transaction API 経由で world / player を更新する | `live.profiles.default_gameplay` / `gameplay_map` |
| `storage` | world snapshot の load / save / import / export | `static.bootstrap.storage_profile` |
| `auth` | Java offline / online、Bedrock offline / XBL 認証 | `live.profiles.auth` / `bedrock_auth` |
| `admin-surface` | console / gRPC などの operator surface、identity mapping、surface-owned config と process resource の利用 | `live.admin.surfaces.<instance>` |

plugin の責務は runtime selection とセットで決まります。`runtime/plugins/` に package が置かれていても、allowlist と profile selection に入っていなければ active にはなりません。

## package 形式

runtime が期待する package 形式は次です。

```text
runtime/
  plugins/
    <plugin-id>/
      plugin.toml
      <shared-library>
```

`plugin.toml` は packaged directory を発見し、current host に合う artifact filename を引くための metadata です。少なくとも plugin id、kind、artifact map が必要です。

```toml
[plugin]
id = "gameplay-canonical"
kind = "gameplay"

[artifacts]
"linux-x86_64" = "libmc_plugin_gameplay_canonical.so"
```

artifact key は `os-arch` 形式です。host と一致する artifact が無ければ、package は見つかっても active にはなれません。

## `plugin.toml` と embedded manifest

plugin には 2 種類の manifest があります。

- `plugin.toml`
  package 形式の metadata です。host はこれを起点に package を発見します。
- shared library に埋め込まれた `PluginManifestV1`
  ABI、plugin 種別、profile capability、reload capability を検証するための manifest です。

Rust plugin 作者が `StaticPluginManifest` で書くのは後者です。host は `plugin.toml` で package を見つけ、library を load したあとに embedded manifest と descriptor を検証します。

## 発見と有効化

plugin host はまず packaged plugin catalog を作り、そのあと runtime selection を解決します。

- discovery
  `plugin.toml` と artifact の存在から catalog に載せる段階です。
- activation
  allowlist、profile selection、platform compatibility を見て active runtime view に入れる段階です。

そのため、`runtime/plugins/` に plugin が置かれていても、`live.plugins.allowlist` に入っていないか、対象 profile が config から参照されなければ active になりません。

## 埋め込み `manifest` と `descriptor` と `capability set`

host は load 後に 3 つの情報を付き合わせます。

- embedded manifest
  shared library が宣言する ABI / kind / profile capability / reload capability
- descriptor
  plugin 自身が `Describe` 系 API で返す runtime 向けの識別情報
- runtime capability set
  plugin 自身が `capability_set()` で返す実行時 capability

現在の実装で重要なのは次です。

- gameplay / storage / auth / admin-surface
  embedded manifest の profile id と descriptor の profile id が一致している必要があります。
- auth
  auth mode は descriptor 側にあり、embedded manifest 側には入りません。
- protocol
  embedded manifest が持つのは `runtime.reload.protocol` だけで、adapter identity や routing 情報は descriptor / capability set 側で表現します。
- 全 kind
  runtime capability set にも `RuntimeReload` capability が入っている必要があります。

manifest は「何者か」を最小限に宣言し、descriptor と capability set が runtime 中の具体的な振る舞いを表します。

## `runtime` がどの plugin を使うか

runtime がどの plugin を実際に使うかは config で決まります。

- protocol
  `default_adapter` / `enabled_adapters` / Bedrock 側の adapter 設定
- gameplay
  `default_gameplay` と `gameplay_map`
- storage
  `static.bootstrap.storage_profile`
- auth
  `auth` と `bedrock_auth`
- admin-surface
  `live.admin.surfaces.<instance>.profile`

profile id を新しく増やす plugin は、manifest / descriptor / config の 3 箇所で同じ id を使うことが前提です。

## Rust SDK の役割

| crate | 役割 | 使いどころ |
| --- | --- | --- |
| `mc-plugin-api` | ABI `6.0`、manifest struct、host API、typed codec | host / runtime 実装、ABI 契約確認 |
| `mc-plugin-sdk-rust` | Rust 向け trait、manifest helper、capability helper、export macro | 通常の Rust plugin authoring の正規入口 |

通常の plugin 作者は `mc-plugin-sdk-rust` を使い、ABI の細部が必要なときだけ `mc-plugin-api` を読みます。capability や id のような semantic type は `mc_plugin_sdk_rust` crate root から import し、`revy_voxel_core` は plugin authoring surface として使いません。

### kind ごとの正規入口

| kind | trait / helper | export |
| --- | --- | --- |
| `protocol` | `RustProtocolPlugin`、`declare_protocol_plugin!`、`delegate_protocol_adapter!` | `declare_protocol_plugin!` または `export_plugin!(protocol, ...)` |
| `gameplay` | `RustGameplayPlugin` | `export_plugin!(gameplay, ...)` |
| `storage` | `RustStoragePlugin` | `export_plugin!(storage, ...)` |
| `auth` | `RustAuthPlugin` | `export_plugin!(auth, ...)` |
| `admin-surface` | `RustAdminSurfacePlugin` | `export_plugin!(admin_surface, ...)` |

共通でよく使う module は次です。

- `mc_plugin_sdk_rust::manifest`
- `mc_plugin_sdk_rust::capabilities`
- `mc_plugin_sdk_rust::{protocol, gameplay, storage, auth, admin_surface}`

### `StaticPluginManifest` が埋めるもの

`StaticPluginManifest` は埋め込み manifest 用の helper です。constructor を使うと plugin 種別、ABI、host ABI range、required manifest capability が自動で入ります。

現在の constructor が生成する manifest capability は次です。

- `StaticPluginManifest::protocol(...)`
  `runtime.reload.protocol`
- `StaticPluginManifest::gameplay(..., profile_id)`
  `gameplay.profile:<profile_id>` と `runtime.reload.gameplay`
- `StaticPluginManifest::storage(..., profile_id)`
  `storage.profile:<profile_id>` と `runtime.reload.storage`
- `StaticPluginManifest::auth(..., profile_id)`
  `auth.profile:<profile_id>` と `runtime.reload.auth`
- `StaticPluginManifest::admin_surface(..., profile_id)`
  `admin-surface.profile:<profile_id>` と `runtime.reload.admin-surface`

ABI はすべて `CURRENT_PLUGIN_ABI`、すなわち `6.0` に揃います。通常の Rust plugin ではこれを手で上書きする必要はありません。

### runtime capability set は別物

embedded manifest と runtime capability set は別です。

- embedded manifest
  host が load 時に検証する capability 文字列
- runtime capability set
  plugin が `capability_set()` で返す enum-based capability set

特に protocol plugin ではこの差が重要です。`StaticPluginManifest::protocol(...)` は embedded manifest に `runtime.reload.protocol` だけを書きますが、runtime 側の adapter / transport 系 capability は `capability_set()` 側で表現します。

## 最小実装パターン

### protocol plugin

```rust
use mc_plugin_sdk_rust::ProtocolCapability;
use mc_plugin_sdk_rust::protocol::declare_protocol_plugin;
use mc_proto_je_47::Je47Adapter;

declare_protocol_plugin!(
    Je47ProtocolPlugin,
    Je47Adapter,
    "je-47",
    "JE 1.8.x (Protocol 47) Plugin",
    &[
        ProtocolCapability::RuntimeReload,
        ProtocolCapability::Je,
        ProtocolCapability::Je47,
    ],
);
```

この macro は adapter への委譲実装、embedded manifest の export、protocol API v3 の export をまとめて行います。

### gameplay plugin

```rust
use mc_plugin_api::codec::gameplay::GameplayDescriptor;
use mc_plugin_sdk_rust::{GameplayCapability, GameplayCapabilitySet};
use mc_plugin_sdk_rust::capabilities::gameplay_capabilities;
use mc_plugin_sdk_rust::export_plugin;
use mc_plugin_sdk_rust::gameplay::{RustGameplayPlugin, gameplay_descriptor};
use mc_plugin_sdk_rust::manifest::StaticPluginManifest;

#[derive(Default)]
pub struct CanonicalGameplayPlugin;

impl RustGameplayPlugin for CanonicalGameplayPlugin {
    fn descriptor(&self) -> GameplayDescriptor {
        gameplay_descriptor("canonical")
    }

    fn capability_set(&self) -> GameplayCapabilitySet {
        gameplay_capabilities(&[GameplayCapability::RuntimeReload])
    }
}

const MANIFEST: StaticPluginManifest = StaticPluginManifest::gameplay(
    "gameplay-canonical",
    "Canonical Gameplay Plugin",
    "canonical",
);

export_plugin!(gameplay, CanonicalGameplayPlugin, MANIFEST);
```

gameplay plugin は callback ごとに host から `GameplayHost` を受け取ります。plugin は domain-level API を呼び、`Ok(())` を返したときだけ host 側 transaction が commit されます。

### admin-surface plugin の責務

`RustAdminSurfacePlugin` は host の admin kernel に対する薄い front-end です。plugin が持つのは次です。

- surface profile と instance declaration
- identity mapping と surface-owned config
- `host.execute(...)` / `host.permissions(...)` の利用
- `take_process_resource` / `publish_handoff_resource` / `take_handoff_resource` を使った stdio や upgrade resource の管理

権限判定そのものは host 側の `static.admin.principals` が持ちます。

## 避けるべき内部 path

authoring code が semantic capability / id を参照するときは `mc_plugin_sdk_rust` crate root を使います。`revy_voxel_core` 直参照は engine internal 依存なので避けます。

リストだけ読まれても次の入口が分かるように、各項目の代替をここで固定します。

| 避ける path | 理由 | 代わりに使うもの |
| --- | --- | --- |
| `mc_plugin_sdk_rust::__macro_support` | macro 展開用の内部 module で、authoring surface ではない | `mc_plugin_sdk_rust::manifest`、`mc_plugin_sdk_rust::capabilities`、`mc_plugin_sdk_rust::{protocol, gameplay, storage, auth, admin_surface}` |
| `mc_plugin_host::__test_hooks` | host 内部の test support で、production authoring からは見せない前提 | 通常の authoring は `mc_plugin_api` / `mc_plugin_sdk_rust` の公開 API を使い、workspace 内 test では `crates/testing/mc-plugin-test-support` と `crates/testing/mc-plugin-host-test-support` を使う |
| `mc_proto_je_common::__version_support` | Java 版ごとの codec helper をまとめた内部 module | versioned Java protocol crate の公開 adapter / codec API を使う。例: `mc_proto_je_47::Je47Adapter` |
| `mc_proto_be_common::__version_support` | Bedrock 版ごとの codec helper をまとめた内部 module | versioned Bedrock protocol crate の公開 adapter / codec API を使う。例: `mc_proto_be_924` の公開 surface |

## packaging まで含めた確認項目

1. descriptor の profile id が manifest constructor に渡した profile id と一致している
2. `capability_set()` が `RuntimeReload` を含んでいる
3. protocol plugin なら runtime capability に adapter / transport 系の capability を入れている
4. auth plugin なら descriptor mode が実装メソッドと一致している
5. `cargo run -p xtask -- package-plugins` 後に `runtime/plugins/<plugin-id>/` ができる
6. config の allowlist と profile selection がその plugin を参照している

workspace 全量を見たいときは `cargo run -p xtask -- package-all-plugins` も使えます。

## sample plugin の見方

- `plugins/protocol/je-47/mc-plugin-proto-je-47`
  `declare_protocol_plugin!` を使った protocol bundle 内 wrapper の最小パターン
- `plugins/gameplay/mc-plugin-gameplay-canonical`
  gameplay profile plugin の代表例
- `plugins/storage/mc-plugin-storage-je-anvil-1_7_10`
  storage profile plugin の代表例
- `plugins/auth/mc-plugin-auth-offline`
  Java offline auth plugin の例
- `plugins/admin/mc-plugin-admin-console`
  `console-v1` admin surface plugin の例
