# Rust SDK と manifest

この文書は、Rust で RevyCraft plugin を書くときの正本です。`mc-plugin-api` と `mc-plugin-sdk-rust` の役割分担、trait / macro、`StaticPluginManifest`、plugin ABI `5.0` を現在の実装に合わせて整理します。

## crate の役割

| crate | 役割 | 使いどころ |
| --- | --- | --- |
| `mc-plugin-api` | ABI `5.0`、manifest struct、host API、typed codec | host / runtime 実装、ABI 契約確認 |
| `mc-plugin-sdk-rust` | Rust 向け trait、manifest helper、capability helper、export macro | 通常の Rust plugin authoring の正規入口 |

通常の plugin 作者は `mc-plugin-sdk-rust` を使い、ABI の細部が必要なときだけ `mc-plugin-api` を読みます。

## kind ごとの正規入口

| kind | trait / helper | export |
| --- | --- | --- |
| protocol | `RustProtocolPlugin`、`declare_protocol_plugin!`、`delegate_protocol_adapter!` | `declare_protocol_plugin!` または `export_plugin!(protocol, ...)` |
| gameplay | `RustGameplayPlugin` | `export_plugin!(gameplay, ...)` |
| storage | `RustStoragePlugin` | `export_plugin!(storage, ...)` |
| auth | `RustAuthPlugin` | `export_plugin!(auth, ...)` |
| admin-surface | `RustAdminSurfacePlugin` | `export_plugin!(admin_surface, ...)` |

共通でよく使う module は次です。

- `mc_plugin_sdk_rust::manifest`
- `mc_plugin_sdk_rust::capabilities`
- `mc_plugin_sdk_rust::{protocol, gameplay, storage, auth, admin_surface}`

## `StaticPluginManifest` が埋めるもの

`StaticPluginManifest` は embedded manifest 用の helper です。constructor を使うと plugin kind、ABI、host ABI range、required manifest capability が自動で入ります。

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

ABI はすべて `CURRENT_PLUGIN_ABI`、すなわち `5.0` に揃います。通常の Rust plugin ではこれを手で上書きする必要はありません。

## runtime capability set は別物

embedded manifest と runtime capability set は別です。

- embedded manifest
  host が load 時に検証する capability 文字列
- runtime capability set
  plugin が `capability_set()` で返す enum-based capability set

特に protocol plugin ではこの差が重要です。`StaticPluginManifest::protocol(...)` は embedded manifest に `runtime.reload.protocol` だけを書きますが、runtime 側の `ProtocolCapability::Je` や `ProtocolCapability::Je47` のような情報は `capability_set()` 側で表現します。

## protocol plugin の最小パターン

```rust
use revy_voxel_core::ProtocolCapability;
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

この macro は次をまとめて行います。

- adapter への委譲実装
- embedded manifest の export
- protocol API v3 の export

現在の protocol export surface は `ProtocolPluginApiV3`、exported symbol は `mc_plugin_protocol_api_v3` です。通常は macro に任せれば十分です。

## non-protocol plugin の最小パターン

```rust
use revy_voxel_core::{GameplayCapability, GameplayCapabilitySet};
use mc_plugin_api::codec::gameplay::GameplayDescriptor;
use mc_plugin_sdk_rust::capabilities::gameplay_capabilities;
use mc_plugin_sdk_rust::export_plugin;
use mc_plugin_sdk_rust::gameplay::RustGameplayPlugin;
use mc_plugin_sdk_rust::manifest::StaticPluginManifest;

#[derive(Default)]
pub struct CanonicalGameplayPlugin;

impl RustGameplayPlugin for CanonicalGameplayPlugin {
    fn descriptor(&self) -> GameplayDescriptor {
        GameplayDescriptor {
            profile: "canonical".into(),
        }
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

gameplay plugin は callback ごとに host から `GameplayHost` を受け取ります。plugin は `read_world_meta()` や `set_block()`、`set_inventory_slot()`、`emit_event()` などの domain-level API を呼び、`Ok(())` を返したときだけ host 側 transaction が commit されます。

現在の gameplay export surface は `GameplayPluginApiV3`、exported symbol は `mc_plugin_gameplay_api_v3` です。通常は `export_plugin!(gameplay, ...)` に任せれば十分です。

## admin-surface plugin の責務

`RustAdminSurfacePlugin` は host の admin kernel に対する薄い front-end です。plugin が持つのは次です。

- surface profile と instance declaration
- identity mapping と surface-owned config
- `host.execute(...)` / `host.permissions(...)` の利用
- `take_process_resource` / `publish_handoff_resource` / `take_handoff_resource` を使った stdio や upgrade resource の管理

つまり、console / gRPC / REST / WebUI のような operator entrypoint は plugin が決め、権限判定そのものは host 側の `static.admin.principals` が持ちます。

## descriptor と manifest の整合

host は descriptor と embedded manifest を照合します。現在の整合条件は次です。

- gameplay
  `GameplayDescriptor.profile` と manifest の `gameplay.profile:<id>`
- storage
  `StorageDescriptor.storage_profile` と manifest の `storage.profile:<id>`
- auth
  `AuthDescriptor.auth_profile` と manifest の `auth.profile:<id>`
- admin-surface
  `AdminSurfaceDescriptor.surface_profile` と manifest の `admin-surface.profile:<id>`

auth mode は descriptor 側で表現され、manifest 側では検証しません。runtime は selection 解決時に `online_mode` と auth descriptor mode の整合を確認します。

## `capability_set()` の注意

default では空集合になりますが、protocol だけは `RustProtocolPlugin` 自体ではなく supertrait の `ProtocolAdapter` 側に `capability_set()` があり、その他 kind は各 trait 側に `capability_set()` があります。実際には host が `RuntimeReload` capability を要求するため、ほとんどの plugin は明示的に上書きします。

例:

- protocol
  `ProtocolCapability::RuntimeReload`
- gameplay
  `GameplayCapability::RuntimeReload`
- storage
  `StorageCapability::RuntimeReload`
- auth
  `AuthCapability::RuntimeReload`
- admin-surface
  `AdminSurfaceCapability::RuntimeReload`

reload 以外の runtime capability は、必要なものだけ enum で追加します。

## `plugin.toml` との関係

`StaticPluginManifest` が生成するのは shared library 内の embedded manifest です。`runtime/plugins/<plugin-id>/plugin.toml` は別物で、packaging 側が discovery 用に使います。

workspace 内 plugin では通常、`xtask` が package 時に `plugin.toml` を生成 / 配置します。external packaged plugin を配布する場合は、shared library に加えて `plugin.toml` も必要です。

## 避けるべき内部 path

- `mc_plugin_sdk_rust::__macro_support`
- `mc_plugin_host::__test_hooks`
- `mc_proto_je_common::__version_support`
- `mc_proto_be_common::__version_support`

これらは公開 authoring surface ではありません。

## packaging まで含めた確認項目

1. descriptor の profile id が manifest constructor に渡した profile id と一致している
2. `capability_set()` が `RuntimeReload` を含んでいる
3. protocol plugin なら runtime capability に adapter / transport 系の capability を入れている
4. auth plugin なら descriptor mode が実装メソッドと一致している
5. `cargo run -p xtask -- package-plugins` 後に `runtime/plugins/<plugin-id>/` ができる
6. config の allowlist と profile selection がその plugin を参照している

workspace 全量を見たいときは `cargo run -p xtask -- package-all-plugins` も使えます。
