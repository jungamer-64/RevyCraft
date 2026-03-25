# runtime と plugin architecture

## 概要

この文書は、RevyCraft の runtime と plugin system がどのように噛み合っているかを contributors 向けに整理したものです。discovery、activation、session lifecycle、reload、安全境界をコードベースの責務に沿ってまとめます。

## 対象読者

- `server-runtime` や `mc-plugin-host` の挙動を追う contributors
- plugin system の lifecycle と reload 条件を正確に把握したい人

## この文書でわかること

- packaged plugin がどのように発見・選択されるか
- Java / Bedrock session が runtime でどう扱われるか
- generation reload と plugin reload がどの安全条件で成立するか
- quarantine と degraded behavior がどこで効くか

## 関連資料

- [`repository-overview.md`](repository-overview.md)
- [`reload-semantics-and-boundaries.md`](reload-semantics-and-boundaries.md)
- [`core-command-event-flow.md`](core-command-event-flow.md)
- [`../operators/configuration-and-reload.md`](../operators/configuration-and-reload.md)
- [`../plugin-authors/plugin-model.md`](../plugin-authors/plugin-model.md)

## 先に押さえる結論

- runtime は plugin 実装そのものを直接抱えず、`LoadedPluginSet` と `ProtocolRegistry` を消費する側に徹しています。
- protocol plugin の catalog と、config によって active になる gameplay / storage / auth / admin-ui profile は分離されています。
- reload は kind ごとに安全条件が違います。protocol / gameplay / storage は移行成功が swap 条件で、auth は新規 request から次世代に切り替わります。
- quarantine は candidate artifact と active plugin の両方に働きますが、挙動は kind と failure policy に依存します。

## レイヤー構成

1. `apps/server`
   `server-bootstrap` binary。config 読み込み、plugin host 構築、runtime 起動、stdio / gRPC admin transport の起動を担います。
2. `crates/runtime/server-runtime`
   transport、listener bind、session loop、generation 管理、status snapshot、admin control plane を持つ orchestration 層です。
3. `crates/core/mc-core`
   protocol 非依存の state machine です。world/player state、command 適用、event 生成を担います。
4. `crates/plugin/mc-plugin-api` と `crates/plugin/mc-plugin-sdk-rust`
   ABI 契約と Rust plugin authoring helper です。
5. `plugins/*`
   protocol / gameplay / storage / auth / admin-ui の concrete plugin 実装です。

## runtime の内部構造

`server-runtime` の中心は `RuntimeServer` ですが、実際の state owner は次の manager に分かれています。

- `SelectionManager`
  active config、loaded plugin snapshot、auth/admin selection、remote principal snapshot を持ちます。
- `TopologyManager`
  active / draining generation、listener worker、generation swap と drain を持ちます。
- `RuntimeKernel`
  `ServerCore`、dirty flag、tick / save、command 適用を持ちます。
- `SessionRegistry`
  live session handle、session task、accepted queue accounting、connection id 採番を持ちます。
- `ReloadCoordinator`
  config source、reload host、consistency gate、shutdown request を持ちます。

コードを読むときは `RuntimeServer` façade -> `selection.rs` -> `topology_manager.rs` -> `kernel.rs` -> `session/*` / `admin.rs` の順が、現在の責務分離に最も沿っています。

## package と discovery

### package の作り方

`tools/xtask` は plugin crate を build したあと、`runtime/plugins/<plugin-id>/` に shared library と `plugin.toml` を配置します。`package-plugins` は `runtime/server.toml` が存在すればその `[live.plugins].allowlist` を使い、存在しない場合だけ `runtime/server.toml.example` に fallback します。

manifest の最小形は次のとおりです。

```toml
[plugin]
id = "je-5"
kind = "protocol"

[artifacts]
"linux-x86_64" = "libmc_plugin_proto_je_5.so"
```

artifact key は `os-arch` 形式です。cross-platform な key space を持ちながら、実際に使われるのは現在の host 環境に一致する artifact だけです。

