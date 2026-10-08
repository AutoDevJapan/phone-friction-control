//! 試験用広告入りの debug APK を Gradle で組み立てる (`pack-ads`)。
//!
//! 生成するのは使い捨ての Gradle 設定だけで、処理はすべて Rust 側にある。依存 AAR の
//! 取り込み・リソース・マニフェストの統合は Gradle (AGP) に任せる。通常の `pack` は変えない。
//! 出力は手元確認用の debug APK で、配布物ではない。広告の同意と表示の制御は別モジュールで行う。

use std::{
    env,
    ffi::OsString,
    fmt::Write as _,
    fs, io,
    path::{Path, PathBuf},
    process::Command,
};

const USAGE: &str = "pack-ads <project-root> <native-library.so> <abi> <new-output-directory> <gradle-executable>; ANDROID_HOME and JAVA_HOME required, PHONE_CONTROL_GRADLE_CACHE optional";
const PROJECT_NAME: &str = "phone-control-ads";
const NDK: &str = "ndk/30.0.14904198/toolchains/llvm/prebuilt";
const BUILD_TOOLS: &str = "build-tools/36.1.0";
/// `ads-test` 機能で組んだライブラリだけが持つ JNI 関数。
const ADS_SYMBOL: &str = "Java_com_autodevjapan_phonefrictioncontrol_AdLoadListener_onAdLoaded";
/// Google 公式のサンプル用アプリ ID (試験広告専用、秘密情報ではない)。
const SAMPLE_APP_ID: &str = "ca-app-pub-3940256099942544~3347511713";

const SETTINGS: &str = "pluginManagement {
    repositories { google(); mavenCentral(); gradlePluginPortal() }
}
dependencyResolutionManagement {
    repositoriesMode.set(RepositoriesMode.FAIL_ON_PROJECT_REPOS)
    repositories { google(); mavenCentral() }
}
rootProject.name = 'phone-control-ads'
";

const PROPERTIES: &str = "org.gradle.daemon=false
org.gradle.jvmargs=-Xmx2g
android.useAndroidX=true
";

fn other(message: impl Into<String>) -> io::Error {
    io::Error::other(message.into())
}

fn command(program: &Path) -> Command {
    #[cfg_attr(not(windows), allow(unused_mut))]
    let mut command = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }
    command
}

fn run(mut command: Command) -> io::Result<()> {
    let status = command.status()?;
    if status.success() {
        Ok(())
    } else {
        Err(other(format!(
            "{} failed: {status}",
            Path::new(command.get_program()).display()
        )))
    }
}

fn exe(dir: &Path, name: &str) -> PathBuf {
    dir.join(if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_owned()
    })
}

fn env_path(name: &str) -> io::Result<PathBuf> {
    env::var_os(name)
        .map(PathBuf::from)
        .ok_or_else(|| other(format!("{name} missing")))
}

/// `needle` がちょうど 1 回現れることを確かめてから置き換える。想定外の形なら失敗させる。
fn replace_once(text: &mut String, needle: &str, with: &str) -> io::Result<()> {
    if text.matches(needle).count() != 1 {
        return Err(other(format!(
            "unexpected manifest: expected exactly one {needle:?}"
        )));
    }
    *text = text.replacen(needle, with, 1);
    Ok(())
}

/// 元のマニフェスト文字列から広告試験用の版を作る。XML の統合は行わず、決まった位置に足すだけ。
fn ads_manifest(source: &str) -> io::Result<String> {
    let mut m = source.to_owned();
    replace_once(
        &mut m,
        "android:name=\"android.app.NativeActivity\"",
        "android:name=\"com.autodevjapan.phonefrictioncontrol.ControlAdsActivity\"",
    )?;
    // AGP 8 は package 属性と uses-sdk を受け付けないため、build.gradle 側の値に寄せる。
    replace_once(
        &mut m,
        " package=\"com.autodevjapan.phonefrictioncontrol\"",
        "",
    )?;
    replace_once(
        &mut m,
        "<uses-sdk android:minSdkVersion=\"26\" android:targetSdkVersion=\"36\" />",
        "",
    )?;
    if !m.contains("xmlns:tools=\"http://schemas.android.com/tools\"") {
        replace_once(
            &mut m,
            "<manifest",
            "<manifest xmlns:tools=\"http://schemas.android.com/tools\"",
        )?;
    }
    replace_once(
        &mut m,
        "<application",
        "<uses-permission android:name=\"android.permission.INTERNET\" />
    <uses-permission android:name=\"android.permission.ACCESS_NETWORK_STATE\" />
    <uses-permission android:name=\"com.google.android.gms.permission.AD_ID\" tools:node=\"remove\" />
    <uses-permission android:name=\"android.permission.READ_BASIC_PHONE_STATE\" tools:node=\"remove\" />

    <application",
    )?;
    let start = m.find("<application").expect("checked above");
    let end = start
        + m[start..]
            .find('>')
            .ok_or_else(|| other("unterminated <application>"))?
        + 1;
    m.insert_str(
        end,
        &format!("\n        <meta-data android:name=\"com.google.android.gms.ads.APPLICATION_ID\" android:value=\"{SAMPLE_APP_ID}\" />"),
    );
    Ok(m)
}

