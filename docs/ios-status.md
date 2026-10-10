# iOSの未達条件（2026-10-10確認）

iOSの許可・アプリ選択・計測・遮蔽は未実装です。Androidの計測結果や、フレームワークの存在確認をiOSの成功として扱いません。

## 計測の意味

Appleの[DeviceActivityEvent](https://developer.apple.com/documentation/deviceactivity/deviceactivityevent)は、対象アプリ・カテゴリ・Webドメインが画面の最前面にある時間を活動時間としています。動画の再生状態を確認する値ではありません。したがって、このAPIでしきい値を検出して遮蔽できたとしても、本製品の「実際に再生された時間」という条件を満たしたとは判定できません。iOSで他社アプリの再生時間を取得する経路は、まだ確認できていません。

## Rustからの接続

調査済みの `swift-runtime-sys` コミット `025a1477436cf591eed50a4f69d5c4e9e79ae757` のFamily Controls・Device Activity・Managed Settings候補は、利用可否の確認だけでした。許可・選択・しきい値の登録・遮蔽に使える実装として採用していません。

[objc2のSwift連携資料](https://docs.rs/objc2/latest/objc2/topics/swift/index.html)は、Swift専用フレームワークの自動マッピングを対象外とし、SwiftでC ABIまたはObjective-Cの入口を用意する方法を案内しています。この製品はSwiftの新規追加を許していないため、その案を実装済みの解決策とは扱いません。Rustだけで必要なSwiftの型・非同期呼出し・ピッカー・拡張を構成できないと証明したわけでもありません。

以下の追加検証で、iOS 17.5の実SDK宣言と、公開クラスの取得に必要なABIを確認しました。許可要求・アプリ選択・監視・遮蔽の接続と、アプリ・拡張の署名と起動確認は引き続き必要です。推測した呼出規約やモックの成功をOS接続の証拠にしません。

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

これは、Rustからこの公開getterを呼ぶことに限ったSimulatorの確認です。許可要求は行わず、他社アプリの情報を読まず、アプリ選択・監視・遮蔽も実行していません。一時署名はApple Developerの会員状態・端末向けの署名・Family Controls配布許可の証拠ではありません。SwiftUIの値、非同期処理、各拡張の接続と製品への統合は未完了です。SDK全体の宣言は成果物へ含めず、使ったSimulatorは検査後に停止・削除しました。

## アカウントと配布

Apple Developerのログインは完了しておらず、会員・既存署名・配布許可の状態は未確認です。保存済み自動入力は表示されず、指定表のApple検索結果は0件でした。iPhoneによるサインインは検証中の表示から先へ進まず、本人の承認画面へ到達したとは記録していません。新規有料契約は行っていません。

[Appleの配布許可手順](https://developer.apple.com/documentation/familycontrols/requesting-the-family-controls-entitlement)では、Account HolderによるFamily Controlsの配布許可申請が必要です。使うScreen Time拡張にも申請し、承認後の能力と対応する配布方法を確認します。開発用の能力が存在することを、配布許可の証拠にしません。

これらは共有ToDo1614で継続する条件です。