### discovery の流れ

`plugin_host_from_config(...)` は `static.plugins.plugins_dir` を走査し、`plugin.toml` を持つ directory を package として扱います。discovery 時点で見るのは次です。

- `plugin.id`
- `plugin.kind`
- 現在の `os-arch` に一致する artifact があるか

allowlist は discovery ではなく runtime selection で適用されます。catalog に存在しても、allowlist や profile selection に入らなければ active runtime view には入りません。

## activation と selection

### protocol plugin

protocol plugin は host 内部で catalog 上の protocol package を読み、immutable な `ProtocolRegistry` snapshot に固められます。runtime は protocol plugin を `ProtocolAdapter` trait object と Bedrock listener metadata として見ます。

route の可否は handshake probe と descriptor に依存します。catalog に載っていることと、現在の topology で active であることは同義ではありません。

### gameplay / storage / auth / admin-ui plugin

これらは config 依存で選択的に有効化されます。

- gameplay
  `live.profiles.default_gameplay` と `live.profiles.gameplay_map` で使われる profile だけを有効化します。
- storage
  `static.bootstrap.storage_profile` で指定された 1 つだけを有効化します。
- auth
  `live.profiles.auth` と、`live.topology.be_enabled = true` のときの `live.profiles.bedrock_auth` を有効化します。
- admin-ui
  `live.admin.ui_profile` で指定された profile だけを有効化します。

## manifest capability と descriptor の整合

runtime は manifest に書かれた capability を装飾ではなく契約として扱います。代表例は次です。

- `gameplay.profile:canonical`
- `storage.profile:je-anvil-1_7_10`
- `auth.profile:offline-v1`
- `auth.mode:offline`
- `admin-ui.profile:console-v1`
- `runtime.reload.protocol`
- `runtime.reload.gameplay`
- `runtime.reload.storage`
- `runtime.reload.auth`
- `runtime.reload.admin-ui`

host は manifest capability と plugin が `Describe` 系 API で返す descriptor を照合します。たとえば gameplay plugin では、manifest の `gameplay.profile:canonical` と `GameplayDescriptor { profile: "canonical" }` が一致しないと load できません。

補足として、manifest capability と runtime capability set は同じ文字列ではありません。たとえば canonical gameplay plugin は manifest では `gameplay.profile:canonical` を宣言し、runtime capability set では `gameplay.profile.canonical` を返します。前者は selection / validation 用、後者は runtime 中の feature advertisement 用と考えると読みやすいです。

## session lifecycle

### Java / TCP の入口

TCP accept は runtime loop から session task へ渡され、初期 phase は `Handshaking` です。この時点では adapter は未確定です。受信 frame は protocol registry の handshake probe に渡され、次のいずれかになります。

- probe に一致しない
  応答せず close します。
- 一致し、active adapter がある
  その adapter と対応 gameplay profile を session に結びつけて `Status` または `Login` に進みます。
- 一致するが active adapter がない
  `Status` では default adapter で応答し、`Login` では default adapter の codec で unsupported protocol disconnect を返します。

### Status phase

status packet の decode / encode は protocol plugin が担い、runtime は `ServerListStatus` を構成します。MOTD と online player 数は runtime 側の責務です。

### Java login phase

offline mode では `auth_profile.authenticate_offline()` で `PlayerId` を得て、そのまま `CoreCommand::LoginStart` を `mc-core` に渡します。

online mode では runtime が RSA-1024 鍵ペアを持ち、login start 後に encryption request を返します。verify token は runtime が生成し、transport encryption は AES-128-CFB8 を使います。challenge 発行時には auth generation を capture しており、途中で auth plugin が reload されても、その login は発行時の generation で完結します。

### Bedrock / UDP の入口

`live.topology.be_enabled = true` のときだけ UDP listener が bind されます。接続受理後は Bedrock baseline adapter と対応 gameplay profile が session に入ります。

