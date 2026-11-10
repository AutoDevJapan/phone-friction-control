//! Android のサービス入口となる JVM クラスを、Java/Kotlin のソースなしで生成する。
//!
//! 使い方: `android-bridge <出力ディレクトリ>`
//! 出力: `<出力ディレクトリ>/com/autodevjapan/phonefrictioncontrol/{ControlService,MediaListener}.class`
//!
//! クラスファイルは版 49 (Java 5) で書くため StackMapTable は不要。処理の中身は持たず、
//! ControlService のコールバックは親の Rust ライブラリが JNI で公開する native 関数へ渡す。

use std::{env, fs, path::Path, process::ExitCode};
#[path = "../../../src/ad_config.rs"]
mod ad_config;
mod ads_callbacks;
mod ads_package;
mod native_features;
mod package;

const PACKAGE: &str = "com/autodevjapan/phonefrictioncontrol";
const LIBRARY: &str = "phone_friction_control";
const ACCESSIBILITY_SERVICE: &str = "android/accessibilityservice/AccessibilityService";
const NOTIFICATION_LISTENER: &str = "android/service/notification/NotificationListenerService";

const ACC_PUBLIC: u16 = 0x0001;
const ACC_PROTECTED: u16 = 0x0004;
const ACC_STATIC: u16 = 0x0008;
const ACC_SUPER: u16 = 0x0020;
const ACC_NATIVE: u16 = 0x0100;

const ALOAD_0: u8 = 0x2a;
const LDC_W: u8 = 0x13;
const INVOKEVIRTUAL: u8 = 0xb6;
const INVOKESPECIAL: u8 = 0xb7;
const INVOKESTATIC: u8 = 0xb8;
const RETURN: u8 = 0xb1;

#[derive(Default)]
struct Pool {
    entries: Vec<Vec<u8>>,
}

impl Pool {
    /// 同じ内容の項目は再利用し、1 始まりの番号を返す。
    fn add(&mut self, entry: Vec<u8>) -> u16 {
        if let Some(i) = self.entries.iter().position(|e| *e == entry) {
            return i as u16 + 1;
        }
        self.entries.push(entry);
        self.entries.len() as u16
    }

    fn utf8(&mut self, s: &str) -> u16 {
        // 名前はすべて ASCII なので、modified UTF-8 と通常の UTF-8 が一致する。
        assert!(s.is_ascii() && !s.contains('\0'));
        let mut e = vec![1];
        e.extend((s.len() as u16).to_be_bytes());
        e.extend(s.as_bytes());
        self.add(e)
    }

    fn tagged(&mut self, tag: u8, a: u16, b: Option<u16>) -> u16 {
        let mut e = vec![tag];
        e.extend(a.to_be_bytes());
        if let Some(b) = b {
            e.extend(b.to_be_bytes());
        }
        self.add(e)
    }

    fn class(&mut self, name: &str) -> u16 {
        let n = self.utf8(name);
        self.tagged(7, n, None)
    }

    fn string(&mut self, s: &str) -> u16 {
        let n = self.utf8(s);
        self.tagged(8, n, None)
    }

    fn method_ref(&mut self, class: &str, name: &str, desc: &str) -> u16 {
        let c = self.class(class);
        let n = self.utf8(name);
        let d = self.utf8(desc);
        let nat = self.tagged(12, n, Some(d));
        self.tagged(10, c, Some(nat))
    }
}

struct Method {
    access: u16,
    name: u16,
    desc: u16,
    /// (max_stack, max_locals, bytecode)。native メソッドは None。
    code: Option<(u16, u16, Vec<u8>)>,
}

fn op(code: u8, index: u16) -> [u8; 3] {
    let [hi, lo] = index.to_be_bytes();
    [code, hi, lo]
}

fn native(pool: &mut Pool, access: u16, name: &str, desc: &str) -> Method {
    Method {
        access: access | ACC_NATIVE,
        name: pool.utf8(name),
        desc: pool.utf8(desc),
        code: None,
    }
}

