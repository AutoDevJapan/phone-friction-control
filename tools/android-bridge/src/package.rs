//! APK assembly with either the standard debug key or a pinned release signer.
use std::{
    env, fs, io,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

struct Release {
    keystore: PathBuf,
    alias: std::ffi::OsString,
    version_code: String,
    version_name: String,
    certificate_sha256: String,
}

fn digest(value: &str) -> io::Result<String> {
    if value.len() != 64 || !value.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(io::Error::other(
            "expected a 64-character certificate SHA-256",
        ));
    }
    Ok(value.to_ascii_lowercase())
}

impl Release {
    fn parse(args: &[std::ffi::OsString]) -> io::Result<Self> {
        let code = args[6]
            .to_str()
            .and_then(|v| v.parse::<u32>().ok())
            .filter(|v| *v > 0 && *v <= 2_100_000_000)
            .ok_or_else(|| io::Error::other("invalid Android version code"))?;
        let name = args[7]
            .to_str()
            .filter(|v| !v.is_empty() && !v.chars().any(char::is_control))
            .ok_or_else(|| io::Error::other("invalid version name"))?;
        if args[5].is_empty() {
            return Err(io::Error::other("key alias is empty"));
        }
        Ok(Self {
            keystore: Path::new(&args[4]).canonicalize()?,
            alias: args[5].clone(),
            version_code: code.to_string(),
            version_name: name.to_owned(),
            certificate_sha256: digest(args[8].to_str().unwrap_or(""))?,
        })
    }
}

fn verify_release_certificate(listing: &str, expected: &str) -> io::Result<()> {
    // apksigner is run with an English locale; this is the certificate, not the APK hash.
    let hashes: Vec<_> = listing
        .lines()
        .filter_map(|line| line.strip_prefix("Signer #1 certificate SHA-256 digest: "))
        .collect();
    if listing.to_ascii_lowercase().contains("cn=android debug") {
        return Err(io::Error::other(
            "Android debug certificates cannot sign a release",
        ));
    }
    if !listing.lines().any(|line| line == "Number of signers: 1")
        || hashes.len() != 1
        || digest(hashes[0])? != expected
    {
        return Err(io::Error::other(
            "release certificate does not match the pinned SHA-256",
        ));
    }
    Ok(())
}

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

