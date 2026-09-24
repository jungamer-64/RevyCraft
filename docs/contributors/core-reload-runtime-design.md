# `reload runtime` と executable cutover の設計

- 対象読者: reload、core revision、session cutover、executable handoff を変更する contributors
- この文書で扱う範囲: cutover protocol、freeze 測定契約、rollback、latency acceptance
- この文書で扱わないこと: operator command の設定手順、plugin authoring のコード例
- 関連文書: [`runtime-and-plugin-architecture.md`](runtime-and-plugin-architecture.md)、[`../operators/configuration-and-reload.md`](../operators/configuration-and-reload.md)

この設計の最上位条件は、1000 live session で reload と executable replacement の停止時間を budget 内に保つことです。plugin load や serialization を速くするだけではなく、freeze に入る前に fallible / allocative work を完了できる ownership と protocol を要求します。

## 公開操作と単一 cutover engine

公開 mode は次の 4 つです。

- `reload runtime artifacts`
- `reload runtime topology`
- `reload runtime core`
- `reload runtime full`

mode は変更してよい resource set だけを制限します。すべて `RuntimeChangeSet` と同じ cutover engine を使い、selection / topology / core の mode 別 commit API は持ちません。watch reload は `full` と同じ意味論です。

## typestate protocol

normal reload は次の一方向 transition です。

1. `StagedCutover`
   config validation、packaged plugin load/finalize、inactive topology、core candidate、session-independent material を構築します。
2. `PreparingCutover`
   session actor に candidate binding を bounded fan-out で配布し、handoff slot と resync buffer を確保します。listener candidate を precommit します。
3. `FrozenCutover`
   listener ingress と data-plane gate を閉じ、core final delta、session、transport、RakNet の最終 state だけを seal します。
4. `PreparedCutover`
   actor は active state と pending state を保持し、外部 packet はまだ送信しません。
5. commit または abort
   commit は candidate epoch を一度 publish し、ingress と gate を再開して epoch latch を通知します。abort は old epoch のまま ingress と gate を再開し、pending state を破棄します。

1000 actor への commit command は逐次送信しません。各 actor は pending state を持ち、共有 epoch latch の revision が一致した場合だけ次の data-plane 処理前に activate します。古い latch notification は pending revision と一致しないため無視されます。

old generation の drain は registry projection だけで切断を決定しません。期限切れ候補へ通知し、actor が data-plane admission と epoch activation を完了した後に現在の binding の期限を再確認します。猶予0でも新epochへ移った接続を古い通知で切断せず、移行対象でない旧世代の接続には期限を適用します。

## freeze に入れてよい処理

freeze 中に行うのは次だけです。

- core journal の final delta seal
- session actor の final phase / buffer / queued event seal
- TCP writer と RakNet router / peer の mutation 停止
- prebuilt resync frame を session queue 先頭へ登録
- epoch publication または explicit abort
- listener ingress と data-plane gate の再開

freeze 中に plugin discovery、dynamic library load、listener bind、child spawn、core 全体 serialization、socket enumeration、実 network writeを行いません。resync frame の encode は prepare / seal で buffer に完成させ、write は gate 再開後に行います。

## freeze の測定境界

`freeze_us` は monotonic clock で次を測ります。

- 開始: data-plane gate の排他取得を開始し、新しい packet、callback、core mutation、accept dispatch の admission を止める時点。進行中の処理の完了待ちも含め、write guard を取得した後へ開始時刻を遅らせません
- normal commit 終了: candidate epoch を publishし、listener ingress を再開し、全 actor が pending state を activate 可能な状態で data-plane gate を開いた直後
- executable commit 終了: child が imported listener / session を activateして data plane を開き、parent が authenticated `Committed` acknowledgement を受けた時点
- abort 終了: old epoch のまま listener ingress と data-plane gate を再開した直後

stage、plugin load、child boot、pre-copy は freeze に含めません。gate close 後の final snapshot/delta、commit IPC、rollback、ingress resume は含めます。`resume_us` は freeze のうち publication 後の ingress/gate 再開に費やした部分です。

gate を閉じてから listener ingress を停止します。listener の bounded accept queue と session admission は区別し、gate 待ちは実際に session を作る runtime 側だけで行います。listener の制御 loop は gate 待ちをせず、pause / resume / shutdown に応答し続けます。

## immutable core と conflict

gameplay read は `Arc<CoreVersion>` を取得し、read-set に source `CoreRevision` を保持します。mutation は sparse overlay から `PreparedCoreCommit { base_revision, next_version, events }` を作ります。base revision が stale の場合は machine-readable stale outcome を返し、plugin callback を再実行しません。

chunk payload は集合の index とは別に immutable ownership を持ちます。index の更新で他の chunk payload を複製せず、block mutation は変更する chunk だけを copy-on-write します。既存 chunk の読み取りは mutable access を要求しません。callback 用の core view は callback 終了時に解放し、確定済み read-set / effect batch だけを commit 待ちへ渡します。並行 session の待ち行列が不要な旧 world payload を保持し続けない lifetime とします。

