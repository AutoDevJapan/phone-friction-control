# 追加受入の証拠

対象ソース、試用APKのハッシュ、設定と観測値は [observations.json](observations.json)、APKから実際に抽出した設定は [apk-manifest.txt](apk-manifest.txt)、通常UI操作後に端末から取得した保存設定は [normal-config.ron](normal-config.ron) に対応します。

observations.jsonは検査時のコマンド応答から必要項目を転記した記録です。新しい実行の生ログではありません。短縮設定による発動、別の期限注入による画面確認、通常UI操作による保存を別項目にしています。

画像は当日の専用仮想端末から取得したものです。対象は検査用Chromeと本アプリで、個人のアカウント・通知・コンテンツを含めていません。

| 観測 | 画像 |
|---|---|
| 同意説明 | [disclosure.png](disclosure.png) |
| 再生状態取得不能・42,000ms保存の試験 | [unavailable.png](unavailable.png) |
| 別の期限注入後の休憩画面 | [break.png](break.png) |
| 満了後のホーム | [expired.png](expired.png) |
| 通常UIの240分設定を再起動後に表示 | [ui-save.png](ui-save.png) |

日跨ぎは端末の時刻変更を行わず、次の既存単体テストで確認しました。両方とも1件成功・失敗0です。

```text
cargo test --locked --lib midnight_preserves_active_break_and_completed_break_is_once_per_day
cargo test --locked --lib changing_the_clock_back_restores_the_day_without_another_break
```

通常導線の再現は、説明から使用状況を許可し、Chromeを選び、中断説明からユーザー補助を許可します。上限を120分から240分へ画面で変更し、アプリを強制停止・起動して保持を確認し、120分へ戻します。強制停止でユーザー補助が解除されるOS条件があるため、再起動をサービス自動復旧の証拠にしません。

取得不能試験は、再生モードと42,000msの既存履歴を検査用端末へ与え、対象のMediaSessionを取得できない間も値が保持されることを確認します。注入値を実再生の実測値として使いません。短縮試験は別の設定で前面モード・1分上限・2分休憩を与え、前面合計・休憩期限・前面Activityを観測します。通常製品の設定範囲とは別です。

配布APK、署名秘密鍵、認証値、端末全体のログはこの証拠に含めません。ファイル照合は [sha256.json](sha256.json) を参照してください。
