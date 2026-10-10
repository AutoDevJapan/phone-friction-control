# iOSの未達条件（2026-10-10確認）

iOS製品の許可画面・アプリ選択・計測・遮蔽は未実装です。検査用のRustコードから公開の個人向け許可APIを呼び、エラーの戻りを受け取るところまで確認しました。許可の承認や、Androidの計測結果をiOSの制御成功として扱いません。

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

## アカウントと配布

Apple Developerのログインは完了しておらず、会員・既存署名・配布許可の状態は未確認です。保存済み自動入力は表示されず、指定表のApple検索結果は0件でした。iPhoneによるサインインは検証中の表示から先へ進まず、本人の承認画面へ到達したとは記録していません。新規有料契約は行っていません。

[Appleの配布許可手順](https://developer.apple.com/documentation/familycontrols/requesting-the-family-controls-entitlement)では、Account HolderによるFamily Controlsの配布許可申請が必要です。使うScreen Time拡張にも申請し、承認後の能力と対応する配布方法を確認します。開発用の能力が存在することを、配布許可の証拠にしません。

これらは共有ToDo1614で継続する条件です。