revision は outgoing event の有無ではなく、適用された mutation log に結び付きます。keepalive ACK のような event を発行しない mutation も revision を進め、process delta に含めます。state が結果的に変わらない mutation attempt でも revision は進み得ます。mutation log が空の場合だけ base revision を保持します。

plugin の speculative read-set は親の commit 前に競合検証し、その後の転送 journal には確定済み effects と適用時刻だけを保持します。child は厳密に隣接した source revision へその effects を適用し、plugin callback や speculative read-set の再検証を行いません。login の entity allocation と admission の invariant は child の再構築でも検証します。

公開済み core の参照取得は mutation / journal の待ち行列に入りません。publication lock が保護するのは `Arc` の取得と交換だけで、mutation の準備、serialization、古い component の解放をその lock 内で行いません。commit は別の writer lock で直列化し、journal と persistence metadata を確定してから version を publish します。version と metadata を同時に観測する pre-copy / save は writer lock を先に取得し、同じ commit boundary に結び付いた組だけを読みます。

同一 process reload は `CoreHandoff` の `Arc<CoreVersion>` capability を candidate `CoreStore` に install します。process transfer は revision R の encoded pre-copy と、その後の bounded mutation journalを分けます。child 起動後も `Preparing` update と `Ready` acknowledgement を繰り返し、child の確認済み revision を進めます。prepare 中に journal が追い越した場合は gate を開いたまま snapshot を作り直します。update 回数と arena 容量は有限で、追いつけなければ freeze 前に失敗します。freeze 中は最後に acknowledge された revision から final revision までの短い delta だけを予約 slot に seal します。`Frozen` update で full snapshot を受理しません。gate 閉鎖直前の race で journal が追い越した場合も、active epoch を変えず abort します。

persistence completion は保存した revision までだけを `persisted_revision` に進めます。保存中に新しい commit があれば latest revision は dirty のままです。

journal の保持対象は cutover の用途で分離します。同一 process の candidate は resync event のみを有限の event 数・semantic encoding byte budget で保持し、event を出さない revision は保持枠を消費しません。freeze 時は完成済み event buffer の所有権を移し、process commit の serialization や commit 数 budget を適用しません。executable の journal は encoded semantic commit のみを保持し、entry・全体 byte 数・隣接 commit 数を制限します。entry の encoding limit は出力 buffer の拡張前に適用します。policy exhaustion は pre-copy の作り直し、codec・allocation failure は cutover failure として区別します。journal の失敗で正常な gameplay commit を取り消すことはありません。abort 時の保持 buffer の解放は gate 再開後へ遅延します。

## session、writer、resync

session phase と lifecycle は [`runtime-and-plugin-architecture.md`](runtime-and-plugin-architecture.md) の state machine を authority とします。prepare 中も active writer は動き続けるため、keepalive と通常 traffic を数秒止めません。writer pause は data-plane freeze 後に行います。

freeze ack、prepare ack、snapshot は bounded fan-out で集約します。central coordinator は actor slot の大きな payload を再コピーしません。commit 後の write failure はその session だけを切断し、既に publish 済みの global epoch を rollback しません。

fan-out は一件の failure で残りの acknowledgement を cancel しません。全 wave の処理完了を回収してから最初の failure を返し、rollback が未完了の prepare / freeze command と競合しない順序を保ちます。abort / rollback 自体も全 session への処理を完了してから成否を返します。

Bedrock writer は、同じ送信 command または resync queue に既に存在する packet stream を、packet 順序とサイズ境界を保って一つの compressed batch にまとめます。aggregation のために新しい packet の到着は待ちません。compression 設定変更や write completion の境界を跨がず、completion はその command の全 batch を送信した後にだけ通知します。大きい単一 packet を aggregation の都合で分割・拒否せず、既存の transport budget を適用します。

## executable upgrade

parent は freeze 前に child を起動し、次を完了します。

- config と common transfer protocol の validation
- exact packaged plugin artifact hash の照合
- child candidate epoch と shared transfer arena の構築
- versioned session directory の継続的 pre-stage / acknowledgement
- TCP stream、TCP/UDP listener、admin resource の duplication
- core snapshot と bounded final-delta slot の作成

freeze 中は directory revision を再確認し、session/RakNet/core final delta を sealし、child validation、`Commit`、`Committed` handshakeだけを行います。large payload は arena descriptor で参照し、length-limited protobuf control envelopeへ埋め込みません。

prepare 中の core delta は bounded temporary buffer で長さを確定してから、実際の長さだけを arena へ保持します。journal が追い越した試行の未使用スロットは保持しません。freeze 用の final delta は別途最大容量を予約し、freeze 中に buffer を確保し直しません。

session state の arena descriptor は connection identity を必須とします。child は descriptor と確認済み directory から import job を割り当て、各 worker が decode、actor identity の照合、plugin / transport import を一続きで完了します。payload 内の identity も同じ directory entry と照合してから plugin を呼ぶため、descriptor は未検証 state を正当化する authority にはなりません。