fn constructor(pool: &mut Pool, super_name: &str) -> Method {
    let init = pool.method_ref(super_name, "<init>", "()V");
    let mut code = vec![ALOAD_0];
    code.extend(op(INVOKESPECIAL, init));
    code.push(RETURN);
    Method {
        access: ACC_PUBLIC,
        name: pool.utf8("<init>"),
        desc: pool.utf8("()V"),
        code: Some((1, 1, code)),
    }
}

fn class_file(
    this: &str,
    super_name: &str,
    methods: impl FnOnce(&mut Pool) -> Vec<Method>,
) -> Vec<u8> {
    let mut pool = Pool::default();
    let this_index = pool.class(this);
    let super_index = pool.class(super_name);
    let mut all = vec![constructor(&mut pool, super_name)];
    all.extend(methods(&mut pool));
    let code_name = pool.utf8("Code");

    let mut out = Vec::new();
    out.extend(0xCAFE_BABEu32.to_be_bytes());
    out.extend(0u16.to_be_bytes()); // minor
    out.extend(49u16.to_be_bytes()); // major
    out.extend((pool.entries.len() as u16 + 1).to_be_bytes());
    for e in &pool.entries {
        out.extend(e);
    }
    out.extend((ACC_PUBLIC | ACC_SUPER).to_be_bytes());
    out.extend(this_index.to_be_bytes());
    out.extend(super_index.to_be_bytes());
    out.extend(0u16.to_be_bytes()); // interfaces
    out.extend(0u16.to_be_bytes()); // fields
    out.extend((all.len() as u16).to_be_bytes());
    for m in &all {
        out.extend(m.access.to_be_bytes());
        out.extend(m.name.to_be_bytes());
        out.extend(m.desc.to_be_bytes());
        match &m.code {
            None => out.extend(0u16.to_be_bytes()),
            Some((max_stack, max_locals, code)) => {
                out.extend(1u16.to_be_bytes());
                out.extend(code_name.to_be_bytes());
                // max_stack + max_locals + code_length + code + 例外表の数 + 属性の数
                out.extend((2 + 2 + 4 + code.len() as u32 + 2 + 2).to_be_bytes());
                out.extend(max_stack.to_be_bytes());
                out.extend(max_locals.to_be_bytes());
                out.extend((code.len() as u32).to_be_bytes());
                out.extend(code);
                out.extend(0u16.to_be_bytes());
                out.extend(0u16.to_be_bytes());
            }
        }
    }
    out.extend(0u16.to_be_bytes()); // class attributes
    out
}

fn control_service() -> Vec<u8> {
    let this = format!("{PACKAGE}/ControlService");
    class_file(&this, ACCESSIBILITY_SERVICE, |pool| {
        let lib = pool.string(LIBRARY);
        let load = pool.method_ref("java/lang/System", "loadLibrary", "(Ljava/lang/String;)V");
        let mut clinit = op(LDC_W, lib).to_vec();
        clinit.extend(op(INVOKESTATIC, load));
        clinit.push(RETURN);

        let disconnected = pool.method_ref(&this, "disconnected", "()V");
        let super_destroy = pool.method_ref(ACCESSIBILITY_SERVICE, "onDestroy", "()V");
        let mut destroy = vec![ALOAD_0];
        destroy.extend(op(INVOKEVIRTUAL, disconnected));
        destroy.push(ALOAD_0);
        destroy.extend(op(INVOKESPECIAL, super_destroy));
        destroy.push(RETURN);

        vec![
            Method {
                access: ACC_STATIC,
                name: pool.utf8("<clinit>"),
                desc: pool.utf8("()V"),
                code: Some((1, 0, clinit)),
            },
            native(pool, ACC_PROTECTED, "onServiceConnected", "()V"),
            native(
                pool,
                ACC_PUBLIC,
                "onAccessibilityEvent",
                "(Landroid/view/accessibility/AccessibilityEvent;)V",
            ),
            native(pool, ACC_PUBLIC, "onInterrupt", "()V"),
            native(pool, ACC_PROTECTED, "disconnected", "()V"),
            Method {
                access: ACC_PUBLIC,
                name: pool.utf8("onDestroy"),
                desc: pool.utf8("()V"),
                code: Some((1, 1, destroy)),
            },
        ]
    })
}