pub(crate) fn assemble(args: &[std::ffi::OsString], release: bool) -> io::Result<()> {
    if args.len() != if release { 9 } else { 4 } {
        return Err(io::Error::other("pack <root> <library.so> <abi> <new-output>; pack-release additionally requires <keystore> <alias> <version-code> <version-name> <certificate-sha256> and PHONE_CONTROL_KS_PASS / PHONE_CONTROL_KEY_PASS"));
    }
    let signing = if release {
        let signing = Release::parse(args)?;
        for name in ["PHONE_CONTROL_KS_PASS", "PHONE_CONTROL_KEY_PASS"] {
            if env::var_os(name).is_none_or(|v| v.is_empty()) {
                return Err(io::Error::other(format!("{name} missing")));
            }
        }
        Some(signing)
    } else {
        None
    };
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
    let library_bytes = fs::read(&library)?;
    if library_bytes
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
    let mut link_args: Vec<&std::ffi::OsStr> = vec![
        "link".as_ref(),
        "-o".as_ref(),
        unsigned.as_os_str(),
        "--manifest".as_ref(),
        manifest.as_os_str(),
        "-I".as_ref(),
        android.as_os_str(),
        "--version-code".as_ref(),
        signing.as_ref().map_or("2", |s| &s.version_code).as_ref(),
        "--version-name".as_ref(),
        signing
            .as_ref()
            .map_or("0.2.0", |s| &s.version_name)
            .as_ref(),
        resources.as_os_str(),
    ];
    if signing.is_none() {
        link_args.push("--debug-mode".as_ref());
    }
    run(&aapt, &link_args)?;
    if release {
        let mut check = Command::new(&aapt);
        check.args(["dump", "badging"]).arg(&unsigned);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            check.creation_flags(0x08000000);
        }
        let result = check.output()?;
        if !result.status.success()
            || String::from_utf8_lossy(&result.stdout)
                .lines()
                .any(|line| line.trim() == "application-debuggable")
        {
            return Err(io::Error::other(
                "release APK must have a readable, non-debuggable manifest",
            ));
        }
    }
    let staging = output.join("staging");
    let native_relative = PathBuf::from(format!("lib/{abi}/libphone_friction_control.so"));
    let native = staging.join(&native_relative);
    fs::create_dir_all(native.parent().unwrap())?;
    // Use the bytes checked above, so a concurrent Cargo rebuild cannot change the input.
    fs::write(&native, library_bytes)?;
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
    let apk = output.join(format!(
        "phone-control-{abi}-{}.apk",
        if release { "release" } else { "debug" }
    ));
    let candidate = output.join("signed-candidate.apk");
    let keystore = PathBuf::from(
        env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
            .ok_or_else(|| io::Error::other("home directory missing"))?,
    )
    .join(".android/debug.keystore");
    let signer = tools.join("lib/apksigner.jar");
    if let Some(signing) = &signing {
        let mut command = Command::new(&java);
        command
            .args(["-jar"])
            .arg(&signer)
            .arg("sign")
            .arg("--ks")
            .arg(&signing.keystore)
            .arg("--ks-key-alias")
            .arg(&signing.alias)
            .args([
                "--ks-pass",
                "env:PHONE_CONTROL_KS_PASS",
                "--key-pass",
                "env:PHONE_CONTROL_KEY_PASS",
                "--out",
            ])
            .arg(&candidate)
            .arg(&aligned);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
        }
        // Signer diagnostics can expose secret material; return only its exit status.
        let status = command
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()?;
        if !status.success() {
            return Err(io::Error::other(format!(
                "release signing failed: {status}"
            )));
        }
        let mut verify = Command::new(&java);
        verify
            .args(["-Duser.language=en", "-Duser.country=US", "-jar"])
            .arg(&signer)
            .args(["verify", "--verbose", "--print-certs"])
            .arg(&candidate);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            verify.creation_flags(0x08000000);
        }
        let verified = verify.output()?;
        if !verified.status.success() {
            return Err(io::Error::other("release signature verification failed"));
        }
        verify_release_certificate(
            &String::from_utf8_lossy(&verified.stdout),
            &signing.certificate_sha256,
        )?;
    } else {
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
                candidate.as_os_str(),
                aligned.as_os_str(),
            ],
        )?;
    }
    run(
        &java,
        &[
            "-jar".as_ref(),
            signer.as_os_str(),
            "verify".as_ref(),
            "--verbose".as_ref(),
            candidate.as_os_str(),
        ],
    )?;
    run(
        &zipalign,
        &[
            "-c".as_ref(),
            "-P".as_ref(),
            "16".as_ref(),
            "4".as_ref(),
            candidate.as_os_str(),
        ],
    )?;
    fs::rename(candidate, &apk)?;
    println!("{}", apk.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHA: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    fn certificate(dn: &str, sha: &str) -> String {
        format!("Number of signers: 1\nSigner #1 certificate DN: {dn}\nSigner #1 certificate SHA-256 digest: {sha}\n")
    }

    #[test]
    fn release_requires_the_exact_single_certificate() {
        assert!(verify_release_certificate(&certificate("CN=AutoDevJapan", SHA), SHA).is_ok());
        assert!(
            verify_release_certificate(&certificate("CN=AutoDevJapan", &"b".repeat(64)), SHA)
                .is_err()
        );
        assert!(verify_release_certificate("Number of signers: 1", SHA).is_err());
        assert!(verify_release_certificate(
            &certificate("CN=AutoDevJapan", SHA).replace("signers: 1", "signers: 2"),
            SHA
        )
        .is_err());
        let duplicate = certificate("CN=AutoDevJapan", SHA)
            + &format!("Signer #1 certificate SHA-256 digest: {SHA}\n");
        assert!(verify_release_certificate(&duplicate, SHA).is_err());
        assert!(verify_release_certificate(
            &certificate("CN=Android Debug,O=Android,C=US", SHA),
            SHA
        )
        .is_err());
    }

    #[test]
    fn digest_is_not_a_path_or_malformed_hex() {
        assert_eq!(digest(&SHA.to_ascii_uppercase()).unwrap(), SHA);
        for invalid in ["", "123", &"g".repeat(64), &format!("{} ", "a".repeat(63))] {
            assert!(digest(invalid).is_err());
        }
    }
}