/// Java の properties 形式へ値を書くための退避。制御文字を含むパスは拒否する。
fn properties_value(path: &Path) -> io::Result<String> {
    let text = path
        .to_str()
        .ok_or_else(|| other("ANDROID_HOME is not valid Unicode"))?;
    let mut out = String::new();
    for c in text.chars() {
        match c {
            '\\' | ':' | '=' | '#' | '!' => {
                out.push('\\');
                out.push(c);
            }
            c if c.is_control() => return Err(other("ANDROID_HOME contains a control character")),
            c if c.is_ascii() => out.push(c),
            c => {
                for unit in c.encode_utf16(&mut [0; 2]) {
                    write!(out, "\\u{unit:04x}").unwrap();
                }
            }
        }
    }
    Ok(out)
}

/// 通常ファイルとディレクトリだけを写す。シンボリックリンクなどは拒否する。
fn copy_tree(from: &Path, to: &Path) -> io::Result<()> {
    if !fs::symlink_metadata(from)?.is_dir() {
        return Err(other(format!("not a directory: {}", from.display())));
    }
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let target = to.join(entry.file_name());
        if kind.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else if kind.is_file() {
            fs::copy(entry.path(), target)?;
        } else {
            return Err(other(format!(
                "not a regular file: {}",
                entry.path().display()
            )));
        }
    }
    Ok(())
}

fn build_gradle(abi: &str) -> String {
    format!(
        "plugins {{ id 'com.android.application' version '8.13.2' }}

android {{
    namespace 'com.autodevjapan.phonefrictioncontrol'
    compileSdk 36
    buildToolsVersion '36.1.0'
    defaultConfig {{
        applicationId 'com.autodevjapan.phonefrictioncontrol'
        minSdk 26
        targetSdk 36
        versionCode 3
        versionName '0.3.0-test-ads'
        ndk {{ abiFilters '{abi}' }}
    }}
    buildTypes {{ debug {{ minifyEnabled false }} }}
    packaging {{ jniLibs {{ useLegacyPackaging false }} }}
}}

dependencies {{
    implementation files('bridge.jar')
    // UMP 4.0 は広告 SDK の依存として入る。
    implementation 'com.google.android.libraries.ads.mobile.sdk:ads-mobile-sdk:1.5.0'
}}
"
    )
}