RakNet の duplicate window と再送・順序制御 state は child が final `Ready` を返す前に構築を完了します。検証済み checkpoint は resource policy に結び付け、validation で構築した duplicate index も receiving actor へ移譲します。Frozen の間は protocol timer の残り時間を消費せず、commit / abort のどちらでも再開時から計時を継続します。この timer 停止は monotonic wall-clock による `freeze_us` の測定区間を短縮しません。

child は `Commit` を受けた後、gameplay admission を閉じたまま共有 latch で全 imported session を activate し、完了を確認してから listener ingress と gate を開きます。最初の tick や queued gameplay が残りの session activation と競合する順序にはしません。

起動時の status 集計と表示は `Committed` 送信後に行います。再開した gameplay と競合する表示用の待ちを activation の成立条件へ含めません。親の freeze 計測は引き続き `Committed` の受信まで継続します。

`Commit` 送信前の failure は parent が rollbackして old epoch を再開できます。sealed transfer の明示的な abort も、gate を開く前に core journal の記録を止めます。放棄した child のための serialization を通常 gameplay へ残さず、保持済み buffer の解放は gate 再開後へ遅延します。送信後に outcome が不明なら parent は再開せず、同じ `TransferId` の status を照会します。解消不能時は fail closed です。`Err` を「child が commit していない」証拠にしません。

## `CutoverReport`

reload response、upgrade response、runtime status の `last_cutover` は同じ machine-readable report を返します。

- operation と reload mode
- Java / Bedrock connection mix と session count
- `stage_us`、`prepare_us`、`freeze_us`、`resume_us`
- committed / aborted outcome
- epoch revision

test/debug 専用 clock や pause API は使いません。performance test は production gRPC command とこの report だけを authority にします。

## latency budget

各 budget は Linux / Windows のそれぞれで、1000 Java、1000 Bedrock、Java 500 + Bedrock 500 の各構成へ適用します。

| operation | Target (p50) | Acceptance (p95) | CI Hard Limit (max) |
| --- | ---: | ---: | ---: |
| normal reload | 50 ms | 100 ms | 200 ms |
| executable upgrade | 100 ms | 250 ms | 500 ms |

target 超過は warning と artifact に残します。pull request job は 1 warm-up + 5 samples で全 sample に hard limitを適用します。scheduled / published release / manual acceptance job は 3 warm-up + 20 samplesで nearest-rank p50/p95 と max を判定します。hard limit は各 measured sample 直後にも検査し、超過時は partial artifact を残して即時失敗します。

artifacts / topology / core 単独 reload の補助 job は、各 OS の混在 1000 接続で 1 warm-up + 5 samples を実行します。全 sample に normal reload の hard limit を適用し、quantile は artifact に報告します。この 5 回 job を full reload の 20 回 acceptance の代替にはしません。

workload は全 session を `Play` まで進め、直前 tick に inbound gameplay と outbound event を処理します。Bedrock は reliable ordered traffic、unacked datagram、fragment reassembly を持つ状態で測定します。executable workload は同じ session population を維持したまま世代を連続 handoff します。最後の measured cutover 後も全 session の gameplay round-trip と Play population を確認してから workload を成功とします。freeze 値を含む artifact だけでは session 継続性の証明になりません。

性能 job は配布時と同じ release profile の server と packaged plugin を使います。harness の nested build と cache identity も Cargo の build profile に追従させ、debug plugin の混在を防ぎます。debug build の機能検証は別に維持します。gameplay workload は staging / prepare 中も継続し、各 connection に最大一件の outstanding command を持たせます。Bedrock の未 ACK marker は通常の gameplay 送信で解除せず、cutover 完了後に明示的に解放します。

測定終了後も client population は operator shutdown の完了まで保持し、その後に client task の停止と join を完了します。最後の sample の完了を client の一斉切断へ暗黙に変換せず、runtime shutdown と harness resource cleanup を別の completion boundary として確認します。

artifact の正規 checker は次です。
連続した committed sample は epoch revision が一つずつ進むことも検証し、同じ report の再利用や世代の飛び越しを受理しません。

```bash
cargo run -p xtask -- check-cutover-latency --input <artifact.json> --tier pull-request
cargo run -p xtask -- check-cutover-latency --input <artifact.json> --tier acceptance
```

## failure contract

- stage / prepare failure: active epoch は不変。candidate と pending state を破棄する
- frozen pre-commit failure: listener と sessionを old epoch で明示的に resumeし、aborted reportを記録する
- core journal outpace / directory revision change: policy exhaustionとして machine-readable errorを返す
- commit 後の session write failure: affected sessionだけを閉じる
- executable commit outcome uncertain: parentは fail closedし、status reconciliationへ進む

## functional acceptance

reload / upgrade 前後で次を保持します。

- player / entity identity
- inventory、cursor、open window、container state
- keepalive、mining、dropped item
- view / chunk state
- protocol / gameplay generation pin
- RakNet unacked datagram、ordered holdback、fragment reassembly、remaining timer

candidate failure 前後では epoch、core、plugin、topology、session が不変であり、commit 後は単一 epoch だけが authority です。正規 verification は workspace gateに加え、Linux/Windows executable workflow、packaged plugin load、3 connection mix の latency workflowを含みます。
