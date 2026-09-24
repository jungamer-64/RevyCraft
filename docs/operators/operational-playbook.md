# 運用者向け playbook

- 対象読者: RevyCraft を日常運用し、起動確認、gRPC admin 有効化、reload 判断、障害切り分けを行う人
- この文書で扱う範囲: 起動前後のチェックリスト、gRPC admin surface の有効化手順、reload の判断フロー、よくある失敗の切り分け、release bundle 前チェック
- この文書で扱わないこと: `runtime/server.toml` の全 key 定義、reload の内部実装、plugin authoring
- 仕様の正本: [`configuration-and-reload.md`](configuration-and-reload.md)

この文書は「まず何を確認すればよいか」を素早く揃えるための playbook です。設定 key の意味や反映境界そのものは [`configuration-and-reload.md`](configuration-and-reload.md)、初回導入の流れは [`getting-started.md`](getting-started.md) を正本として参照してください。

## 起動前チェックリスト

- Rust stable toolchain と `cargo` が使える
- `runtime/server.toml` を使うか、`REVY_SERVER_CONFIG` で別 path を指すか決めてある
- `static.plugins.plugins_dir` の relative path が active config の親 directory から解決できる
- `live.plugins.allowlist` に必要な plugin id が入っている
- allowlist を変えたあとに `cargo run -p xtask -- package-plugins` を再実行している
- `live.profiles.auth`、`live.profiles.bedrock_auth`、`live.profiles.default_gameplay`、`live.profiles.gameplay_map` が allowlist と噛み合っている
- remote admin を使うなら `admin-grpc.toml` と token file の path を用意している

## 起動確認チェックリスト

- 標準出力に `server listening on ...` が出ている
- 直後に runtime status summary が出ている
- console admin surface を有効化しているなら `help` か `status` が実行できる
- gRPC admin surface を有効化しているなら `bind_addr` で listen している
- 想定外の permission denied が出ていない
- reload watch を有効にしている場合は、active config path が存在し続けることを確認している

起動手順そのものを見直したい場合は [`getting-started.md`](getting-started.md) を参照してください。

## gRPC admin surface を有効にする

1. `runtime/server.toml` の allowlist に `admin-grpc` が入っていることを確認し、無ければ追加する
2. `static.admin.principals.<id>` に remote principal を追加し、少なくとも `status` と必要な権限を与える
3. `live.admin.surfaces.<instance>` に `profile = "grpc-v1"` と `config = "admin-grpc.toml"` を追加する
4. `runtime/admin-grpc.toml.example` を参考に `runtime/admin-grpc.toml` を用意し、`bind_addr` と `principals.<id>.token_file` を設定する
5. token file の 1 行目に空でない bearer token を入れる
6. `cargo run -p xtask -- package-plugins` と `cargo run -p revy-server` を順に実行する

最小例:

`runtime/server.toml`

```toml
[static.admin.principals.ops]
permissions = [
  "status",
  "sessions",
  "reload-runtime",
  "shutdown",
]

[live.admin.surfaces.grpc]
profile = "grpc-v1"
config = "admin-grpc.toml"
```

`runtime/admin-grpc.toml`

```toml
bind_addr = "127.0.0.1:50051"
allow_non_loopback = false

[principals.ops]
token_file = "admin/ops.token"
```

確認ポイント:

- `live.plugins.allowlist` に `admin-grpc` が入っている
- `config = "admin-grpc.toml"` は `runtime/server.toml` 基準で解決される
- `token_file = "admin/ops.token"` は `runtime/admin-grpc.toml` 基準で解決される
- `upgrade runtime executable <path>` も remote principal から使うなら `"upgrade-runtime"` permission が必要
- `allow_non_loopback = false` のままなら loopback bind だけが許可される

仕様の正本は [`configuration-and-reload.md`](configuration-and-reload.md) の admin surface 節です。

## reload 実行時の判断フロー

1. shared library だけを入れ替えたいなら `reload runtime artifacts`
2. listener、MOTD、adapter、有効な generation を変えたいなら `reload runtime topology`
3. `level_name`、`game_mode`、`difficulty`、`view_distance`、`max_players` を live state を保ったまま反映したいなら `reload runtime core`
4. allowlist、profile selection、admin surface、buffer limits、failure policy を変えたいなら `reload runtime full`
5. `online_mode`、`world_dir`、`storage_profile`、`static.plugins.*` を変えたいなら reload ではなく process restart

reload mode の正確な反映境界は [`configuration-and-reload.md`](configuration-and-reload.md) を参照してください。

reload / executable upgrade の直後は response または `status.last_cutover` を確認します。

- outcome が `committed` で、epoch revision が進んでいる
- connection mix と session count が実際の population に一致する
- `freeze_us` と `resume_us` を command 全体の所要時間から分けて確認する
- 通常 reload の 200 ms、executable upgrade の 500 ms という単一試行 hard limit を超えていない

## よくある失敗と確認箇所

### `runtime/server.toml` が見つからない

- `cargo run -p revy-server` は `REVY_SERVER_CONFIG` または `runtime/server.toml` しか読みません。
- `cargo run -p xtask -- package-plugins` は `runtime/server.toml.example` に fallback できるので、package 成功と boot 成功は別です。
- boot で失敗したら active config の path そのものを最初に確認してください。

### allowlist と profile selection が噛み合わない

- `live.plugins.allowlist` に plugin id があっても、`live.profiles.*` が別 profile を指していれば期待した plugin は active になりません。
- JE online mode なら `online_mode = true`、`auth = "mojang-online-v1"`、`auth-mojang-online` の 3 点を揃えます。
- Bedrock XBL なら `bedrock_auth = "bedrock-xbl-v1"` と `auth-bedrock-xbl` を揃えます。

### `runtime/plugins/<plugin-id>/` に package が無い

- allowlist を変えたあとに `cargo run -p xtask -- package-plugins` を再実行したか確認してください。
- `package-plugins` は allowlist 外の managed plugin を package しません。
- `runtime/plugins/<plugin-id>/plugin.toml` と shared library の両方が揃っていることを見ます。

### gRPC surface の token/config path が合わない

- `live.admin.surfaces.<instance>.config` は `runtime/server.toml` 基準です。
- `principals.<id>.token_file` は `runtime/admin-grpc.toml` 基準です。
- token は trim 後に non-empty である必要があります。
- principal 間で同じ token は使えません。

### restart-required な変更を reload で反映しようとしている

- `online_mode`、`level_type`、`world_dir`、`storage_profile`、`static.plugins.*` は restart-required です。
- これらを変えた直後に `reload runtime full` をしても反映対象ではありません。
- 「reload で足りる変更か」を迷ったら、まず [`configuration-and-reload.md`](configuration-and-reload.md) の「設定セクションと restart 境界」と「`reload` mode 選択早見表」を確認します。

## release bundle 作成前チェック

- 既定では `runtime/server.toml.example` が bundle source になるので、本当にその config を配布したいか確認する
- 別 config を配布したい場合は `--config <path>` を明示する
- `--target <triple>` ごとに必要な Rust target component と linker を用意する
- bundle に world data や admin token などの secret が自動では入らない前提を理解しておく
- allowlist を更新したあとに package と bundle 生成の順番が崩れていないか確認する
- published release の cutover latency acceptance job が、1000 Java、1000 Bedrock、500 + 500 の reload / executable 全構成で通っている
- 生成後は `dist/releases/<target>/` か `--output-dir` の中身を見て、`server-bootstrap`、`runtime/server.toml`、必要な plugin package が揃っていることを確認する

bundle の内容と config source の仕様は [`getting-started.md`](getting-started.md) を参照してください。