pub(super) fn assemble(args: &[OsString]) -> io::Result<()> {
    let [root, native, abi, output, gradle] = args else {
        return Err(other(USAGE));
    };
    let root = Path::new(root).canonicalize()?;
    let native = Path::new(native).canonicalize()?;
    if !native.is_file() {
        return Err(other("native library is not a file"));
    }
    let abi = abi
        .to_str()
        .filter(|abi| ["arm64-v8a", "x86_64"].contains(abi))
        .ok_or_else(|| other("unsupported ABI"))?;
    let sdk = env_path("ANDROID_HOME")?;
    let java_home = env_path("JAVA_HOME")?;
    let jdk = java_home.join("bin");
    let manifest = ads_manifest(&fs::read_to_string(
        root.join("android/AndroidManifest.xml"),
    )?)?;

    // 以前の成果物や他人の作業を上書きしない。canonicalize の \\?\ 形式は Gradle に渡さない。
    fs::create_dir(output)?;
    let output = std::path::absolute(output)?;
    let project = output.join("project");
    let main = project.join("src/main");
    copy_tree(&root.join("android/res"), &main.join("res"))?;
    fs::write(main.join("AndroidManifest.xml"), manifest)?;
    let lib_dir = main.join("jniLibs").join(abi);
    fs::create_dir_all(&lib_dir)?;
    let library = lib_dir.join("libphone_friction_control.so");
    fs::copy(&native, &library)?;

    let ndk_bin = sdk
        .join(NDK)
        .join(if cfg!(windows) {
            "windows-x86_64"
        } else {
            "linux-x86_64"
        })
        .join("bin");
    let mut nm = command(&exe(&ndk_bin, "llvm-nm"));
    nm.args(["-D", "--defined-only"]).arg(&library);
    let listing = nm.output()?;
    let exported = String::from_utf8_lossy(&listing.stdout)
        .lines()
        .any(|line| line.split_whitespace().last() == Some(ADS_SYMBOL));
    if !listing.status.success() || !exported {
        return Err(other(format!(
            "{ADS_SYMBOL} not exported; build the library with the ads-test feature"
        )));
    }

    // Strip the copy only; preserve the Cargo artifact for debugging/reuse.
    let mut strip = command(&exe(&ndk_bin, "llvm-strip"));
    strip.arg("--strip-unneeded").arg(&library);
    run(strip)?;

    let classes = output.join("classes");
    super::write_classes(&classes)?;
    super::ads_callbacks::write_classes(&classes)?;
    let mut jar = command(&exe(&jdk, "jar"));
    jar.arg("-cf")
        .arg(project.join("bridge.jar"))
        .arg("-C")
        .arg(&classes)
        .arg(".");
    run(jar)?;

    fs::write(project.join("settings.gradle"), SETTINGS)?;
    fs::write(project.join("gradle.properties"), PROPERTIES)?;
    fs::write(project.join("build.gradle"), build_gradle(abi))?;
    fs::write(
        project.join("local.properties"),
        format!("sdk.dir={}\n", properties_value(&sdk)?),
    )?;

    let mut build = command(Path::new(gradle));
    build.env("JAVA_HOME", &java_home).args([
        "--no-daemon",
        "--console",
        "plain",
        "--max-workers",
        "4",
    ]);
    if let Some(cache) = env::var_os("PHONE_CONTROL_GRADLE_CACHE") {
        build.arg("-g").arg(cache);
    }
    build.arg("-p").arg(&project).arg("assembleDebug");
    run(build)?;

    let built = project.join(format!("build/outputs/apk/debug/{PROJECT_NAME}-debug.apk"));
    let apk = output.join(format!("phone-control-{abi}-test-ads.apk"));
    fs::copy(&built, &apk)?;
    let tools = sdk.join(BUILD_TOOLS);
    let mut align = command(&exe(&tools, "zipalign"));
    align.args(["-c", "-P", "16", "-v", "4"]).arg(&apk);
    run(align)?;
    let mut verify = command(&exe(&jdk, "java"));
    verify
        .arg("-jar")
        .arg(tools.join("lib/apksigner.jar"))
        .args(["verify", "--verbose"])
        .arg(&apk);
    run(verify)?;
    println!("{}", apk.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEFAULT_MANIFEST: &str = include_str!("../../../android/AndroidManifest.xml");

    #[test]
    fn ads_permissions_and_app_id_only_in_the_ads_manifest() {
        assert!(!DEFAULT_MANIFEST.contains("INTERNET"));
        assert!(!DEFAULT_MANIFEST.contains("APPLICATION_ID"));
        let m = ads_manifest(DEFAULT_MANIFEST).unwrap();
        for want in [
            "android.permission.INTERNET",
            "android.permission.ACCESS_NETWORK_STATE",
            "permission.AD_ID\" tools:node=\"remove\"",
            "READ_BASIC_PHONE_STATE\" tools:node=\"remove\"",
            SAMPLE_APP_ID,
        ] {
            assert!(m.contains(want), "{want}");
        }
        assert!(!m.contains("package=") && !m.contains("<uses-sdk"));
        // 権限は application の前、アプリ ID は application の中に置く。
        let app = m.find("<application").unwrap();
        assert!(m.find("permission.INTERNET").unwrap() < app);
        assert!(m.find(SAMPLE_APP_ID).unwrap() > app);
        assert!(m.find(SAMPLE_APP_ID).unwrap() < m.find("</application>").unwrap());
    }

    #[test]
    fn unexpected_manifest_is_rejected() {
        assert!(ads_manifest("<manifest></manifest>").is_err());
        let twice = DEFAULT_MANIFEST.replace("</manifest>", "<application /></manifest>");
        assert!(ads_manifest(&twice).is_err());
    }

    #[test]
    fn sdk_dir_is_escaped_for_properties() {
        assert_eq!(
            properties_value(Path::new(r"C:\Android\Sdk=1#ä")).unwrap(),
            r"C\:\\Android\\Sdk\=1\#\u00e4"
        );
        assert!(properties_value(Path::new("a\nb")).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn copy_tree_rejects_symlinks() {
        let dir = env::temp_dir().join(format!("ads-package-test-{}", std::process::id()));
        let src = dir.join("res/values");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("strings.xml"), "<resources />").unwrap();
        copy_tree(&dir.join("res"), &dir.join("ok")).unwrap();
        assert!(dir.join("ok/values/strings.xml").is_file());
        std::os::unix::fs::symlink("/etc/passwd", src.join("leak.xml")).unwrap();
        assert!(copy_tree(&dir.join("res"), &dir.join("bad")).is_err());
        fs::remove_dir_all(&dir).unwrap();
    }
}
