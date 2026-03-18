# RevyCraft

Minecraft JE 1.7.10 互換サーバーの初期実装と、既存の Bevy ローカルクライアントを同居させた Rust workspace です。

## Workspace

- `revycraft-client`: 既存のローカル Bevy ボクセルクライアント
- `mc-core`: バージョン非依存のワールド、プレイヤー、イベント、サーバーコア
- `mc-proto-common`: protocol adapter 境界と wire codec 共通部
- `mc-proto-je-1_7_10`: Minecraft JE 1.7.10 専用 codec と Anvil/NBT 変換
- `server-runtime`: protocol-agnostic な dedicated server runtime

## Run

サーバー起動:

```bash
cargo run -p server-runtime
```

Bevy クライアント起動:

```bash
cargo run -p revycraft-client
```

`server-runtime` はルートの `server.properties` を読みます。ファイルが無い場合はデフォルト設定で起動します。
現在のワールド生成は `level-type=FLAT` のみ対応です。

## Server Features

- Minecraft JE 1.7.10 / protocol 5 handshake, status ping, login, play
- offline-mode 認証
- superflat overworld 生成
- 初期 chunk bulk 送信
- 複数接続
- 他プレイヤー spawn / teleport / head rotation 同期
- keepalive
- `level.dat`, `playerdata/*.dat`, `region/*.mca` の read/write
- 将来の multi-version 対応を見据えた core / protocol 分離

## Tests

```bash
cargo test --workspace
```

## Client Controls

- `WASD`: move
- `Space`: jump
- `Mouse`: look
- `Left Click`: remove block
- `Right Click`: place selected block
- `1` / `2` / `3`: select `Grass` / `Dirt` / `Stone`
- `Esc`: release cursor
- `Left Click` after release: recapture cursor

## Notes

- `online-mode=true` は未実装です。現在は fail fast します。
- `level-type` は `FLAT` のみ対応です。その他の値は起動時に reject します。
- 初期対象外: block editing over network, inventory, chat, mobs, combat, Nether/End
- 既存配布ワールドの広範互換ではなく、サーバーが生成した 1.7.10 world の保存・再読込を優先しています。
