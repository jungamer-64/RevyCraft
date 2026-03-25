# RevyCraft

RevyCraft は、Minecraft Java Edition 系と Bedrock 系の protocol adapter を同一プロセス・同一ポートで扱える Rust 製 server-only workspace です。runtime は protocol / gameplay / storage / auth / admin-ui plugin を packaged artifact から読み込み、`mc-plugin-host` が immutable な `LoadedPluginSet` を構成し、`ServerSupervisor` が listener / generation / session supervision を起動します。

公開 docs は入口を読者別に分けています。概要だけを把握したいときはこの README、詳細に入るときは `docs/README.md` から辿ってください。

## Docs

- 総合ハブ: [`docs/README.md`](docs/README.md)
- 実装 contributors 向け: [`docs/contributors/repository-overview.md`](docs/contributors/repository-overview.md)
- 運用者向け: [`docs/operators/getting-started.md`](docs/operators/getting-started.md)
- plugin 作者向け: [`docs/plugin-authors/plugin-model.md`](docs/plugin-authors/plugin-model.md)

## 最短の起動手順

通常起動では、`cargo run -p xtask -- package-plugins` が `runtime/server.toml` を優先して読み、存在しない場合だけ `runtime/server.toml.example` に fallback します。`[live.plugins].allowlist` に含まれる plugin だけを package し、workspace 外で持ち込んだ third-party plugin directory は消しません。

```bash
cargo run -p xtask -- package-plugins
cargo run -p server-bootstrap
```

`server-bootstrap` は `runtime/server.toml` を読みます。設定ファイルが無い場合は default config で起動し、plugin は `runtime/plugins/<plugin-id>/plugin.toml` から解決します。runtime は `target/` の build artifact を直接読みません。

別の config path を使うときは `REVY_SERVER_CONFIG=/path/to/server.toml cargo run -p server-bootstrap` を使います。指定 path が見つからない場合は warning を出し、そのまま default config fallback で起動を試みます。

optional plugin を含めて workspace 管理下の plugin を全量 package したいときだけ、次を使います。

```bash
cargo run -p xtask -- package-all-plugins
```

配布用の release bundle を target ごとに作るときは次を使います。

```bash
cargo run -p xtask -- build-release-bundles \
  --target x86_64-unknown-linux-gnu \
  --target aarch64-apple-darwin
```

実行手順と bundle 構成の詳細は [`docs/operators/getting-started.md`](docs/operators/getting-started.md) を参照してください。

## 対応範囲の要約

- Java Edition / Bedrock の protocol adapter を plugin として切り替え可能
- handshake / status / login / play
- offline-mode / online-mode auth
- superflat overworld generation
- initial chunk send
- survival / creative block break / place
- authoritative inventory sync
- JE non-player container/window foundation
- multiple players
- block change sync
- `level.dat`, `playerdata/*.dat`, `region/*.mca` read/write
- same-process shared-port `TCP(JE)` + `UDP(BE)`
- dynamic plugin host / quarantine / generation reload
- read-only runtime / plugin introspection snapshots

具体的な対応 version / protocol number / adapter id は、active な plugin 構成と config に依存します。README では個別一覧を固定せず、実行時の構成を正として扱います。

Bedrock / JE の world interaction は survival v1 まで対応しています。creative は従来通り無限設置 / 即時破壊、survival は finite placement、single-block break、ephemeral な world drop、500ms pickup delay、5 分 despawn を持ちます。drop は persistence されず restart で消えます。durability、mining speed、tool requirement、hunger / exhaustion、loot table、fortune / silk touch は未実装です。JE では generic container/window 基盤が入り、player inventory の `window 0` に加えて session-local な non-player `CraftingTable` / `Chest` / `Furnace` window と、gameplay から開ける world-backed な single chest を扱えます。world-backed chest は persistence、same-chest multi-view sync、non-empty break reject を持ちます。furnace は `sand -> glass`、`cobblestone -> stone` と `oak_log` / `oak_planks` / `stick` の starter subset を扱います。world-backed furnace、double chest、operator trigger、Bedrock container/property handling はまだ未実装です。

## リポジトリの入口

- `apps/server`: `server-bootstrap` binary
- `crates/runtime/server-runtime`: protocol 非依存の runtime orchestration
- `crates/plugin`: `mc-plugin-api`, `mc-plugin-host`, `mc-plugin-sdk-rust`
- `plugins/*`: concrete plugin 実装
- `runtime/`: 実行時設定、packaged plugin 配置先、world データ

コードベースの読み順、運用設定、plugin authoring surface は audience 別 docs に分離しています。次の一歩に迷ったら [`docs/README.md`](docs/README.md) を起点にしてください。
