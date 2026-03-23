# plugin model

## 概要

この文書は、RevyCraft に plugin を追加する人向けの入口です。plugin kind、packaged layout、manifest capability、selection の考え方を先に整理します。

## 対象読者

- workspace 内に新しい plugin crate を追加する人
- workspace 外から packaged plugin を持ち込みたい人
- plugin host が何を見て plugin を認識するか知りたい人

## この文書でわかること

- RevyCraft の plugin kind と役割
- package / discovery / activation の流れ
- manifest capability と descriptor をどう合わせるべきか
- plugin catalog と active runtime view がなぜ別なのか

## 関連資料

- [`rust-sdk-and-manifest.md`](rust-sdk-and-manifest.md)
- [`../contributors/runtime-and-plugin-architecture.md`](../contributors/runtime-and-plugin-architecture.md)
- [`../../runtime/server.toml.example`](../../runtime/server.toml.example)

## 先に押さえる結論

- runtime が読むのは `target/` ではなく packaged plugin です。
- discovery された plugin がそのまま active になるわけではありません。allowlist と profile selection が active runtime view を決めます。
- manifest capability、descriptor、config 上の profile id は整合している必要があります。
- live reload の対象にしたい plugin では、kind ごとの `runtime.reload.*` capability と state export / import の実装が重要です。

## plugin kind

| kind | 主な責務 | config との結びつき |
| --- | --- | --- |
| `protocol` | handshake routing、status / login / play packet の decode / encode | `live.topology` の adapter selection と結びつきます |
| `gameplay` | `CoreCommand` を評価し、`GameplayEffect` / `CoreEvent` を生成 | `live.profiles.default_gameplay` と `live.profiles.gameplay_map` の profile id で選ばれます |
| `storage` | world snapshot の load / save / import / export | `static.bootstrap.storage_profile` の profile id で選ばれます |
| `auth` | JE offline / online、Bedrock offline / XBL 認証 | `live.profiles.auth` と `live.profiles.bedrock_auth` で選ばれます |
| `admin-ui` | stdio operator line の parse / render | `live.admin.ui_profile` で選ばれます |

## packaged layout

runtime が期待する配置は次のとおりです。

```text
runtime/
  plugins/
    <plugin-id>/
      plugin.toml
      <shared-library>
```

`plugin.toml` には少なくとも plugin id、kind、artifact map が必要です。

```toml
[plugin]
id = "gameplay-canonical"
kind = "gameplay"

[artifacts]
"linux-x86_64" = "libmc_plugin_gameplay_canonical.so"
```

artifact key は `os-arch` 形式です。現在の host と一致する artifact がなければ discovery 対象にはなっても active にはなりません。

## discovery と activation の違い

plugin host はまず packaged plugin catalog を作り、そのあと runtime selection を解決します。ここで次の 2 段階を分けて考えると混乱しにくくなります。

- discovery
  `plugin.toml` と artifact の存在から catalog に載せる段階です。
- activation
  allowlist、profile selection、platform compatibility を見て active runtime view に入れる段階です。

たとえば `runtime/plugins/` に plugin が置かれていても、`live.plugins.allowlist` に含まれないか、対象 profile が config から参照されなければ active にはなりません。

## manifest capability と descriptor

plugin host は manifest capability と descriptor を照合します。ここがずれると plugin は load できません。

代表的な組み合わせは次のとおりです。

- gameplay
  manifest `gameplay.profile:canonical`
  descriptor `GameplayDescriptor { profile: "canonical" }`
- storage
  manifest `storage.profile:je-anvil-1_7_10`
  descriptor `StorageDescriptor { storage_profile: "je-anvil-1_7_10" }`
- auth
  manifest `auth.profile:offline-v1`, `auth.mode:offline`
  descriptor `AuthDescriptor { auth_profile: "offline-v1", mode: Offline }`
- admin-ui
  manifest `admin-ui.profile:console-v1`
  descriptor `AdminUiDescriptor { ui_profile: "console-v1" }`

protocol plugin は profile id ではなく adapter / transport / listener metadata を返します。reload 可能にしたい場合は `runtime.reload.protocol` capability が追加で必要です。

## manifest capability と runtime capability set の違い

RevyCraft では、manifest capability と runtime capability set を別物として扱います。

- manifest capability
  load / selection / validation に使う宣言です。例: `gameplay.profile:canonical`
- runtime capability set
  runtime 中に plugin が持つ feature advertisement です。例: `gameplay.profile.canonical`

文字列形式が違っていても不思議ではありません。どちらを何のために使うのかを分けて実装してください。

## reload を意識した設計

kind ごとの reload 可能性は一律ではありません。

- protocol
  route topology を変えず、session state を export / import できる必要があります。
- gameplay
  active session state を export / import できる必要があります。
- storage
  runtime world state を import / export できる必要があります。
- auth
  新規 request から新 generation へ切り替わる前提です。
- admin-ui
  request 単位で次 generation に切り替わります。

reload 対象として扱わせたいときは、kind ごとの `runtime.reload.*` capability を manifest に入れ、必要な export / import surface を実装してください。

## sample plugin の見方

workspace 内の sample plugin は、kind ごとの正規パターンを示しています。

- `plugins/protocol/mc-plugin-proto-je-47`
  protocol adapter を macro で export する最小パターンです。
- `plugins/gameplay/mc-plugin-gameplay-canonical`
  `PolicyGameplayPlugin` と `export_plugin!(gameplay, ...)` の組み合わせです。
- `plugins/storage/mc-plugin-storage-je-anvil-1_7_10`
  `RustStoragePlugin` と manifest capability の対応例です。
- `plugins/auth/mc-plugin-auth-offline`
  auth profile と auth mode を manifest / descriptor の両方で宣言する例です。
- `plugins/admin-ui/mc-plugin-admin-ui-console`
  line-oriented console UI profile の実装例です。

## plugin 作者が次に読む文書

Rust での正規入口、macro、unsupported path、manifest 記述の実際は [`rust-sdk-and-manifest.md`](rust-sdk-and-manifest.md) を参照してください。
