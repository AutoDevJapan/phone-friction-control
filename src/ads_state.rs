//! 検証用バナー広告の状態。JNIに依存しないため、PCのテストでも同じ実装を動かす。
//!
//! T は保持する参照の型。Android では GlobalRef、テストでは () を使う。
// PCのテスト版では、Android側からだけ使う項目が未使用になる。
#![cfg_attr(not(all(target_os = "android", feature = "ads-test")), allow(dead_code))]

pub(super) const LEFT: &str = "ホーム画面を離れたため広告を閉じました。";
pub(super) const PRIVACY_LEFT: &str =
    "ホーム画面を離れたため、プライバシー設定の処理を中止しました。";
pub(super) const SLOT_MISSING: &str = "広告欄が見えないため広告を閉じました。";
pub(super) const TOO_SMALL: &str = "広告欄が小さいため広告を表示しません。";
pub(super) const NO_CONSENT: &str =
    "広告の同意を確認できないため、広告は表示しません。アプリはそのまま使えます。";
pub(super) const DENIED: &str = "同意の設定により広告は表示しません。アプリはそのまま使えます。";
pub(super) const FAILED: &str = "広告を準備できませんでした。アプリはそのまま使えます。";
pub(super) const LOAD_FAILED: &str = "広告を読み込めませんでした。アプリはそのまま使えます。";
pub(super) const PRIVACY_FAILED: &str = "プライバシー設定を開けませんでした。";
pub(super) const PRIVACY_CLOSED: &str =
    "プライバシー設定を閉じました。広告を見るときはもう一度ボタンを押してください。";
