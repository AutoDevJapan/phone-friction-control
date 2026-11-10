//! 広告 SDK と同意 SDK のコールバック用インターフェースを実装する JVM クラスを生成する。
//!
//! 各クラスは java/lang/Object を親に 1 つのインターフェースだけを実装し、コールバックは
//! すべて native として Rust ライブラリへ渡す。JVM 側に処理の中身は持たない。

use super::{
    constructor, native, op, Method, Pool, ACC_PUBLIC, ACC_STATIC, ACC_SUPER, INVOKESTATIC, LDC_W,
    LIBRARY, PACKAGE, RETURN,
};
use std::{fs, path::Path};

const OBJECT: &str = "java/lang/Object";
const ADS: &str = "com/google/android/libraries/ads/mobile/sdk/common";
const UMP: &str = "com/google/android/ump";

/// (クラス名, インターフェース, [(メソッド名, 記述子)])
type CallbackSpec = (&'static str, String, Vec<(&'static str, String)>);

fn specs() -> [CallbackSpec; 4] {
    [
        (
            "AdLoadListener",
            format!("{ADS}/AdLoadCallback"),
            vec![
                // AdLoadCallback<AdT> の onAdLoaded(AdT) は消去後 Object になる。
                ("onAdLoaded", "(Ljava/lang/Object;)V".into()),
                ("onAdFailedToLoad", format!("(L{ADS}/LoadAdError;)V")),
            ],
        ),
        (
            "ConsentUpdateSuccess",
            format!("{UMP}/ConsentInformation$OnConsentInfoUpdateSuccessListener"),
            vec![("onConsentInfoUpdateSuccess", "()V".into())],
        ),
        (
            "ConsentUpdateFailure",
            format!("{UMP}/ConsentInformation$OnConsentInfoUpdateFailureListener"),
            vec![(
                "onConsentInfoUpdateFailure",
                format!("(L{UMP}/FormError;)V"),
            )],
        ),
        (
            "ConsentDismissed",
            format!("{UMP}/ConsentForm$OnConsentFormDismissedListener"),
            vec![("onConsentFormDismissed", format!("(L{UMP}/FormError;)V"))],
        ),
    ]
}

fn callback_class(simple: &str, interface: &str, callbacks: &[(&str, String)]) -> Vec<u8> {
    generate_class(simple, OBJECT, Some(interface), callbacks)
}