fn media_listener() -> Vec<u8> {
    class_file(
        &format!("{PACKAGE}/MediaListener"),
        NOTIFICATION_LISTENER,
        |_| Vec::new(),
    )
}

fn control_activity() -> Vec<u8> {
    class_file(
        &format!("{PACKAGE}/ControlActivity"),
        "android/app/NativeActivity",
        |pool| {
            let move_back = pool.method_ref("android/app/Activity", "moveTaskToBack", "(Z)Z");
            let mut code = vec![ALOAD_0, 0x04];
            code.extend(op(INVOKEVIRTUAL, move_back));
            code.extend([0x57, RETURN]);
            vec![Method {
                access: ACC_PUBLIC,
                name: pool.utf8("onBackPressed"),
                desc: pool.utf8("()V"),
                code: Some((2, 1, code)),
            }]
        },
    )
}
fn write_classes(out_dir: &Path) -> std::io::Result<()> {
    let dir = out_dir.join(PACKAGE);
    fs::create_dir_all(&dir)?;
    fs::write(dir.join("ControlActivity.class"), control_activity())?;
    fs::write(dir.join("ControlService.class"), control_service())?;
    fs::write(dir.join("MediaListener.class"), media_listener())
}

fn main() -> ExitCode {
    let args: Vec<_> = env::args_os().skip(1).collect();
    if args
        .first()
        .is_some_and(|arg| arg == "pack-ads" || arg == "pack-ads-release")
    {
        return match ads_package::assemble(&args[1..], args[0] == "pack-ads-release") {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("android-bridge: {error}");
                ExitCode::FAILURE
            }
        };
    }
    if args.first().is_some_and(|arg| arg == "ads-classes") {
        return match args
            .get(1)
            .ok_or_else(|| std::io::Error::other("ads-classes <output-directory>"))
            .and_then(|out| ads_callbacks::write_classes(Path::new(out)))
        {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("android-bridge: {error}");
                ExitCode::FAILURE
            }
        };
    }
    if args
        .first()
        .is_some_and(|arg| arg == "pack" || arg == "pack-release")
    {
        return match package::assemble(&args[1..], args[0] == "pack-release") {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("android-bridge: {error}");
                ExitCode::FAILURE
            }
        };
    }
    let Some(out_dir) = env::args_os().nth(1) else {
        eprintln!("usage: android-bridge <output-dir>");
        return ExitCode::from(2);
    };
    match write_classes(Path::new(&out_dir)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("android-bridge: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    //! 生成器とは独立したクラスファイルの読み取り器で、JVM 仕様どおりに解釈できるかを確かめる。
    use super::*;

    #[derive(Debug, Clone)]
    enum Cp {
        Utf8(String),
        Class(u16),
        Str(u16),
        NameAndType(u16, u16),
        Methodref(u16, u16),
    }

    struct ParsedMethod {
        access: u16,
        name: String,
        desc: String,
        code: Option<(u16, u16, Vec<u8>)>,
    }

    struct Parsed {
        magic: u32,
        minor: u16,
        major: u16,
        cp: Vec<Option<Cp>>, // 0 番は未使用
        access: u16,
        this: String,
        super_name: String,
        interfaces: u16,
        fields: u16,
        methods: Vec<ParsedMethod>,
    }

    struct Reader<'a>(&'a [u8]);
    impl<'a> Reader<'a> {
        fn take(&mut self, n: usize) -> &'a [u8] {
            assert!(self.0.len() >= n, "class file truncated");
            let (a, b) = self.0.split_at(n);
            self.0 = b;
            a
        }
        fn u1(&mut self) -> u8 {
            self.take(1)[0]
        }
        fn u2(&mut self) -> u16 {
            u16::from_be_bytes(self.take(2).try_into().unwrap())
        }
        fn u4(&mut self) -> u32 {
            u32::from_be_bytes(self.take(4).try_into().unwrap())
        }
    }

    impl Parsed {
        fn utf8(&self, i: u16) -> String {
            match self.cp.get(i as usize) {
                Some(Some(Cp::Utf8(s))) => s.clone(),
                other => panic!("#{i} is not Utf8: {other:?}"),
            }
        }
        fn class_name(&self, i: u16) -> String {
            match self.cp.get(i as usize) {
                Some(Some(Cp::Class(n))) => self.utf8(*n),
                other => panic!("#{i} is not Class: {other:?}"),
            }
        }
        fn string(&self, i: u16) -> String {
            match self.cp.get(i as usize) {
                Some(Some(Cp::Str(n))) => self.utf8(*n),
                other => panic!("#{i} is not String: {other:?}"),
            }
        }
        fn method_ref(&self, i: u16) -> (String, String, String) {
            let Some(Some(Cp::Methodref(c, nat))) = self.cp.get(i as usize) else {
                panic!("#{i} is not Methodref");
            };
            let Some(Some(Cp::NameAndType(n, d))) = self.cp.get(*nat as usize) else {
                panic!("#{nat} is not NameAndType");
            };
            (self.class_name(*c), self.utf8(*n), self.utf8(*d))
        }
        fn method(&self, name: &str) -> &ParsedMethod {
            self.methods
                .iter()
                .find(|m| m.name == name)
                .unwrap_or_else(|| panic!("no method {name}"))
        }
        /// 使う命令だけを解釈し、参照先を名前に解決した命令列を返す。
        fn decode(&self, code: &[u8]) -> Vec<String> {
            let mut r = Reader(code);
            let mut out = Vec::new();
            while !r.0.is_empty() {
                let ins = match r.u1() {
                    0x2a => "aload_0".to_string(),
                    0x04 => "iconst_1".to_string(),
                    0x57 => "pop".to_string(),
                    0xb1 => "return".to_string(),
                    0x13 => format!("ldc_w {:?}", self.string(r.u2())),
                    o @ 0xb6..=0xb8 => {
                        let name = match o {
                            0xb6 => "invokevirtual",
                            0xb7 => "invokespecial",
                            _ => "invokestatic",
                        };
                        let (c, n, d) = self.method_ref(r.u2());
                        format!("{name} {c}.{n}{d}")
                    }
                    o => panic!("unexpected opcode {o:#x}"),
                };
                out.push(ins);
            }
            out
        }
    }

    fn parse(bytes: &[u8]) -> Parsed {
        let mut r = Reader(bytes);
        let magic = r.u4();
        let minor = r.u2();
        let major = r.u2();
        let count = r.u2();
        let mut cp = vec![None];
        for _ in 1..count {
            cp.push(Some(match r.u1() {
                1 => {
                    let len = r.u2() as usize;
                    Cp::Utf8(String::from_utf8(r.take(len).to_vec()).unwrap())
                }
                7 => Cp::Class(r.u2()),
                8 => Cp::Str(r.u2()),
                12 => Cp::NameAndType(r.u2(), r.u2()),
                10 => Cp::Methodref(r.u2(), r.u2()),
                t => panic!("unexpected constant tag {t}"),
            }));
        }
        let access = r.u2();
        let this = r.u2();
        let super_index = r.u2();
        let interfaces = r.u2();
        let fields = r.u2();
        let mut partial = Parsed {
            magic,
            minor,
            major,
            cp,
            access,
            this: String::new(),
            super_name: String::new(),
            interfaces,
            fields,
            methods: Vec::new(),
        };
        partial.this = partial.class_name(this);
        partial.super_name = partial.class_name(super_index);
        for _ in 0..r.u2() {
            let access = r.u2();
            let name = partial.utf8(r.u2());
            let desc = partial.utf8(r.u2());
            let mut code = None;
            for _ in 0..r.u2() {
                let attr_name = partial.utf8(r.u2());
                let len = r.u4() as usize;
                let mut a = Reader(r.take(len));
                assert_eq!(attr_name, "Code");
                let max_stack = a.u2();
                let max_locals = a.u2();
                let code_len = a.u4() as usize;
                let bytes = a.take(code_len).to_vec();
                assert_eq!(a.u2(), 0, "exception table");
                assert_eq!(a.u2(), 0, "code attributes");
                assert!(a.0.is_empty(), "Code attribute_length too long");
                code = Some((max_stack, max_locals, bytes));
            }
            partial.methods.push(ParsedMethod {
                access,
                name,
                desc,
                code,
            });
        }
        assert_eq!(r.u2(), 0, "class attributes");
        assert!(r.0.is_empty(), "trailing bytes after class file");
        partial
    }

    #[test]
    fn plain_manifest_dex_and_service_target_agree() {
        let manifest = include_str!("../../../android/AndroidManifest.xml");
        assert!(manifest
            .contains("android:name=\"com.autodevjapan.phonefrictioncontrol.ControlActivity\""));
        assert!(manifest.contains("android:enableOnBackInvokedCallback=\"false\""));
        assert!(manifest.contains("android:launchMode=\"singleInstance\""));
        assert!(manifest.contains("keyboardHidden|uiMode"));
        assert!(include_str!("package.rs").contains("activity.as_os_str()"));
        assert!(include_str!("../../../src/service.rs").contains(
            "let activity_class = \"com.autodevjapan.phonefrictioncontrol.ControlActivity\";"
        ));
    }
    #[test]
    fn generated_plain_activity_handles_back() {
        let dir = env::temp_dir().join(format!("plain-activity-test-{}", std::process::id()));
        write_classes(&dir).unwrap();
        let bytes = fs::read(dir.join(PACKAGE).join("ControlActivity.class")).unwrap();
        let c = parse(&bytes);
        assert_eq!(c.super_name, "android/app/NativeActivity");
        let m = c.method("onBackPressed");
        let (stack, locals, code) = m.code.as_ref().unwrap();
        assert_eq!((*stack, *locals), (2, 1));
        assert_eq!(
            c.decode(code),
            [
                "aload_0",
                "iconst_1",
                "invokevirtual android/app/Activity.moveTaskToBack(Z)Z",
                "pop",
                "return"
            ]
        );
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn headers_use_version_49_and_public_super_flags() {
        for (bytes, this, sup) in [
            (
                control_service(),
                "com/autodevjapan/phonefrictioncontrol/ControlService",
                ACCESSIBILITY_SERVICE,
            ),
            (
                media_listener(),
                "com/autodevjapan/phonefrictioncontrol/MediaListener",
                NOTIFICATION_LISTENER,
            ),
        ] {
            let c = parse(&bytes);
            assert_eq!(c.magic, 0xCAFEBABE);
            assert_eq!((c.major, c.minor), (49, 0));
            assert_eq!(c.access, 0x0021);
            assert_eq!(c.this, this);
            assert_eq!(c.super_name, sup);
            assert_eq!((c.interfaces, c.fields), (0, 0));
        }
    }

    #[test]
    fn constant_pool_has_no_duplicates_and_all_references_resolve() {
        for bytes in [control_service(), media_listener()] {
            let c = parse(&bytes);
            let mut seen = Vec::new();
            for (i, e) in c.cp.iter().enumerate().skip(1) {
                let key = format!("{:?}", e.as_ref().unwrap());
                assert!(!seen.contains(&key), "duplicate constant {key}");
                seen.push(key);
                match e.as_ref().unwrap() {
                    Cp::Class(_) => drop(c.class_name(i as u16)),
                    Cp::Str(_) => drop(c.string(i as u16)),
                    Cp::Methodref(..) => drop(c.method_ref(i as u16)),
                    Cp::NameAndType(n, d) => drop((c.utf8(*n), c.utf8(*d))),
                    Cp::Utf8(_) => {}
                }
            }
        }
    }

    #[test]
    fn constructors_are_public_and_call_the_superclass_constructor() {
        for (bytes, sup) in [
            (control_service(), ACCESSIBILITY_SERVICE),
            (media_listener(), NOTIFICATION_LISTENER),
        ] {
            let c = parse(&bytes);
            let init = c.method("<init>");
            assert_eq!(init.access, ACC_PUBLIC);
            assert_eq!(init.desc, "()V");
            let (max_stack, max_locals, code) = init.code.as_ref().unwrap();
            // this を 1 つ積むだけで、引数は this のみ。
            assert_eq!((*max_stack, *max_locals), (1, 1));
            assert_eq!(
                c.decode(code),
                [
                    "aload_0",
                    format!("invokespecial {sup}.<init>()V").as_str(),
                    "return"
                ]
            );
        }
    }

    #[test]
    fn control_service_callbacks_are_native_without_code() {
        let c = parse(&control_service());
        for (name, access, desc) in [
            ("onServiceConnected", ACC_PROTECTED, "()V"),
            (
                "onAccessibilityEvent",
                ACC_PUBLIC,
                "(Landroid/view/accessibility/AccessibilityEvent;)V",
            ),
            ("onInterrupt", ACC_PUBLIC, "()V"),
            ("disconnected", ACC_PROTECTED, "()V"),
        ] {
            let m = c.method(name);
            assert_eq!(m.access, access | ACC_NATIVE, "{name}");
            assert_eq!(m.desc, desc, "{name}");
            assert!(m.code.is_none(), "{name} must not have Code");
        }
    }

    #[test]
    fn control_service_loads_the_rust_library_in_static_initializer() {
        let c = parse(&control_service());
        let m = c.method("<clinit>");
        assert_eq!((m.access, m.desc.as_str()), (ACC_STATIC, "()V"));
        let (max_stack, max_locals, code) = m.code.as_ref().unwrap();
        assert_eq!((*max_stack, *max_locals), (1, 0));
        assert_eq!(
            c.decode(code),
            [
                "ldc_w \"phone_friction_control\"",
                "invokestatic java/lang/System.loadLibrary(Ljava/lang/String;)V",
                "return",
            ]
        );
    }

    #[test]
    fn on_destroy_notifies_rust_then_calls_super() {
        let c = parse(&control_service());
        let m = c.method("onDestroy");
        assert_eq!((m.access, m.desc.as_str()), (ACC_PUBLIC, "()V"));
        let (max_stack, max_locals, code) = m.code.as_ref().unwrap();
        assert_eq!((*max_stack, *max_locals), (1, 1));
        assert_eq!(
            c.decode(code),
            [
                "aload_0",
                "invokevirtual com/autodevjapan/phonefrictioncontrol/ControlService.disconnected()V",
                "aload_0",
                format!("invokespecial {ACCESSIBILITY_SERVICE}.onDestroy()V").as_str(),
                "return",
            ]
        );
    }

    #[test]
    fn media_listener_only_has_a_constructor() {
        let c = parse(&media_listener());
        let names: Vec<_> = c.methods.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(names, ["<init>"]);
        assert!(!c
            .cp
            .iter()
            .flatten()
            .any(|e| matches!(e, Cp::Utf8(s) if s.contains("loadLibrary"))));
    }

    #[test]
    fn writes_both_classes_under_the_package_directory() {
        let dir = env::temp_dir().join(format!("android-bridge-test-{}", std::process::id()));
        write_classes(&dir).unwrap();
        let pkg = dir.join(PACKAGE);
        assert_eq!(
            parse(&fs::read(pkg.join("ControlService.class")).unwrap()).this,
            format!("{PACKAGE}/ControlService")
        );
        assert_eq!(
            parse(&fs::read(pkg.join("MediaListener.class")).unwrap()).this,
            format!("{PACKAGE}/MediaListener")
        );
        fs::remove_dir_all(&dir).unwrap();
    }
}
