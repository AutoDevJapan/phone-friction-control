//! Reproducible debug APK assembly. No Java/Kotlin business-logic sources.
use std::{
    env, fs, io,
    path::{Path, PathBuf},
    process::Command,
};

fn run(executable: &Path, args: &[&std::ffi::OsStr]) -> io::Result<()> {
    let mut command = Command::new(executable);
    command.args(args);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let status = command.status()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "{} failed: {status}",
            executable.display()
        )))
    }
}

fn tool(dir: &Path, name: &str) -> PathBuf {
    dir.join(if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_owned()
    })
}

pub(crate) fn assemble(args: &[std::ffi::OsString]) -> io::Result<()> {
    if args.len() != 4 {
        return Err(io::Error::other("pack <project-root> <native-library.so> <abi> <new-output-directory>; ANDROID_HOME and JAVA_HOME required"));
    }
    let root = Path::new(&args[0]).canonicalize()?;
    let library = Path::new(&args[1]).canonicalize()?;
    let abi = args[2]
        .to_str()
        .ok_or_else(|| io::Error::other("invalid ABI"))?;
    if !["arm64-v8a", "x86_64"].contains(&abi) {
        return Err(io::Error::other("unsupported ABI"));
    }
    // 広告入りのライブラリは広告用マニフェストでしか動かないため、通常のpackでは受け付けない。
    let ads_symbol = b"Java_com_autodevjapan_phonefrictioncontrol_AdLoadListener_onAdLoaded";
    if fs::read(&library)?
        .windows(ads_symbol.len())
        .any(|window| window == ads_symbol)
    {
        return Err(io::Error::other(
            "library was built with ads-test; use pack-ads for it",
        ));
    }
    let output = Path::new(&args[3]);
    // Never overwrite a previous artifact or someone else's working files.
    fs::create_dir(output)?;
    let output = output.canonicalize()?;
    let sdk = PathBuf::from(
        env::var_os("ANDROID_HOME").ok_or_else(|| io::Error::other("ANDROID_HOME missing"))?,
    );
    let jdk = PathBuf::from(
        env::var_os("JAVA_HOME").ok_or_else(|| io::Error::other("JAVA_HOME missing"))?,
    )
    .join("bin");
    let tools = sdk.join("build-tools/36.1.0");
    let android = sdk.join("platforms/android-36/android.jar");
    let classes = output.join("classes");
    let dex = output.join("dex");
    fs::create_dir(&dex)?;
    super::write_classes(&classes)?;
    let service = classes.join(super::PACKAGE).join("ControlService.class");
    let listener = classes.join(super::PACKAGE).join("MediaListener.class");
    let java = tool(&jdk, "java");
    let d8 = tools.join("lib/d8.jar");
    run(
        &java,
        &[
            "-cp".as_ref(),
            d8.as_os_str(),
            "com.android.tools.r8.D8".as_ref(),
            "--lib".as_ref(),
            android.as_os_str(),
            "--min-api".as_ref(),
            "26".as_ref(),
            "--output".as_ref(),
            dex.as_os_str(),
            service.as_os_str(),
            listener.as_os_str(),
        ],
    )?;
    let aapt = tool(&tools, "aapt2");
    let resources = output.join("resources.zip");
    let res = root.join("android/res");
    run(
        &aapt,
        &[
            "compile".as_ref(),
            "--dir".as_ref(),
            res.as_os_str(),
            "-o".as_ref(),
            resources.as_os_str(),
        ],
    )?;
    let unsigned = output.join("unsigned.apk");
    let manifest = root.join("android/AndroidManifest.xml");
    run(
        &aapt,
        &[
            "link".as_ref(),
            "-o".as_ref(),
            unsigned.as_os_str(),
            "--manifest".as_ref(),
            manifest.as_os_str(),
            "-I".as_ref(),
            android.as_os_str(),
            "--version-code".as_ref(),
            "2".as_ref(),
            "--version-name".as_ref(),
            "0.2.0".as_ref(),
            "--debug-mode".as_ref(),
            resources.as_os_str(),
        ],
    )?;
    let staging = output.join("staging");
    let native_relative = PathBuf::from(format!("lib/{abi}/libphone_friction_control.so"));
    let native = staging.join(&native_relative);
    fs::create_dir_all(native.parent().unwrap())?;
    fs::copy(&library, &native)?;
    let ndk = sdk
        .join("ndk/30.0.14904198/toolchains/llvm/prebuilt")
        .join(if cfg!(windows) {
            "windows-x86_64"
        } else {
            "linux-x86_64"
        })
        .join("bin");
    run(
        &tool(&ndk, "llvm-strip"),
        &["--strip-unneeded".as_ref(), native.as_os_str()],
    )?;
    run(
        &tool(&jdk, "jar"),
        &[
            "--update".as_ref(),
            "--file".as_ref(),
            unsigned.as_os_str(),
            "--no-compress".as_ref(),
            "-C".as_ref(),
            staging.as_os_str(),
            native_relative.as_os_str(),
            "-C".as_ref(),
            dex.as_os_str(),
            "classes.dex".as_ref(),
        ],
    )?;
    let aligned = output.join("aligned.apk");
    let zipalign = tool(&tools, "zipalign");
    run(
        &zipalign,
        &[
            "-P".as_ref(),
            "16".as_ref(),
            "-f".as_ref(),
            "4".as_ref(),
            unsigned.as_os_str(),
            aligned.as_os_str(),
        ],
    )?;
    let apk = output.join(format!("phone-control-{abi}-debug.apk"));
    let keystore = PathBuf::from(
        env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
            .ok_or_else(|| io::Error::other("home directory missing"))?,
    )
    .join(".android/debug.keystore");
    let signer = tools.join("lib/apksigner.jar");
    run(
        &java,
        &[
            "-jar".as_ref(),
            signer.as_os_str(),
            "sign".as_ref(),
            "--ks".as_ref(),
            keystore.as_os_str(),
            "--ks-key-alias".as_ref(),
            "androiddebugkey".as_ref(),
            "--ks-pass".as_ref(),
            "pass:android".as_ref(),
            "--key-pass".as_ref(),
            "pass:android".as_ref(),
            "--out".as_ref(),
            apk.as_os_str(),
            aligned.as_os_str(),
        ],
    )?;
    run(
        &java,
        &[
            "-jar".as_ref(),
            signer.as_os_str(),
            "verify".as_ref(),
            "--verbose".as_ref(),
            apk.as_os_str(),
        ],
    )?;
    run(
        &zipalign,
        &[
            "-c".as_ref(),
            "-P".as_ref(),
            "16".as_ref(),
            "4".as_ref(),
            apk.as_os_str(),
        ],
    )?;
    println!("{}", apk.display());
    Ok(())
}