login には次の 2 段階があります。

1. `BedrockNetworkSettingsRequest`
   protocol number から adapter を確定し、network settings response を返します。
2. `BedrockLogin`
   `live.profiles.bedrock_auth` に応じて offline または XBL 認証を行い、`CoreCommand::LoginStart` に変換します。

network settings 応答後は Bedrock compression が有効になります。

### Play phase

play phase では、protocol plugin の `decode_play()` が client packet を `CoreCommand` に変換し、`mc-core` がそれを処理します。逆方向では `CoreEvent` を protocol plugin の `encode_play_event()` が wire packet 群へ変換します。

runtime は session ごとに `SessionCapabilitySet` と generation 情報を保持し、どの protocol / gameplay generation がその接続に見えているかを追跡します。

`CoreCommand`、`GameplayEffect`、`CoreEvent` の役割分担と変換経路を型ベースで追いたいときは [`core-command-event-flow.md`](core-command-event-flow.md) を参照してください。

## runtime loop と snapshot

runtime loop の主な責務は次です。

- TCP accept
- Bedrock accept
- 50ms tick
- 約 2 秒ごとの save
- `live.plugins.reload_watch` または `live.topology.reload_watch` が有効なときの config watch reload

外から観測したいときは `ServerSupervisor` を使います。

- `status().await`
  active / draining generation、listener binding、session summary、plugin host status を返します。
- `session_status().await`
  connection ごとの詳細 session 情報を返します。
- `admin_control_plane()`
  request 単位の permission check を行う typed handle を返します。

内部では `ServerSupervisor` が `RunningServer` を包み、その先に `RuntimeServer` がいます。runtime 実装を読むとき以外は、lower-level 型を直接の API と見なさないほうが安全です。

## admin control plane

operator surface は `server-runtime` の admin control plane に集約されています。

- local principal は `local-console`
- remote principal は `static.admin.grpc.principals.<id>`
- stdio の parse / render は active admin-ui plugin が担当
- gRPC transport は plugin を介さず、typed `AdminControlPlaneHandle` を直接叩く

`reload_config()` で admin-ui profile や remote principal map が差し替わっても、進行中 request は開始時点の snapshot で完了し、次の request から新設定が使われます。built-in gRPC transport は plaintext h2 のみで、TLS は reverse proxy / ingress 側で終端する前提です。

## reload モデル

### generation id

plugin host は plugin generation、runtime は topology generation を持ちます。session status にも generation が出るため、reload 後にどの接続が旧世代へ pin されているかを観測できます。

### `reload_generation()`

`ServerSupervisor::reload_generation()` は最新 config の network / topology 部分だけを current selection に重ねて、新しい topology generation を作ります。plugin selection の変更は見ません。

切替の流れは次のとおりです。

1. config source を再読込する
2. `network` と `topology` だけを candidate config に反映する
3. candidate listener / routing / default adapter を validate する
4. 必要な listener を bind する
5. active generation を publish し、旧 generation を draining へ落とす

`drain_grace_secs` の間は旧 generation の session を継続させ、期限後に best-effort disconnect を試みます。

### protocol reload

protocol reload は route topology の互換性と session migration 成功が swap 条件です。候補 generation を load したあと、次を満たしたときだけ generation を切り替えます。

1. route topology を変えない
2. active `status / login / play` session を旧 generation から export できる
3. export した blob を candidate generation へ import できる

route 互換性の固定条件は次です。

- `adapter_id`
- `transport`
- `edition`
- `protocol_number`
- `wire_format`
- `bedrock_listener_descriptor`

protocol plugin が reload 対象になるには manifest に `runtime.reload.protocol` capability が必要です。candidate failure や import failure は quarantine ではなく skip 扱いで、現 generation を維持したまま `loaded_at` だけが進みます。

### gameplay reload

gameplay reload は対象 profile の active session を集め、旧 generation から各 session blob を export し、candidate generation へ import します。全件成功したときだけ generation を swap します。session migration が成立しない gameplay plugin は live swap されません。

