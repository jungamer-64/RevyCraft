# plugin model

この文書は、RevyCraft に plugin を追加する人向けの正本です。plugin kind、packaged layout、discovery と activation、`plugin.toml` と embedded manifest の役割差を先に整理します。

## plugin kind

| kind | 主な責務 | config との結びつき |
| --- | --- | --- |
| `protocol` | handshake routing、status / login / play packet の decode / encode | `live.topology` の adapter selection |
| `gameplay` | semantic な `GameplayCommand` を評価し、host transaction API 経由で world / player を更新する | `live.profiles.default_gameplay` / `gameplay_map` |
| `storage` | world snapshot の load / save / import / export | `static.bootstrap.storage_profile` |
| `auth` | Java offline / online、Bedrock offline / XBL 認証 | `live.profiles.auth` / `bedrock_auth` |
| `admin-surface` | console / gRPC などの operator surface、identity mapping、surface-owned config と process resource の利用 | `live.admin.surfaces.<instance>` |

## packaged layout

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
  packaged layout の metadata
- shared library に埋め込まれた `PluginManifestV1`
  ABI、plugin kind、profile capability、reload capability を検証するための manifest

Rust plugin 作者が `StaticPluginManifest` で書くのは後者です。host は `plugin.toml` で package を見つけ、library を load したあとに embedded manifest と descriptor を検証します。

## discovery と activation

plugin host はまず packaged plugin catalog を作り、そのあと runtime selection を解決します。

- discovery
  `plugin.toml` と artifact の存在から catalog に載せる段階
- activation
  allowlist、profile selection、platform compatibility を見て active runtime view に入れる段階

そのため、`runtime/plugins/` に plugin が置かれていても、`live.plugins.allowlist` に入っていないか、対象 profile が config から参照されなければ active になりません。

## embedded manifest・descriptor・capability set

host は load 後に 3 つの情報を付き合わせます。

- embedded manifest
  shared library が宣言する ABI / kind / profile capability / reload capability
- descriptor
  plugin 自身が `Describe` 系 API で返す runtime 向けの識別情報
- runtime capability set
  plugin 自身が `CapabilitySet` で返す実行時 capability

現在の実装で重要なのは次です。

- gameplay / storage / auth / admin-surface
  embedded manifest の profile id と descriptor の profile id が一致している必要がある
- auth
  auth mode は descriptor 側にあり、embedded manifest 側には入らない
- protocol
  embedded manifest が持つのは `runtime.reload.protocol` だけで、adapter identity や routing 情報は descriptor / capability set 側で表現する
- 全 kind
  runtime capability set にも `RuntimeReload` capability が入っている必要がある

つまり、manifest は「何者か」を最小限に宣言し、descriptor と capability set が runtime 中の具体的な振る舞いを表します。

## selection と profile の見方

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

## sample plugin の見方

- `plugins/protocol/mc-plugin-proto-je-47`
  `declare_protocol_plugin!` を使った protocol plugin の最小パターン
- `plugins/gameplay/mc-plugin-gameplay-canonical`
  gameplay profile plugin の代表例
- `plugins/storage/mc-plugin-storage-je-anvil-1_7_10`
  storage profile plugin の代表例
- `plugins/auth/mc-plugin-auth-offline`
  Java offline auth plugin の例
- `plugins/admin/mc-plugin-admin-console`
  `console-v1` admin surface plugin の例

Rust からの実装方法、`StaticPluginManifest`、macro、ABI `5.0` の詳細は [`rust-sdk-and-manifest.md`](rust-sdk-and-manifest.md) を参照してください。