fn generate_class(
    simple: &str,
    parent: &str,
    interface: Option<&str>,
    callbacks: &[(&str, String)],
) -> Vec<u8> {
    let mut pool = Pool::default();
    let this_index = pool.class(&format!("{PACKAGE}/{simple}"));
    let super_index = pool.class(parent);
    let interface_index = interface.map(|name| pool.class(name));
    let mut methods = vec![constructor(&mut pool, parent)];

    let lib = pool.string(LIBRARY);
    let load = pool.method_ref("java/lang/System", "loadLibrary", "(Ljava/lang/String;)V");
    let mut clinit = op(LDC_W, lib).to_vec();
    clinit.extend(op(INVOKESTATIC, load));
    clinit.push(RETURN);
    methods.push(Method {
        access: ACC_STATIC,
        name: pool.utf8("<clinit>"),
        desc: pool.utf8("()V"),
        code: Some((1, 0, clinit)),
    });
    for (name, desc) in callbacks {
        methods.push(native(&mut pool, ACC_PUBLIC, name, desc));
    }
    if simple == "ControlAdsActivity" {
        let callback = pool.method_ref(&format!("{PACKAGE}/{simple}"), "onHostPause", "()V");
        let pause = pool.method_ref(parent, "onPause", "()V");
        let mut code = vec![0x2a]; // aload_0
        code.extend(op(0xb6, callback)); // invokevirtual
        code.push(0x2a);
        code.extend(op(0xb7, pause)); // invokespecial super.onPause
        code.push(RETURN);
        methods.push(Method {
            access: 0x0004,
            name: pool.utf8("onPause"),
            desc: pool.utf8("()V"),
            code: Some((1, 1, code)),
        });
    }
    if simple == "ControlAdsActivity" {
        // winit cannot create a second event loop in the same Android process.
        // Back backgrounds the task instead of destroying its NativeActivity.
        let move_back = pool.method_ref("android/app/Activity", "moveTaskToBack", "(Z)Z");
        let mut code = vec![0x2a, 0x04]; // this, true
        code.extend(op(0xb6, move_back));
        code.extend([0x57, RETURN]); // discard boolean result
        methods.push(Method {
            access: ACC_PUBLIC,
            name: pool.utf8("onBackPressed"),
            desc: pool.utf8("()V"),
            code: Some((2, 1, code)),
        });
    }
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
    out.extend(u16::from(interface_index.is_some()).to_be_bytes());
    if let Some(interface_index) = interface_index {
        out.extend(interface_index.to_be_bytes());
    }
    out.extend(0u16.to_be_bytes()); // fields
    out.extend((methods.len() as u16).to_be_bytes());
    for m in &methods {
        out.extend(m.access.to_be_bytes());
        out.extend(m.name.to_be_bytes());
        out.extend(m.desc.to_be_bytes());
        match &m.code {
            None => out.extend(0u16.to_be_bytes()),
            Some((max_stack, max_locals, code)) => {
                out.extend(1u16.to_be_bytes());
                out.extend(code_name.to_be_bytes());
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

/// (クラス名, クラスファイル) の一覧。
pub(super) fn classes() -> Vec<(&'static str, Vec<u8>)> {
    specs()
        .iter()
        .map(|(simple, interface, callbacks)| {
            (*simple, callback_class(simple, interface, callbacks))
        })
        .collect()
}

pub(super) fn write_classes(out_dir: &Path) -> std::io::Result<()> {
    let dir = out_dir.join(PACKAGE);
    fs::create_dir_all(&dir)?;
    for (simple, bytes) in classes() {
        fs::write(dir.join(format!("{simple}.class")), bytes)?;
    }
    fs::write(
        dir.join("ControlHomeActivity.class"),
        super::home_activity(&format!("{PACKAGE}/ControlAdsActivity")),
    )?;
    fs::write(
        dir.join("ControlAdsActivity.class"),
        generate_class(
            "ControlAdsActivity",
            "android/app/NativeActivity",
            None,
            &[("onHostPause", "()V".into())],
        ),
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    //! 生成器と独立に、ヘッダー・インターフェース・メソッドの宣言だけを読み取って確かめる。
    use super::*;

    struct Reader<'a>(&'a [u8]);
    impl<'a> Reader<'a> {
        fn take(&mut self, n: usize) -> &'a [u8] {
            assert!(self.0.len() >= n, "class file truncated");
            let (a, b) = self.0.split_at(n);
            self.0 = b;
            a
        }
        fn u2(&mut self) -> u16 {
            u16::from_be_bytes(self.take(2).try_into().unwrap())
        }
        fn u4(&mut self) -> u32 {
            u32::from_be_bytes(self.take(4).try_into().unwrap())
        }
    }

    struct Parsed {
        access: u16,
        this: String,
        super_name: String,
        interfaces: Vec<String>,
        fields: u16,
        /// (access, name, desc, Code 属性の有無)
        methods: Vec<(u16, String, String, bool)>,
    }

    fn parse(bytes: &[u8]) -> Parsed {
        let mut r = Reader(bytes);
        assert_eq!(r.u4(), 0xCAFEBABE);
        assert_eq!((r.u2(), r.u2()), (0, 49));
        // 番号 -> (タグ, 文字列 または 参照先)
        let mut cp: Vec<(u8, String, u16)> = vec![(0, String::new(), 0)];
        for _ in 1..r.u2() {
            let tag = r.take(1)[0];
            cp.push(match tag {
                1 => {
                    let len = r.u2() as usize;
                    (1, String::from_utf8(r.take(len).to_vec()).unwrap(), 0)
                }
                7 | 8 => (tag, String::new(), r.u2()),
                10 | 12 => {
                    let first = r.u2();
                    r.u2();
                    (tag, String::new(), first)
                }
                t => panic!("unexpected constant tag {t}"),
            });
        }
        let utf8 = |i: u16| {
            let (tag, s, _) = &cp[i as usize];
            assert_eq!(*tag, 1, "#{i} is not Utf8");
            s.clone()
        };
        let class = |i: u16| {
            let (tag, _, n) = &cp[i as usize];
            assert_eq!(*tag, 7, "#{i} is not Class");
            utf8(*n)
        };
        let access = r.u2();
        let this = class(r.u2());
        let super_name = class(r.u2());
        let interfaces = (0..r.u2()).map(|_| class(r.u2())).collect();
        let fields = r.u2();
        let methods = (0..r.u2())
            .map(|_| {
                let (access, name, desc) = (r.u2(), utf8(r.u2()), utf8(r.u2()));
                let attrs = r.u2();
                for _ in 0..attrs {
                    assert_eq!(utf8(r.u2()), "Code");
                    let len = r.u4() as usize;
                    r.take(len);
                }
                (access, name, desc, attrs > 0)
            })
            .collect();
        assert_eq!(r.u2(), 0, "class attributes");
        assert!(r.0.is_empty(), "trailing bytes after class file");
        Parsed {
            access,
            this,
            super_name,
            interfaces,
            fields,
            methods,
        }
    }

    #[test]
    fn each_class_implements_one_interface_with_native_callbacks() {
        let expected = [
            (
                "AdLoadListener",
                "com/google/android/libraries/ads/mobile/sdk/common/AdLoadCallback",
                &[
                    ("onAdLoaded", "(Ljava/lang/Object;)V"),
                    (
                        "onAdFailedToLoad",
                        "(Lcom/google/android/libraries/ads/mobile/sdk/common/LoadAdError;)V",
                    ),
                ][..],
            ),
            (
                "ConsentUpdateSuccess",
                "com/google/android/ump/ConsentInformation$OnConsentInfoUpdateSuccessListener",
                &[("onConsentInfoUpdateSuccess", "()V")][..],
            ),
            (
                "ConsentUpdateFailure",
                "com/google/android/ump/ConsentInformation$OnConsentInfoUpdateFailureListener",
                &[(
                    "onConsentInfoUpdateFailure",
                    "(Lcom/google/android/ump/FormError;)V",
                )][..],
            ),
            (
                "ConsentDismissed",
                "com/google/android/ump/ConsentForm$OnConsentFormDismissedListener",
                &[(
                    "onConsentFormDismissed",
                    "(Lcom/google/android/ump/FormError;)V",
                )][..],
            ),
        ];
        let generated = classes();
        assert_eq!(generated.len(), expected.len());
        for ((simple, bytes), (want, interface, callbacks)) in generated.iter().zip(expected) {
            assert_eq!(*simple, want);
            let c = parse(bytes);
            assert_eq!(c.access, 0x0021);
            assert_eq!(
                c.this,
                format!("com/autodevjapan/phonefrictioncontrol/{want}")
            );
            assert_eq!(c.super_name, "java/lang/Object");
            assert_eq!(c.interfaces, [interface]);
            assert_eq!(c.fields, 0);
            let mut want_methods = vec![
                (0x0001, "<init>".to_string(), "()V".to_string(), true),
                (0x0008, "<clinit>".to_string(), "()V".to_string(), true),
            ];
            for (name, desc) in callbacks {
                want_methods.push((0x0101, name.to_string(), desc.to_string(), false));
            }
            assert_eq!(c.methods, want_methods, "{want}");
        }
    }
    #[test]
    fn back_keeps_the_native_event_loop_activity_alive() {
        let bytes = generate_class(
            "ControlAdsActivity",
            "android/app/NativeActivity",
            None,
            &[("onHostPause", "()V".into())],
        );
        let c = parse(&bytes);
        assert!(c
            .methods
            .iter()
            .any(|(_, name, desc, code)| name == "onBackPressed" && desc == "()V" && *code));
    }
}
