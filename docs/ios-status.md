# iOSの検証状況（2026-11-09更新）

## 最新の受入範囲

iOS 18.5のarm64 Simulatorで、Rustからの所有付き `Binding<Bool>` 操作と、製品の既存プレビュー画面の起動・設定保存・プロセス再起動後の復元を確認しました。実アプリの選択・利用時間の計測・監視・遮蔽・SwiftUI所有者の完全排出・正式配布は未達です。以下のSDK18.5の結果を、後半に残したSDK17.5の履歴と混ぜません。

### 所有付きBindingの実呼出し

Xcode 16.4 / iOS Simulator SDK18.5で、[実行42595140558](https://github.com/AutoDevJapan/phone-friction-control-ios-sdk-audit/actions/runs/42595140558)は公開の `Binding<Bool>` 初期化子、値のcopy、元とコピーのdestroyを実行しました。constructor 1・copy 1・destroy 2は検査側が実行する操作数です。実測したRust owner Drop・context destroy・deallocは各2で、単独owner試験1件とBindingのcontext1件を含みます。

[実行42605442750](https://github.com/AutoDevJapan/phone-friction-control-ios-sdk-audit/actions/runs/42605442750)では元のBindingを破棄した後もコピーの公開getter/setterが動作し、getter初期値false、setter後の読み戻しtrue、最初の遅延getterの実測差分1回を確認しました。close後の拒否2回はRust側ガードへの直接呼出しです。解放済みのSwiftオブジェクトを呼ぶ試験ではありません。

これはBool・当該SDK/Simulatorの検査です。setter内部のBL先、getterのfast path、非trivial型、SwiftUI view更新中の所有関係をすべて検証した結果ではありません。`binding_verified=false` と `full_teardown_verified=false` を維持しています。アプリを正に選択した結果や、画面全体の完全排出の代用にはしません。

### 製品の起動修正と保存復元

[PR19](https://github.com/AutoDevJapan/phone-friction-control/pull/19)をmainの `8ec2ceed983c83917d941a20926d1484686870ca` へ統合しました。iOSだけwgpuを選び、Metal adapterが15まで対応するinter-stage shader variablesに既定要求16を送って終了していた問題を修正しています。当該要求値だけをadapter上限との最小値にし、他のdevice要求とwgpuの検証は維持します。Android/desktopは従来のglowです。

検証した製品ソースは `ebb79b4f58898a5557e8806dfa66d55a0820c297` です。Rust 1.97.1で `--locked` ビルドし、追加パッチ・診断loggerを含まない同じバイナリを[実行42631724918](https://github.com/AutoDevJapan/phone-friction-control-ios-sdk-audit/actions/runs/42631724918)で再利用しました。

| 対象 | SHA-256 |
| --- | --- |
| 実行バイナリ | `231c96d8335abd518bbd86c04b34815cd36155d30b81f6fddfe6fc2f1bd204d0` |
| 実効Cargo.lock | `ce89b963e268a1dd2b724be1c1a04d54a58864ed1c8db31d05788301c680b661` |

public XCTestで自作アプリの上限設定を120分から300分へ変更し、アプリのdata container内 `Documents/usage-state.ron` の値を確認しました。終了後のPIDは6836から7710に変わり、再起動後20秒foregroundを維持しました。保存値300分と、原スクリーンショットの「5時間」が一致します。単なる既定値の再表示ではありません。

このバイナリを生成した[実行42630176629](https://github.com/AutoDevJapan/phone-friction-control-ios-sdk-audit/actions/runs/42630176629)では、ビルド後の `simctl launch` が45秒でSIGALRMとなり、PID行を取得できませんでした。その原因は未特定です。同じバイナリのXCTest経由の成功と区別し、環境起因・コード起因のどちらとも断定しません。

実行対象はpreview entry pointと別のSimulator用bundle IDです。画面の「1時間42分」などは架空のプレビューデータで、実利用時間・アプリ選択・Screen Time許可の証拠ではありません。Android専用の説明や操作がiOSのプレビューにも残る不一致は未解消です。[PR21](https://github.com/AutoDevJapan/phone-friction-control/pull/21)のpreview/runtime分離は別候補であり、platform capabilityに合う説明・操作の修正、実制御との接続が引き続き必要です。

今回の受入はiOS18.5 Simulatorのみです。実端末での動作は未検証です。

## 2026-10-11確認時点の未達条件（以下は過去の記録）

iOS製品の許可画面・アプリ選択・計測・遮蔽は未実装です。Rustの検査ホストでは、Simulatorの個人向け許可操作と、公開getterによる許可済み状態の取得まで確認しました。検査ホストでの成功と、製品の画面・選択・制御の完成は区別します。

## 計測の意味

Appleの[DeviceActivityEvent](https://developer.apple.com/documentation/deviceactivity/deviceactivityevent)は、対象アプリ・カテゴリ・Webドメインが画面の最前面にある時間を活動時間としています。動画の再生状態を確認する値ではありません。したがって、このAPIでしきい値を検出して遮蔽できたとしても、本製品の「実際に再生された時間」という条件を満たしたとは判定できません。iOSで他社アプリの再生時間を取得する経路は、まだ確認できていません。

## Rustからの接続

調査済みの `swift-runtime-sys` コミット `025a1477436cf591eed50a4f69d5c4e9e79ae757` のFamily Controls・Device Activity・Managed Settings候補は、利用可否の確認だけでした。許可・選択・しきい値の登録・遮蔽に使える実装として採用していません。

[objc2のSwift連携資料](https://docs.rs/objc2/latest/objc2/topics/swift/index.html)は、Swift専用フレームワークの自動マッピングを対象外とし、SwiftでC ABIまたはObjective-Cの入口を用意する方法を案内しています。この製品はSwiftの新規追加を許していないため、その案を実装済みの解決策とは扱いません。Rustだけで必要なSwiftの型・非同期呼出し・ピッカー・拡張を構成できないと証明したわけでもありません。

以下の追加検証で、iOS 17.5の実SDK宣言、公開クラスの取得、公開許可APIの非同期呼出しとエラーの戻りを確認しました。許可状態の確認・アプリ選択・監視・遮蔽の接続と、製品アプリ・拡張の署名と起動確認は引き続き必要です。推測した呼出規約やモックの成功をOS接続の証拠にしません。

## Rustコンパイラーで確認した境界（2026-10-10追加）

Rustの[Swift呼出規約の実験](https://github.com/rust-lang/rust/issues/156481)を、インストール済みのnightlyで追加検証しました。`tools/ios-abi-probe.rs` はApple APIを呼ばない、未定義の外部関数を使う小さなコンパイラー実験です。製品ライブラリの一部ではありません。

`rustc 1.99.0-nightly (eff8269f7 2026-07-18)` で、`aarch64-apple-ios` と `aarch64-apple-ios-sim` のLLVM IR生成は両方終了コード0でした。実際のIRには `call swiftcc void @abi_probe_external()` と `declare swiftcc void @abi_probe_external()` がありました。同じコードをWindowsのターゲットでコンパイルすると、E0570（対象ターゲットでSwift ABIをサポートしない）でした。この違いは、RustからSwift呼出規約を出力する経路が存在する証拠です。

これはSDKの型レイアウト、SwiftUIのgeneric metadataやprotocol witness table、非同期コンテキスト、Appleフレームワークのリンク、実行を確認した結果ではありません。特に、空の外部関数の呼出規約を、許可要求・アプリ選択・監視・遮蔽の接続成功へ読み替えません。Swiftコードの生成や追加も行っていません。

非公開の製品リポジトリには、SDKの宣言とコンパイラー生成物だけを収集する手動検査を準備しています。こちらのmacOSジョブは起動していません。代わりに、オーナーが承認した検査コード2ファイルだけを[別の公開リポジトリ](https://github.com/AutoDevJapan/phone-friction-control-ios-sdk-audit)へ置き、[無料の標準実行枠](https://docs.github.com/en/actions/concepts/billing-and-usage)で以下を確認しました。製品ソースや製品署名鍵を公開した結果ではありません。

## 実SDKとRustからの公開クラス取得（2026-10-10追加）

検査環境はarm64のmacOS 14、Xcode 15.4、Swift 5.10、iOS Simulator SDK 17.5です。Rustは `1.99.0-nightly (b6839f4d0 2026-07-17)` で、上のWindows実験とはホストとコンパイラーのコミットが異なります。

SDKの公開宣言から、個人向けの許可要求が `requestAuthorization(for:)` の非同期・エラーを返す呼出しであることを確認しました。引数のない古いcompletion handler版は非推奨で、個人向けの許可の代用にしていません。ピッカーは `Binding<FamilyActivitySelection>` を必要とし、選択・監視・遮蔽ではSwiftの値とトークン集合の表現も接続する必要があります。リンク用のエクスポートが存在することと、公開APIであることも区別します。内部用の許可関数を使いません。

[検査PR1](https://github.com/AutoDevJapan/phone-friction-control-ios-sdk-audit/pull/1)のRustコードで、公開の `AuthorizationCenter.shared` を呼ぶ試験を実行しました。SDKにあるシンボルと宣言を照合し、Swift 5.10のメタデータ要求と応答の定義を使って、完全なクラス情報を取得します。RustのARM64アセンブリが、その情報をSwiftのクラス用のレジスターへ渡します。新しいSwiftコードやSwiftで生成する接続層は追加していません。

試験用アプリを一時署名し、iOS 17.5のiPhone 15 Simulatorへ導入して起動しました。getterが返すオブジェクトの実クラスと、SDKのメタデータから取得したクラスのポインターが同一で、表示名が `FamilyControls.AuthorizationCenter` であることを確認しました。[実行38052477810](https://github.com/AutoDevJapan/phone-friction-control-ios-sdk-audit/actions/runs/38052477810)は成功です。先行実行はクラス名の表記を固定した検査で失敗し、型の同一性を比較する検査へ修正してから確認しました。

このgetter検査では許可要求を行わず、他社アプリの情報を読まず、アプリ選択・監視・遮蔽も実行していません。一時署名はApple Developerの会員状態・端末向けの署名・Family Controls配布許可の証拠ではありません。SwiftUIの値、各拡張の接続と製品への統合は未完了です。SDK全体の宣言は成果物へ含めず、使ったSimulatorは検査後に停止・削除しました。

## 公開許可APIの非同期呼出し（2026-10-10追加）

同じSDK 17.5・Swift 5.10・arm64 Simulatorで、公開の `requestAuthorization(for:) async throws` を呼ぶRust検査を追加しました。[検査PR2](https://github.com/AutoDevJapan/phone-friction-control-ios-sdk-audit/pull/2)では、SDKの公開非同期関数の相対ポインターと必要な呼出し用メモリのサイズ32を読み、Rustで作ったタスクがSwiftの実行系から呼ばれることを確認しました。[実行38057880461](https://github.com/AutoDevJapan/phone-friction-control-ios-sdk-audit/actions/runs/38057880461)は成功です。この段階は許可要求をしていません。

個人向け引数は、SDKのarm64 Swift interfaceの `@objc` 整数列挙型を照合して作ります。Swift 5.10の[interfaceの出力規則](https://github.com/swiftlang/swift/blob/swift-5.10-RELEASE/lib/AST/ASTPrinter.cpp)は、この列挙型の明示した数値を出力します。[整数の暗黙値の規則](https://docs.swift.org/swift-book/documentation/the-swift-programming-language/enumerations/)と、取得した値未指定の2ケースを照合しました。検査はケース名・順・型を確認し、条件が変わっていれば要求前に失敗します。

先行の[実行38058986790](https://github.com/AutoDevJapan/phone-friction-control-ios-sdk-audit/actions/runs/38058986790)は、戻り側のメモリを取り違え、解放順のエラーで失敗しました。Swift runtimeヘッダーの手動の戻り処理と、[コンパイラーが生成する戻り処理](https://github.com/swiftlang/swift/blob/swift-5.10-RELEASE/lib/IRGen/GenCall.cpp)は異なります。後者が渡す呼ばれた側のメモリから、親である呼出し側を取り出す処理へ修正しました。

[検査PR3](https://github.com/AutoDevJapan/phone-friction-control-ios-sdk-audit/pull/3)のソース `905a532` の[実行38059723021](https://github.com/AutoDevJapan/phone-friction-control-ios-sdk-audit/actions/runs/38059723021)は全工程成功です。保存した呼出し側への復帰を比較してから、子のメモリを解放し、公開許可APIのエラーの戻りを観測しました。時間切れ・解放エラーはありませんでした。版を固定した生成物と実測を、別モデルが最終確認し、Must 0件でマージしました。

結果は `error_present=true` です。公開APIへの呼出しとエラー受取りに限った確認であり、許可の承認を示しません。許可状態、アプリ選択、監視、遮蔽、他社アプリの再生時間、実行系内部の全タスクの終了は未確認です。待機は期限付きで、時間切れでは保持しているタスクへキャンセルを要求し、さらに短い待機の後に検査を失敗として終了します。OSの許可画面を自動承認する処理はありません。

製品リポジトリには検査コードと手動検査を同期しますが、macOS検査をこの非公開リポジトリで起動していません。実行したのは、承認済みの2ファイルだけを置く公開検査リポジトリの無料の標準枠です。製品のiOSアプリとして配布・統合した結果ではありません。

[検査PR4](https://github.com/AutoDevJapan/phone-friction-control-ios-sdk-audit/pull/4)の[実行38061084706](https://github.com/AutoDevJapan/phone-friction-control-ios-sdk-audit/actions/runs/38061084706)では、戻ったエラーをNSErrorへ変換し、`NSCocoaErrorDomain` のコード4097を取得しました。使った変換はSwift 5.10の実行系が持つSPIです。Family Controlsの内部の許可関数を呼ぶものではなく、この版を固定した検査のための処理です。エラーの参照は変換が消費し、返ったNSErrorだけを解放します。ドメインとコード以外の個人情報や選択値は読んでいません。

[Foundationの定義](https://github.com/swiftlang/swift-corelibs-foundation/blob/main/Sources/Foundation/NSError.swift)では4097はXPC接続の中断に当たります。通信が中断した理由はこの値だけでは確定しません。署名や許可設定の不足と断定せず、一時署名の設定と試験アプリに限った接続ログを比較して原因を調べます。Simulatorの一時署名にFamily Controlsの設定を含める検査も、Appleが開発・配布を許可した証拠にはなりません。

## UIKitの起動とSimulator向け署名（2026-10-11追加）

[検査PR5](https://github.com/AutoDevJapan/phone-friction-control-ios-sdk-audit/pull/5)では、Family Controlsのキーを直接の一時署名に追加する比較を行いました。導入はできましたが、[実行38063806271](https://github.com/AutoDevJapan/phone-friction-control-ios-sdk-audit/actions/runs/38063806271)の起動は拒否されました。対象アプリに絞ったログは `NSPOSIXErrorDomain` の162で、プロセスの起動失敗を示しました。署名にはXMLとDERの両方があり、DERがないことを原因としていません。この失敗を成功へ変更せず、Xcodeの通常のSimulator向け署名工程と比較します。

[検査PR6](https://github.com/AutoDevJapan/phone-friction-control-ios-sdk-audit/pull/6)では、Rustから公開の `UIApplicationMain` を呼ぶ検査用ホストを実装しました。Rustで登録するdelegateが空のwindowとcontrollerを設定し、アプリがactiveになったときに公開許可要求を一度だけ開始します。結果待ちは別のworkerに置き、UIKitのmain threadを待機ループで塞ぎません。

先行実行はmain queueの関数を外部シンボルとして呼び、リンクで失敗しました。公開ヘッダーにあるinline定義に合わせ、main queueの不透明なglobalのアドレスを取得する処理へ直しました。ソース `110c807` の[実行38064979076](https://github.com/AutoDevJapan/phone-friction-control-ios-sdk-audit/actions/runs/38064979076)は成功し、起動とactiveのcallback、公開の個人向け許可要求、呼出し元への復帰、NSError4097を確認しました。

空windowの設定と表示要求が戻ったことは確認しましたが、画面の画像や描画結果を確認した意味ではありません。製品画面の完成、許可の承認、選択、監視、遮蔽、実再生時間の取得、実行系内部の全タスクの終了も未確認です。

[検査PR7](https://github.com/AutoDevJapan/phone-friction-control-ios-sdk-audit/pull/7)では、Rustのobjectと静的ライブラリをXcodeの通常のapplication targetでリンクします。SwiftやCのダミーソースは追加せず、Simulator向けのFamily Controls設定をXcodeの工程で埋め込みます。開発用teamの登録、証明書の発行、provisioningの更新は行っていません。

先行の `f04f320` はビルド成功を返しても実行ファイルがありませんでした。objectを直接リンク入力へ加えた `aec9875` はリンクまで進み、ライブラリの検索先がないため失敗しました。検索先を修正した `e950b07` の[実行38067019902](https://github.com/AutoDevJapan/phone-friction-control-ios-sdk-audit/actions/runs/38067019902)で、Xcodeのビルド、arm64の実行ファイル、入口シンボル、署名、導入、UIKitの起動を確認しました。

このXcode版の公開許可要求は、20秒の待機とキャンセル後5秒の待機で結果が戻らず、検査は失敗しました。通常の起動ができたことと、許可の承認・非同期の完了を区別します。

画像と対象processのログを追加した `80d7902` の[実行38067742836](https://github.com/AutoDevJapan/phone-friction-control-ios-sdk-audit/actions/runs/38067742836)でも、検査全体は結果待ちの期限切れで失敗しました。一方、要求開始に合わせた3枚のSimulator画像で、Screen Timeへのアクセスを求めるOSの画面と「Continue」「Don’t Allow」の選択肢を確認しました。以前の即時エラー4097から、通常の許可画面での選択待ちへ進んだことが今回の成果です。選択肢を押していないため、承認・拒否後の継続と許可状態はまだ確認していません。

## Rustによる許可画面の操作検査（2026-10-11追加）

ソース `2eedf31` の[実行38075160045](https://github.com/AutoDevJapan/phone-friction-control-ios-sdk-audit/actions/runs/38075160045)では、Xcodeのテスト用ビルド、Rustで登録したXCTestCaseの発見、検査アプリの起動、公開の要求ボタンのタップまで成功しました。ホストのリンク先にXCTestがないことも実ファイルで確認しました。以前の起動待ちは、ホストとテストへ共通で渡したリンク設定によりXCTestをホストへ混入させたことが原因でした。各targetの設定へ分けて修正しています。

この実行は、アプリ自身の画面からOSの許可画面を探した検査で失敗しました。アプリのアクセシビリティツリーは検査用ボタンだけでした。一方、終了時の画像には、ホーム画面上のScreen Timeの許可画面が残っていました。要求とOS表示は確認できましたが、OSの選択肢のタップ・許可結果の戻りはまだ確認できていません。OS側の画面を公開XCTestから観測する修正を別モデルで確認し、ソース `849ec48` の追加検査を開始しました。

ソース `849ec48` の[実行38075928010](https://github.com/AutoDevJapan/phone-friction-control-ios-sdk-audit/actions/runs/38075928010)は全工程成功しました。公開のXCUIApplicationでSpringBoardの許可画面を観測し、「Don’t Allow」のタップ後に検査ホストの結果表示を待ちます。結果ファイルに公開APIの継続観測と `error_present=true` があり、1件のXCTestが18.664秒で成功しました。OSの拒否操作と公開APIのエラー受取りを確認した結果です。許可状態getterの確認と、Continue側の結果は別の検査条件です。

同じソースのContinue側の[実行38076292581](https://github.com/AutoDevJapan/phone-friction-control-ios-sdk-audit/actions/runs/38076292581)では、Continueのタップは成功しました。その先に別のOS画面があり、「Allow with Passcode」と「Don’t Allow」が表示されました。検査がこの2段目を操作していないため、公開APIの戻りはなく、結果表示の30秒待機で失敗しました。Continueだけで許可済みとは判定せず、表示された2段目の操作と、その後の画面・戻りを追加検証します。パスコードの入力や、認証を迂回する呼出しは行っていません。

ソース `5cce94b` の[実行38077076147](https://github.com/AutoDevJapan/phone-friction-control-ios-sdk-audit/actions/runs/38077076147)で、2段目のボタンのタップに成功しました。その後の画像と画面ツリーは「Passcode field」という入力欄を示しました。未入力のため公開APIは戻らず、検査は失敗しました。次の検査は、毎回新しく作成するarm64 Simulatorに限定し、通常の入力欄から非秘密の合成値を1回だけ送ります。入力後の結果と許可状態はまだ未確認です。

ソース `0afb76e` の[実行38078299191](https://github.com/AutoDevJapan/phone-friction-control-ios-sdk-audit/actions/runs/38078299191)では、公開の入力APIをSDKヘッダーと照合し、1回の合成入力でOSの「Approved to Access Screen Time」画面まで進みました。最後のDoneを押していなかったため、全体は完了待ちで失敗しました。Done操作を追加した最終ソース `65f67e2` の[実行38079380084](https://github.com/AutoDevJapan/phone-friction-control-ios-sdk-audit/actions/runs/38079380084)は全工程成功し、1件のXCTestが29.209秒で成功しました。公開APIからの戻りと `error_present=false` を確認しています。

最終統合レビューはMust 0件、CodeQLも成功し、[検査PR7](https://github.com/AutoDevJapan/phone-friction-control-ios-sdk-audit/pull/7)をマージしました。公開mainの `e211950` のCodeQLも成功しています。製品リポジトリへ当初同期した2ファイルは、この公開mainと同じソースでした。この段階の無エラーの戻りを、状態getterの取得や製品UI・監視・遮蔽の完成とは判定しません。後続の状態getterと選択値の検査は、以下の別の実測として記録します。

## 許可状態を読むための型情報（2026-10-11追加）

ソース `64ced79` の[実行38077790004](https://github.com/AutoDevJapan/phone-friction-control-ios-sdk-audit/actions/runs/38077790004)は全工程成功しました。SDKの公開宣言とexportを照合したうえで、AuthorizationStatusの完全なenumメタデータを取得しました。サイズ・stride・alignmentはいずれも1です。公開のauthorizationStatus getter入口は、ページ内の最大128バイトだけ採取しました。

この検査は許可要求をしない独立プロセスで行いました。getter・値の操作表・enum値の構築は呼んでいません。型情報と入口の取得を、許可状態の取得や承認の確認とは扱いません。最初の起動要求はGitHubがHTTP 500を返しましたが、実行一覧に同じソースの受付があることを確認し、再送していません。

## 実getterによる許可状態の取得（2026-10-11追加）

ソース `e731b48` の[実行38080955889](https://github.com/AutoDevJapan/phone-friction-control-ios-sdk-audit/actions/runs/38080955889)は全工程成功しました。要求をしない新規Simulatorの独立プロセスで、公開の状態getter、rawValue getter、実行系のenum case名取得をそれぞれ1回呼び、raw値0と `notDetermined` の一致を確認しました。型情報だけの検査とは別の結果です。

[検査PR8](https://github.com/AutoDevJapan/phone-friction-control-ios-sdk-audit/pull/8)で、状態取得を最新のXcode検査ホストへ統合しました。公開許可APIの戻りを受け取ってからメインスレッドで状態を読み、SDKの結果と状態を同じファイルへ保存します。実getterの呼出し・raw値・反射したcase名の照合を終えてから、XCTestに完了を知らせます。

ソース `9fcc8dd` の[許可側の実行38082434285](https://github.com/AutoDevJapan/phone-friction-control-ios-sdk-audit/actions/runs/38082434285)は全工程成功しました。OSの許可操作後にAPIがエラーなしで戻り、実getterがraw値2と `approved` を返しました。状態・raw値・case名の取得は各1回で、メインスレッドで読んだことも確認しました。

同じソースの[拒否側の実行38081982214](https://github.com/AutoDevJapan/phone-friction-control-ios-sdk-audit/actions/runs/38081982214)は、XCTestの1件が16.256秒で成功した後、状態の期待値照合で失敗しました。OSの「Don't Allow」操作後にAPIはエラーを返し、実getterはraw値0と `notDetermined` を返しています。`denied` を前提にした検査条件が実測と違いました。

[Appleの個人向け許可APIの説明](https://developer.apple.com/documentation/familycontrols/authorizationcenter/requestauthorization(for:))は、承認または取消後に状態を設定するか、エラーを投げるとしています。拒否後に必ず `denied` へ変わるとは仮定せず、APIのエラーと「状態が許可済みではないこと」を照合するよう修正しました。Rust側はraw値とcase名の対応を厳密に検証し、許可側には引き続きエラーなし・raw値2・`approved` を要求します。

修正ソース `f82e110` の[拒否側の実行38082622453](https://github.com/AutoDevJapan/phone-friction-control-ios-sdk-audit/actions/runs/38082622453)は全工程成功し、エラーとraw値0・`notDetermined` を再確認しました。`denied` の取得成功とは扱いません。拒否条件の修正はRustや許可側の操作・照合を変更しておらず、許可側の成功検査を重複実行していません。

統合と期待値修正の別モデルレビューは指摘0件、最終CodeQLも成功し、検査PR8をマージしました。公開main `b3bb985` のCodeQLも成功しています。ここまでの成果は検査ホストでの許可と状態取得であり、製品への統合・可変なアプリ選択・監視・遮蔽・他社動画の再生時間取得・正式配布の成功ではありません。

## 選択値とBindingの型情報（2026-10-11追加）

ソース `4db3afa` の[実行38070513964](https://github.com/AutoDevJapan/phone-friction-control-ios-sdk-audit/actions/runs/38070513964)で、公開のメタデータと値の操作表を読み、型のサイズと配置を取得しました。`FamilyActivitySelection` はサイズ56・stride56・alignment8、`Binding<FamilyActivitySelection>` は72・72・8でした。固定値を仮定してメモリへ値を詰めた結果ではなく、実行系が返した情報です。

Swiftの保持用boxを確保し、未初期化の値領域が要求されたアラインメントを満たすことと、投影した領域が返された領域と一致することを確認しました。未初期化の値を破棄する操作は使わず、その状態のbox専用の解放を行いました。

公開コンストラクターとコピー操作の入口を、ページ境界内の最大128バイトに限って記録しました。SDKの宣言にある初期化用の記号と、実際に公開される値を作る入口の記号は異なります。[Swift 5.10の名前の規則](https://github.com/swiftlang/swift/blob/swift-5.10-RELEASE/docs/ABI/Mangling.rst)に合わせ、公開宣言と実際のexportを正確に照合してから取得しています。

この検査では、選択値の構築、値の読取り、初期化関数・クロージャ・コピー関数の呼出し、許可要求を行っていません。Bindingの可変な選択を受け取るには、引数の配置、getter/setterの文脈、値と参照の所有権を追加確認し、実際の受渡しを検証する必要があります。型情報とメモリ処理の成功を、アプリ選択や製品対応の成功として扱いません。

ソース `4ea0160` の[実行38075174827](https://github.com/AutoDevJapan/phone-friction-control-ios-sdk-audit/actions/runs/38075174827)でgetter・setterの公開入口も採取しました。setterに直接呼出しを要求したチェックで検査全体は失敗しました。実際のsetterは間接呼出しを使い、getterは最初の無条件分岐で別の実装へ進みます。最初の直接呼出しを単純に探す旧方式は、その分岐を無視して隣接コードを拾っていました。実行が分岐する位置で止める方式へ修正し、ソース `8d0d2a6` で追加検査を開始しました。SDK命令の再配置後の絶対アドレスを元のSDKの分岐先とは扱いません。

ソース `8d0d2a6` の[実行38075934628](https://github.com/AutoDevJapan/phone-friction-control-ios-sdk-audit/actions/runs/38075934628)は全工程成功しました。getterの先頭の直接分岐先、コンストラクターの直接呼出し先を、元の入口からの相対値と同一イメージの照合で採取しました。setterはoffset40の間接呼出しまでを確認し、間接呼出し先は読みません。値の構築・API呼出し・クロージャ呼出し・許可要求はすべて0件です。

ソース `7591545` の[実行38077219284](https://github.com/AutoDevJapan/phone-friction-control-ios-sdk-audit/actions/runs/38077219284)では、独立した値の寿命を調べる検査も全工程成功しました。Rustで別々の領域を3つ確保し、公開のSelection.init()を1回、コピーを2回、代入を1回、破棄を3回呼びました。元の値を破棄した後も複製した値を別の領域へコピーでき、3つの領域をそれぞれ解放しました。値の内容は出力しません。

これは初期状態のSelection値の寿命を確認した前提工程です。この段階では可変Bindingの生成・getter/setter・クロージャを呼んでおらず、アプリ選択・許可要求・監視・遮蔽も行っていませんでした。初期状態の値を可変Bindingの代用品にはしていません。

[検査PR9](https://github.com/AutoDevJapan/phone-friction-control-ios-sdk-audit/pull/9)で、確認済みの選択値の検査を最新の許可操作・状態getterコードへ統合しました。選択モードは許可要求を呼ばず、許可操作・状態取得とは別に実行します。初期化・値の操作を行うモジュールは成功済み `7591545` とバイト一致しています。

最初の起動要求は、ワークフローの1工程がGitHubの21,000文字上限を超えたため、HTTP422で受理されませんでした。準備とビルドを別工程に分け、公開SDK識別子と検査オプションだけを引き継ぐよう修正しました。追加レビューは指摘0件、ソース `d10a1cf` の[実行38083393269](https://github.com/AutoDevJapan/phone-friction-control-ios-sdk-audit/actions/runs/38083393269)は全工程成功しました。3領域の確保・解放、初期化1・コピー2・代入1・破棄3、元値を破棄した後のコピーを、統合後にも確認しました。

PR9のmain取り込み時の競合は、実測済み `d10a1cf` の2ファイルと同じ内容を保って解消しました。製品リポジトリへ同期する検査2ファイルもSHA256でバイト一致を確認しています。製品クレートやAndroidの動作は変更していません。

カテゴリを指定する公開初期化子の[実行38082809973](https://github.com/AutoDevJapan/phone-friction-control-ios-sdk-audit/actions/runs/38082809973)も成功しました。公開宣言とexportを照合し、入口の最大128バイトをページ内で採取した結果です。最初の戻り命令までの28バイトを復号し、Boolの入力と間接戻り先の受渡しを確認しました。この入口検査では、その初期化子やBindingを呼んでいません。

## 実Bindingの生成と値の更新（2026-10-11追加）

[検査PR10](https://github.com/AutoDevJapan/phone-friction-control-ios-sdk-audit/pull/10)で、Rustが保持する可変なSelection値を公開のBindingへつなぎました。公開初期化子でfalse・trueの異なる値をそれぞれ作り、getterが現在値をコピーし、setterが借りた入力を保存値へ代入します。値のフィールドを手で変更したり、定数のBindingを可変な選択の代用品にしたりしていません。

コンストラクターへ渡すSelectionの型情報と、公開getter・setterが使うBinding自身の型情報は異なります。追加の命令照合でこの区別を確認し、アクセサーがBindingのgeneric引数からSelectionを取り出すことを検査してから呼びます。旧ソースの[実行38084347523](https://github.com/AutoDevJapan/phone-friction-control-ios-sdk-audit/actions/runs/38084347523)は、誤った型情報を渡す前に取り消しました。ビルド・実行の工程はどちらも省略され、誤った型情報での実呼出しはありません。

修正ソース `4412036` の[実行38084816899](https://github.com/AutoDevJapan/phone-friction-control-ios-sdk-audit/actions/runs/38084816899)は全工程成功しました。実Bindingの初期化1回、全体のコピー1回、公開getter3回、公開setter2回を確認しました。getterクロージャは4回、setterクロージャは2回呼ばれ、初期化中のgetter呼出しも実際の回数に含めています。

初期値falseとの一致、変更後のtrueとの一致・falseとの不一致、復元後のfalseとの一致を、値内容を出力しない5回の比較で確認しました。Bindingの元の値を破棄した後も、複製したBindingで読取り・書込みを続けました。確保・解放は10領域ずつ、残ったcallbackは0です。

setterには専用コピーの所有権をSDKへ一度だけ渡し、Rustのクロージャは借りた入力を破棄しません。Bindingは自身の値操作表で全体をコピー・破棄し、保存値はすべてのBindingを破棄した後に解放します。記録した値操作回数は検査コードが直接呼んだ回数であり、SDK内部の破棄をすべて数えた意味ではありません。

修正後の別モデルレビューは指摘0件、最終CodeQLも成功し、検査PR10をマージしました。同期した検査2ファイルは、公開main `1abcbe3` と同じ実測済みソースです。この結果は可変な値の受渡しの前提を確認したもので、FamilyActivityPickerの画面表示・実際のアプリ選択・選択token・Device Activity・Managed Settings・拡張・製品UI・正式配布の成功ではありません。

## アカウントと配布

Apple Developerのログインは完了しておらず、会員・既存署名・配布許可の状態は未確認です。保存済み自動入力は表示されず、指定表のApple検索結果は0件でした。iPhoneによるサインインは検証中の表示から先へ進まず、本人の承認画面へ到達したとは記録していません。新規有料契約は行っていません。

[Appleの配布許可手順](https://developer.apple.com/documentation/familycontrols/requesting-the-family-controls-entitlement)では、Account HolderによるFamily Controlsの配布許可申請が必要です。使うScreen Time拡張にも申請し、承認後の能力と対応する配布方法を確認します。開発用の能力が存在することを、配布許可の証拠にしません。

これらは共有ToDo1614で継続する条件です。

## 選択画面と表示用クラスの型情報（2026-10-11追加）

[検査PR11](https://github.com/AutoDevJapan/phone-friction-control-ios-sdk-audit/pull/11)の[実行38087686599](https://github.com/AutoDevJapan/phone-friction-control-ios-sdk-audit/actions/runs/38087686599)で、公開のFamilyActivityPickerの型情報とSwiftUI.Viewへの適合、UIHostingControllerの特殊化したクラス情報を取得しました。Pickerはsize146・stride152・alignment8で、表示用クラスの直接の親がUIViewControllerであることを照合しました。

公開初期化子の入口と、その最初の直接呼出し先を最大128バイトの範囲で採取しました。命令を別のファイルへ移した後の表示アドレスは、SDKの呼出し先として使いません。型情報の取得と公開宣言の照合を、画面表示や値の生成とは区別します。

この実行ではPickerと表示用コントローラーの初期化子を呼んでいません。値の生成、画面表示、アプリ選択、選択token、監視、遮蔽は未確認です。前担当が準備したPicker値の生成・コピー・破棄の検査は、別モデルで引数と所有権を確認してから実行します。

## 選択画面の値の生成・コピー・破棄（2026-10-12追加）

[検査PR12](https://github.com/AutoDevJapan/phone-friction-control-ios-sdk-audit/pull/12)のソース`666e160`を、[実行38149959145](https://github.com/AutoDevJapan/phone-friction-control-ios-sdk-audit/actions/runs/38149959145)で確認しました。Xcodeによるビルドと署名、iOS17.5のSimulatorでの起動、結果照合は全工程成功しました。

Rustが保持する実Bindingの専用コピーを公開のFamilyActivityPicker初期化子へ一度だけ渡しました。Pickerを1回生成し、その値の操作表で1回コピーして、元と複製をそれぞれ破棄しました。入力のBindingの所有権はSDKへ移し、Rustはその入力を二重に破棄しません。Bindingの保存値は、すべてのPickerとBindingの所有者を破棄するまで保持します。

Rustが確保した領域は全体で13個、解放も13個です。残るPickerとBindingの所有者、処理中のcallbackはすべて0で、保存値を指すポインターの解除を確認しました。これらはRustが直接管理する領域と呼出しの集計であり、SDK内部の全確保・全参照の解放を数えた意味ではありません。

レビューで、要求を呼ばない検査にも許可要求の入力を必須にする矛盾が見つかりました。このモードは入力をfalseに限定し、許可要求のコードをコンパイルから除外します。独立して生成したIRにも要求経路の関数がないことを確認してから、Simulatorへ導入します。修正後の別モデルレビューはMust0、CodeQLも成功しました。

今回の結果は、選択画面を構成する値の寿命の確認です。UIHostingControllerの生成・画面表示・利用者によるアプリ選択・選択token・監視・遮蔽・製品への統合・正式配布は未確認です。許可取得の過去の成功を繰り返さず、次は表示用コントローラーと実際の選択へ進めます。

## 実際の選択画面の表示と撮影（2026-10-12追加）

[公開検査PR13](https://github.com/AutoDevJapan/phone-friction-control-ios-sdk-audit/pull/13)のソース`f77c08c`を、[実行38159659196](https://github.com/AutoDevJapan/phone-friction-control-ios-sdk-audit/actions/runs/38159659196)で確認しました。SDKの宣言検査、ビルドと署名、Simulatorでの構築・撮影・結果照合は全工程成功しました。実画像で「Choose Activities」、検索欄、アプリ・カテゴリの一覧が表示されていることを確認しました。

Rustから公開のUIHostingController初期化子へPicker値の専用コピーを一度だけ渡しました。返ったクラスの型情報がSDKから取得したものと一致し、親クラス・実ウィンドウへの接続・可視状態を確認しました。終了時はrootを取り外し、自分のcontrollerとwindowの参照をそれぞれ一度解放しました。

UIKitが内部に保持する参照の終了は確認できません。callbackが参照する選択値・型情報・検査状態は、この使い捨てプロセスの終了まで保持します。Rustの値領域は確保5・解放4で、保存値の1領域を意図して保持しています。完全な後始末やSDK内部の全所有者の解放を主張しません。callback getterは10回、setterは0回、終了時に処理中のcallbackは0でした。

この検査では許可要求と状態getterを呼んでいません。アプリを選ぶ操作、選択token、監視、遮蔽、製品への統合、正式配布は未確認です。workflowの自動的な画面内容検証は未実装で、実画像を目視した表示確認とは区別します。

先行の4実行は、新しい呼出しに進む前のSDK宣言検査で失敗しました。空白・利用条件の注記・複数行・属性マクロの表記を単純な1行の文字列として扱っていたことが原因でした。UIKitの横空白を行ごとにそろえ、Objective-C関数と図形の構造はClangが解釈した型とフィールドで照合する方式へ変更しました。関数名・戻り型・引数、図形の精度・順序・特殊配置の拒否を維持しています。失敗結果と全ログは保持し、最新の成功へ置き換えていません。
