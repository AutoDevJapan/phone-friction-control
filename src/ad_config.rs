//! Manifest keys and ID validation shared by the native runtime and APK builder.
pub(crate) const APP_KEY: &str = "com.google.android.gms.ads.APPLICATION_ID";
pub(crate) const BANNER_KEY: &str = "com.autodevjapan.phonefrictioncontrol.ads.BANNER_UNIT_ID";
pub(crate) const SAMPLE_APP: &str = "ca-app-pub-3940256099942544~3347511713";
pub(crate) const SAMPLE_BANNER: &str = "ca-app-pub-3940256099942544/9214589741";

pub(crate) fn validate(app: &str, banner: &str, production: bool) -> Result<(), &'static str> {
    fn publisher(id: &str, separator: u8) -> Option<&str> {
        let (publisher, unit) = id
            .strip_prefix("ca-app-pub-")?
            .split_once(char::from(separator))?;
        (publisher.len() == 16
            && publisher.bytes().all(|b| b.is_ascii_digit())
            && unit.len() == 10
            && unit.bytes().all(|b| b.is_ascii_digit()))
        .then_some(publisher)
    }
    let app_publisher = publisher(app, b'~').ok_or("invalid AdMob app ID")?;
    let banner_publisher = publisher(banner, b'/').ok_or("invalid banner unit ID")?;
    if app_publisher != banner_publisher {
        return Err("AdMob IDs must have the same publisher");
    }
    if production {
        if app_publisher == "3940256099942544" {
            return Err("sample publisher cannot be used in production");
        }
    } else if app != SAMPLE_APP || banner != SAMPLE_BANNER {
        return Err("test ads require the official sample IDs");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixture_and_sample_are_distinct_and_ids_are_strict() {
        let app = "ca-app-pub-1234567890123456~1234567890";
        let banner = "ca-app-pub-1234567890123456/9876543210";
        assert!(validate(app, banner, true).is_ok()); // Syntax fixture only; never request ads.
        assert!(validate(SAMPLE_APP, SAMPLE_BANNER, false).is_ok());
        assert!(validate(SAMPLE_APP, SAMPLE_BANNER, true).is_err());
        assert!(validate(app, banner, false).is_err());
        for bad in [
            "",
            "ca-app-pub-1234567890123456/1234567890",
            "ca-app-pub-1234567890123456~123456789",
            "ca-app-pub-1234567890123456~1234567890\n",
            "ca-app-pub-1234567890123456~123456789'",
            "ca-app-pub-１２３４５６７８９０１２３４５６~1234567890",
        ] {
            assert!(validate(bad, banner, true).is_err());
        }
        assert!(validate(app, "ca-app-pub-1234567890123457/9876543210", true).is_err());
        assert!(validate(app, "ca-app-pub-1234567890123456~9876543210", true).is_err());
    }
}

#[cfg(test)]
mod consent_debug_tests {
    #[test]
    fn consent_debug_geography_matches_the_selected_build() {
        assert_eq!(
            super::consent_debug_geography(),
            cfg!(feature = "ads-test").then_some(1)
        );
    }
}
#[cfg(any(feature = "ads-test", test))]
pub(crate) const fn consent_debug_geography() -> Option<i32> {
    if cfg!(feature = "ads-test") {
        Some(1)
    } else {
        None
    }
}