pub(super) const CLOSED: &str = "広告を閉じました。";
pub(super) const BREAK: &str = "休憩中は広告を表示しません。";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Stage {
    Idle,
    Update,
    Form,
    Check,
    Init,
    Ready,
    Loading,
    Shown,
    PrivacyUpdate,
    PrivacyForm,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Event {
    UpdateOk,
    UpdateFailed,
    Dismissed { error: bool },
    Loaded,
    LoadFailed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Op {
    Update,
    Form { privacy: bool },
    Check,
    Init,
    Load([i32; 4]),
    Layout([i32; 4]),
}

pub(super) enum Step<T> {
    Nothing,
    Cleanup(T),
    Run(Op, u64),
}

pub(super) struct Manager<T> {
    pub(super) epoch: u64,
    pub(super) stage: Stage,
    /// 今の段階の操作を送り済みか。段階ごとに1回だけ送る。
    pub(super) queued: bool,
    pub(super) initialized: bool,
    pub(super) init_running: bool,
    pub(super) placed: Option<[i32; 4]>,
    pub(super) ad_size: (i32, i32),
    pub(super) notice: &'static str,
    pub(super) consent: Option<T>,
    pub(super) callbacks: Vec<(u64, T)>,
    pub(super) view: Option<T>,
}

impl<T> Manager<T> {
    pub(super) const fn new() -> Self {
        Self {
            epoch: 0,
            stage: Stage::Idle,
            queued: false,
            initialized: false,
            init_running: false,
            placed: None,
            ad_size: (0, 0),
            notice: "",
            consent: None,
            callbacks: Vec::new(),
            view: None,
        }
    }

    pub(super) fn set(&mut self, stage: Stage) {
        self.stage = stage;
        self.queued = false;
    }

    /// 世代を進めて、それまでのコールバックと広告をすべて無効にする。
    pub(super) fn cancel(&mut self, notice: &'static str) -> Option<T> {
        self.epoch += 1;
        self.set(Stage::Idle);
        self.callbacks.clear();
        self.placed = None;
        self.notice = notice;
        self.view.take()
    }

    pub(super) fn begin(&mut self, stage: Stage) -> Option<T> {
        let view = self.cancel("");
        self.set(stage);
        view
    }

    /// プライバシー設定を含め、何かの処理が進行中か。
    pub(super) fn active(&self) -> bool {
        self.stage != Stage::Idle
    }

    /// 広告の表示に向けた処理が進行中か。広告ボタンの表示切り替えに使う。
    pub(super) fn ad_active(&self) -> bool {
        !matches!(
            self.stage,
            Stage::Idle | Stage::PrivacyUpdate | Stage::PrivacyForm
        )
    }

    pub(super) fn event(&mut self, epoch: u64, event: Event) -> Option<T> {
        use Stage::*;
        if epoch != self.epoch {
            return None;
        }
        match (self.stage, event) {
            (Update, Event::UpdateOk) => self.set(Form),
            (PrivacyUpdate, Event::UpdateOk) => self.set(PrivacyForm),
            (Form, Event::Dismissed { error: false }) => self.set(Check),
            (Update, Event::UpdateFailed) | (Form, Event::Dismissed { error: true }) => {
                return self.cancel(NO_CONSENT);
            }
            (PrivacyUpdate, Event::UpdateFailed)
            | (PrivacyForm, Event::Dismissed { error: true }) => {
                return self.cancel(PRIVACY_FAILED);
            }
            (PrivacyForm, Event::Dismissed { error: false }) => return self.cancel(PRIVACY_CLOSED),
            (Loading, Event::Loaded) => self.set(Shown),
            (Loading, Event::LoadFailed) => return self.cancel(LOAD_FAILED),
            _ => {}
        }
        None
    }

    pub(super) fn checked(&mut self, epoch: u64, can_request_ads: bool) {
        if epoch != self.epoch || self.stage != Stage::Check {
            return;
        }
        if !can_request_ads {
            self.cancel(DENIED);
        } else if self.initialized {
            self.set(Stage::Ready);
        } else {
            self.set(Stage::Init);
        }
    }

    /// 初期化の完了は、今 Init で待っている依頼だけを進める。取り消し済みの依頼は Idle のまま。
    pub(super) fn init_done(&mut self, ok: bool) {
        self.init_running = false;
        self.initialized |= ok;
        if self.stage == Stage::Init {
            if ok {
                self.set(Stage::Ready);
            } else {
                self.cancel(FAILED);
            }
        }
    }

    pub(super) fn step(&mut self, home: bool, bounds: Option<[i32; 4]>) -> Step<T> {
        // ホームを離れたら、プライバシー設定を含むすべての処理を止める。
        // 広告欄が見えないだけなら、広告の処理だけを止める。
        let notice = match (self.stage, home) {
            (Stage::Idle, _) => None,
            (Stage::PrivacyUpdate | Stage::PrivacyForm, false) => Some(PRIVACY_LEFT),
            (Stage::PrivacyUpdate | Stage::PrivacyForm, true) => None,
            (_, false) => Some(LEFT),
            (_, true) => bounds.is_none().then_some(SLOT_MISSING),
        };
        if let Some(notice) = notice {
            return self.cancel(notice).map_or(Step::Nothing, Step::Cleanup);
        }
        if matches!(self.stage, Stage::Loading | Stage::Shown)
            && let Some(b) = bounds
            && self.placed != Some(b)
        {
            if b[2] < self.ad_size.0 || b[3] < self.ad_size.1 {
                return self.cancel(TOO_SMALL).map_or(Step::Nothing, Step::Cleanup);
            }
            self.placed = Some(b);
            return Step::Run(Op::Layout(b), self.epoch);
        }
        if self.queued {
            return Step::Nothing;
        }
        let op = match (self.stage, bounds) {
            (Stage::Update | Stage::PrivacyUpdate, _) => Op::Update,
            (Stage::Form, _) => Op::Form { privacy: false },
            (Stage::PrivacyForm, _) => Op::Form { privacy: true },
            (Stage::Check, _) => Op::Check,
            (Stage::Init, _) if !self.init_running => {
                self.init_running = true;
                Op::Init
            }
            (Stage::Ready, Some(b)) => Op::Load(b),
            _ => return Step::Nothing,
        };
        self.queued = true;
        Step::Run(op, self.epoch)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SLOT: Option<[i32; 4]> = Some([0, 100, 1080, 300]);
    const AD: (i32, i32) = (1080, 150);

    fn next(m: &mut Manager<()>) -> Option<Op> {
        m.step(true, SLOT).op()
    }

    fn consent(m: &mut Manager<()>) {
        m.begin(Stage::Update);
        assert_eq!(next(m), Some(Op::Update));
        m.event(m.epoch, Event::UpdateOk);
        assert_eq!(next(m), Some(Op::Form { privacy: false }));
        m.event(m.epoch, Event::Dismissed { error: false });
        assert_eq!(next(m), Some(Op::Check));
    }

    /// ads.rs の load() が Ready から Loading へ進めるときと同じ更新をする。
    fn place(m: &mut Manager<()>) {
        assert_eq!(next(m), Some(Op::Load(SLOT.unwrap())));
        m.view = Some(());
        m.ad_size = AD;
        m.placed = SLOT;
        m.set(Stage::Loading);
    }

    fn shown(m: &mut Manager<()>) {
        consent(m);
        m.checked(m.epoch, true);
        if !m.initialized {
            assert_eq!(next(m), Some(Op::Init));
            m.init_done(true);
        }
        place(m);
        m.event(m.epoch, Event::Loaded);
        assert_eq!(m.stage, Stage::Shown);
    }

    #[test]
    fn late_callback_after_close_cannot_revive() {
        let mut m = Manager::<()>::new();
        m.begin(Stage::Update);
        let old = m.epoch;
        assert_eq!(next(&mut m), Some(Op::Update));
        m.cancel(CLOSED);
        m.event(old, Event::UpdateOk);
        m.checked(old, true);
        assert_eq!(m.stage, Stage::Idle);
        assert_eq!(next(&mut m), None);
    }

    #[test]
    fn consent_error_or_denial_never_initializes_or_loads() {
        let mut m = Manager::<()>::new();
        m.begin(Stage::Update);
        m.event(m.epoch, Event::UpdateFailed);
        assert_eq!((m.stage, m.notice), (Stage::Idle, NO_CONSENT));
        assert_eq!(next(&mut m), None);

        m.begin(Stage::Update);
        m.event(m.epoch, Event::UpdateOk);
        m.event(m.epoch, Event::Dismissed { error: true });
        assert_eq!((m.stage, m.notice), (Stage::Idle, NO_CONSENT));
        assert_eq!(next(&mut m), None);

        consent(&mut m);
        m.checked(m.epoch, false);
        assert_eq!((m.stage, m.notice), (Stage::Idle, DENIED));
        assert_eq!(next(&mut m), None);
        assert!(!m.init_running && !m.initialized);
    }

    #[test]
    fn stale_callbacks_are_ignored() {
        let mut m = Manager::<()>::new();
        consent(&mut m);
        let old = m.epoch;
        m.begin(Stage::Update);
        m.checked(old, true);
        m.event(old, Event::UpdateFailed);
        assert_eq!((m.stage, m.notice), (Stage::Update, ""));

        // 今の世代でも、段階に合わないイベントは無視する。
        m.event(m.epoch, Event::Loaded);
        m.event(m.epoch, Event::Dismissed { error: false });
        assert_eq!(m.stage, Stage::Update);

        shown(&mut m);
        let old = m.epoch;
        m.cancel(CLOSED);
        m.event(old, Event::Loaded);
        m.event(old, Event::LoadFailed);
        assert_eq!((m.stage, m.notice), (Stage::Idle, CLOSED));
    }

    #[test]
    fn leaving_home_hides_and_requires_new_request() {
        let mut m = Manager::<()>::new();
        consent(&mut m);
        m.checked(m.epoch, true);
        assert_eq!(next(&mut m), Some(Op::Init));
        assert_eq!(next(&mut m), None, "初期化は1回だけ送る");
        m.init_done(true);
        place(&mut m);
        m.event(m.epoch, Event::Loaded);
        assert_eq!(m.stage, Stage::Shown);
        assert!(matches!(m.step(false, SLOT), Step::Cleanup(())));
        assert_eq!((m.stage, m.notice), (Stage::Idle, LEFT));
        assert_eq!(next(&mut m), None);
    }

    #[test]
    fn missing_slot_cancels_ad_stages_only() {
        let mut m = Manager::<()>::new();
        m.begin(Stage::Update);
        assert!(matches!(m.step(true, None), Step::Nothing));
        assert_eq!((m.stage, m.notice), (Stage::Idle, SLOT_MISSING));

        for stage in [Stage::PrivacyUpdate, Stage::PrivacyForm] {
            m.begin(stage);
            assert!(matches!(m.step(true, None), Step::Run(..)));
            assert_eq!(m.stage, stage);
        }
    }

    #[test]
    fn privacy_cancels_on_home_exit_or_break() {
        for stage in [Stage::PrivacyUpdate, Stage::PrivacyForm] {
            let mut m = Manager::<()>::new();
            m.begin(stage);
            assert!(m.active() && !m.ad_active());
            let old = m.epoch;
            assert!(matches!(m.step(true, SLOT), Step::Run(..)));
            assert!(matches!(m.step(false, SLOT), Step::Nothing));
            assert_eq!((m.stage, m.notice), (Stage::Idle, PRIVACY_LEFT));
            assert!(!m.active());

            // 中止後に届いた設定画面の結果では再開しない。
            m.event(old, Event::UpdateOk);
            m.event(old, Event::Dismissed { error: false });
            assert_eq!((m.stage, m.notice), (Stage::Idle, PRIVACY_LEFT));
            assert_eq!(next(&mut m), None);
        }
    }

    #[test]
    fn privacy_form_results() {
        let mut m = Manager::<()>::new();
        m.begin(Stage::PrivacyUpdate);
        assert_eq!(next(&mut m), Some(Op::Update));
        m.event(m.epoch, Event::UpdateOk);
        assert_eq!(next(&mut m), Some(Op::Form { privacy: true }));
        m.event(m.epoch, Event::Dismissed { error: false });
        assert_eq!((m.stage, m.notice), (Stage::Idle, PRIVACY_CLOSED));
        assert_eq!(next(&mut m), None, "閉じた後に広告を読み込まない");

        m.begin(Stage::PrivacyUpdate);
        m.event(m.epoch, Event::UpdateFailed);
        assert_eq!((m.stage, m.notice), (Stage::Idle, PRIVACY_FAILED));

        m.begin(Stage::PrivacyForm);
        m.event(m.epoch, Event::Dismissed { error: true });
        assert_eq!((m.stage, m.notice), (Stage::Idle, PRIVACY_FAILED));
    }

    #[test]
    fn canceled_init_never_revives_but_later_request_continues() {
        let mut m = Manager::<()>::new();
        consent(&mut m);
        m.checked(m.epoch, true);
        assert_eq!(next(&mut m), Some(Op::Init));
        m.cancel(CLOSED);
        m.init_done(true);
        assert_eq!((m.stage, m.notice), (Stage::Idle, CLOSED));
        assert_eq!(next(&mut m), None);

        // 初期化済みなので、次の依頼は初期化を飛ばして読み込む。
        consent(&mut m);
        m.checked(m.epoch, true);
        assert_eq!(m.stage, Stage::Ready);
        assert_eq!(next(&mut m), Some(Op::Load(SLOT.unwrap())));
    }

    #[test]
    fn later_request_waits_for_running_init() {
        let mut m = Manager::<()>::new();
        consent(&mut m);
        m.checked(m.epoch, true);
        assert_eq!(next(&mut m), Some(Op::Init));
        m.cancel(CLOSED);

        consent(&mut m);
        m.checked(m.epoch, true);
        assert_eq!(m.stage, Stage::Init);
        assert_eq!(next(&mut m), None, "実行中の初期化を重ねて送らない");
        m.init_done(true);
        assert_eq!(next(&mut m), Some(Op::Load(SLOT.unwrap())));

        // 失敗した初期化は、取り消し済みの依頼を失敗表示に変えない。
        let mut m = Manager::<()>::new();
        consent(&mut m);
        m.checked(m.epoch, true);
        assert_eq!(next(&mut m), Some(Op::Init));
        m.cancel(CLOSED);
        m.init_done(false);
        assert_eq!((m.stage, m.notice), (Stage::Idle, CLOSED));
        assert!(!m.initialized && !m.init_running);
    }

    #[test]
    fn clipped_slot_cancels_and_moved_slot_relayouts() {
        let mut m = Manager::<()>::new();
        shown(&mut m);
        let moved = [0, 200, 1080, 300];
        assert_eq!(m.step(true, Some(moved)).op(), Some(Op::Layout(moved)));
        assert!(matches!(m.step(true, Some(moved)), Step::Nothing));

        let narrow = [0, 200, AD.0 - 1, 300];
        assert!(matches!(m.step(true, Some(narrow)), Step::Cleanup(())));
        assert_eq!((m.stage, m.notice), (Stage::Idle, TOO_SMALL));

        shown(&mut m);
        let short = [0, 100, 1080, AD.1 - 1];
        assert!(matches!(m.step(true, Some(short)), Step::Cleanup(())));
        assert_eq!((m.stage, m.notice), (Stage::Idle, TOO_SMALL));
        assert_eq!(next(&mut m), None);
    }

    #[test]
    fn no_autonomous_reload() {
        let mut m = Manager::<()>::new();
        shown(&mut m);
        for _ in 0..3 {
            assert_eq!(next(&mut m), None);
        }

        m.cancel("");
        consent(&mut m);
        m.checked(m.epoch, true);
        place(&mut m);
        assert_eq!(next(&mut m), None, "読み込み中に重ねて読み込まない");
        m.event(m.epoch, Event::LoadFailed);
        assert_eq!((m.stage, m.notice), (Stage::Idle, LOAD_FAILED));
        for _ in 0..3 {
            assert_eq!(next(&mut m), None);
        }
    }

    impl Step<()> {
        fn op(self) -> Option<Op> {
            match self {
                Step::Run(op, _) => Some(op),
                _ => None,
            }
        }
    }
}
