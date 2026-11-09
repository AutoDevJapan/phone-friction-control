# 広告版のBack再入場と同意試験

main `a2bc0d2` の広告試用APKで、BackでActivityが終了した後に再入場すると、同じプロセスでActivityを再生成し、`EventLoop can't be recreated`となることを再現しました。EEA試験設定の候補でも同じエラーでした。

修正は広告版のBackをタスク背景化へ接続し、既存Activityを保持します。新しいAndroid Back経路が旧コールバックを迂回しないよう、広告版Activityだけに `enableOnBackInvokedCallback=false` を設定します。通常版のNativeActivityはこの変更の対象外です。その他のActivity破棄経路すべてを解決したとは説明しません。

候補APK SHA-256 `a1ee70845fe2b0708c9be6f8812d76980ec1b94e3b494a7472ac50ef39b8bd7a`、Android37/x86_64/16KBで確認。通常Back→再入場でPID11608とActivityRecord229423154を保持し、ホーム描画へ復帰しました。検査途中の再入場には強制停止を使っていません。[画面](back-reentry.png)。生成器の回帰テストは22件成功。

同じ候補APKへ試験用の有効期限1794195025707を与え、Back→再入場後も同一期限と保存状態を保持しました。[前](active-before.ron)と[後](active-after.ron)は同一バイト列で、[広告のない休憩画面](active-break.png)を確認しました。期限注入は通常の上限到達試験とは別です。

`ads-test`だけに公式UMPのEEA debug geographyを追加しました。productionビルドはdebug geographyを設定しません。両Androidビルドと両featureの判定テストは成功。Googleのsample App IDとsample banner IDだけを使用します。

EEA設定の同じネイティブ候補（Back修正前のパッケージ）で、[公式Publisher Test Adsフォーム](consent-form.png)の「Do not consent」を操作し、SDKが `CONSENT_SIGNAL_SUFFICIENT` を返し、フォームが閉じて[通常画面とTest Ad](consent-denied.png)へ戻りました。前後の保存状態は一致、期限は両方Noneでした。拒否は必ずしも広告要求不可を意味しません。最終Back修正APKではprivacy設定要求が失敗案内へ戻り、設定や制御を変更しないことを確認しました。最終APKでの新規フォーム拒否を再実施した結果ではありません。

実配信や製品用広告ユニットの承認とは別の検査です。通信は通常のSDK診断とdata-flow確認に限定し、暗号化payload不在の実測とはしません。