### storage reload

storage reload は session ではなく `WorldSnapshot` を軸に差し替えます。runtime が持つ snapshot を candidate backend に `import_runtime_state(...)` できたときだけ storage profile を入れ替えます。

### auth reload

auth reload は主に profile generation の切替です。既存 play session は影響を受けず、新規 login から新 generation が使われます。Java online auth の challenge 発行済み session は旧 generation で完了します。

## quarantine モデル

failure policy は kind ごとに `protocol`, `gameplay`, `storage`, `auth`, `admin-ui` に分かれます。sample config の既定値は `protocol=quarantine`, `gameplay=quarantine`, `storage=fail-fast`, `auth=skip`, `admin-ui=skip` です。

quarantine には 2 つの面があります。

- active quarantine
  runtime invocation failure を起こした active plugin に対して適用します。
- artifact quarantine
  boot / reload candidate の load や migration に失敗した artifact に対して適用します。

代表的な degraded behavior は次のとおりです。

- protocol active quarantine
  quarantined error を返しやすくなり、descriptor 側も退避値になります。
- gameplay active quarantine
  hook を no-op として扱います。
- storage / auth
  `quarantine` ではなく `skip | fail-fast` を使います。

`fail-fast` は panic ではなく graceful stop です。listener accept を止め、best-effort save / shutdown を試みたあと runtime loop が fatal error を返します。

## gameplay host API

gameplay plugin だけは host callback を持ちます。plugin 側は `GameplayHost` trait を通して次を参照できます。

- world meta
- player snapshot
- block state
- can_edit_block
- log

ABI `3.4` でも gameplay invoke ごとに `HostApiTableV1` が plugin へ明示的に渡される前提は維持します。そのうえで protocol / core codec は `UseBlock`、semantic container/window event、`ContainerPropertyChanged`、`WorldSnapshot::block_entities`、`BlockEntityState` を扱え、JE-first の generic container 基盤として `window_id + container kind + contents + property diff + world-backed chest binding` を流せます。現時点の non-player container は session-local な `CraftingTable`、`Chest`、`Furnace` と、gameplay から開く world-backed single chest です。world-backed chest は persistence、same-chest multi-view sync、non-empty break reject を持ちます。world-backed furnace、double chest、operator / gameplay trigger、Bedrock container/property handling はまだこの層に入りません。

## concrete plugin の見方

plugin crate は概ね「実装本体 + manifest 宣言 + export macro」の 3 点セットです。

- protocol plugin
  concrete adapter を包み、`declare_protocol_plugin!` か `delegate_protocol_adapter!` で export します。
- gameplay plugin
  `RustGameplayPlugin` または `PolicyGameplayPlugin` を実装し、`export_plugin!(gameplay, ...)` で公開します。
- storage / auth / admin-ui plugin
  kind ごとの trait を実装し、`StaticPluginManifest` と `export_plugin!` を組み合わせます。

## テスト戦略

この repo のテストは architecture の意図をかなり直接に検証しています。

- `in-process-testing` feature を有効にした in-process plugin を差し込む test
- `runtime/plugins` 形式に package した shared library を実際に load する test
- Linux 上での dynamic reload test
- unknown profile、unsupported protocol、probe mismatch などの guardrail test

特に reload test は `REVY_PLUGIN_BUILD_TAG` を使って build 差し替えを検出し、generation が本当に切り替わったかを capability でも観測できるようにしています。

## 重要な設計上の読み取り

1. runtime の正本は binary 単体ではなく packaged runtime です。
2. plugin catalog と active runtime view は分かれていて、config が selection を決めます。
3. reload は kind ごとに安全条件が違い、単純な一括再初期化ではありません。
4. Java online auth は challenge 発行時の generation を固定することで reload と競合しにくくしています。
5. gameplay host API は強力ですが同期前提なので、plugin 側の設計にも制約があります。
