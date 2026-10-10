//! Inspect defined dynamic exports in the same ELF bytes that will be packaged.
use std::io;

pub(super) const TEST: &str = "phone_control_ads_test";
pub(super) const PRODUCTION: &str = "phone_control_ads_production";
pub(super) const CALLBACK: &str =
    "Java_com_autodevjapan_phonefrictioncontrol_AdLoadListener_onAdLoaded";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Flavor {
    Plain,
    Test,
    Production,
}

fn invalid() -> io::Error {
    io::Error::other("invalid or unsupported Android ELF library")
}

fn range(bytes: &[u8], offset: u64, size: u64) -> io::Result<&[u8]> {
    let start = usize::try_from(offset).map_err(|_| invalid())?;
    let len = usize::try_from(size).map_err(|_| invalid())?;
    bytes
        .get(start..start.checked_add(len).ok_or_else(invalid)?)
        .ok_or_else(invalid)
}

fn number(bytes: &[u8], at: usize, size: usize) -> io::Result<u64> {
    let mut value = [0; 8];
    value[..size].copy_from_slice(bytes.get(at..at + size).ok_or_else(invalid)?);
    Ok(u64::from_le_bytes(value))
}

pub(super) fn validate(bytes: &[u8], abi: &str, expected: Flavor) -> io::Result<()> {
    if bytes.get(..7) != Some(b"\x7fELF\x02\x01\x01")
        || number(bytes, 16, 2)? != 3 // ET_DYN
        || number(bytes, 18, 2)? != match abi { "arm64-v8a" => 183, "x86_64" => 62, _ => return Err(invalid()) }
        || number(bytes, 58, 2)? != 64
    {
        return Err(invalid());
    }
    let sections = range(
        bytes,
        number(bytes, 40, 8)?,
        number(bytes, 60, 2)?.checked_mul(64).ok_or_else(invalid)?,
    )?;
    if sections.is_empty() {
        return Err(invalid());
    }
    let mut names = Vec::new();
    for section in sections.as_chunks::<64>().0 {
        if number(section, 4, 4)? != 11 {
            continue;
        } // SHT_DYNSYM
        if number(section, 56, 8)? != 24 {
            return Err(invalid());
        }
        let symbols = range(bytes, number(section, 24, 8)?, number(section, 32, 8)?)?;
        if symbols.len() % 24 != 0 {
            return Err(invalid());
        }
        let linked = number(section, 40, 4)?
            .checked_mul(64)
            .ok_or_else(invalid)?;
        let strings_section = range(sections, linked, 64)?;
        if number(strings_section, 4, 4)? != 3 {
            return Err(invalid());
        } // SHT_STRTAB
        let strings = range(
            bytes,
            number(strings_section, 24, 8)?,
            number(strings_section, 32, 8)?,
        )?;
        for symbol in symbols.as_chunks::<24>().0 {
            // Only externally visible, defined global/weak functions count, not incidental strings.
            if ![1, 2].contains(&(symbol[4] >> 4))
                || symbol[4] & 15 != 2
                || ![0, 3].contains(&(symbol[5] & 3))
                || number(symbol, 6, 2)? == 0
            {
                continue;
            }
            let at = usize::try_from(number(symbol, 0, 4)?).map_err(|_| invalid())?;
            let tail = strings.get(at..).ok_or_else(invalid)?;
            let end = tail.iter().position(|b| *b == 0).ok_or_else(invalid)?;
            names.push(&tail[..end]);
        }
    }
    validate_exports(&names, expected)
}

fn validate_exports(names: &[&[u8]], expected: Flavor) -> io::Result<()> {
    let has = |name: &str| names.contains(&name.as_bytes());
    let actual = match (has(TEST), has(PRODUCTION), has(CALLBACK)) {
        (false, false, false) => Flavor::Plain,
        (true, false, true) => Flavor::Test,
        (false, true, true) => Flavor::Production,
        _ => {
            return Err(io::Error::other(
                "missing or mixed native advertising feature exports",
            ))
        }
    };
    if actual != expected {
        return Err(io::Error::other(format!(
            "native ads flavor {actual:?} cannot be packaged as {expected:?}"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // ELF fixture with a real dynamic symbol table; not an executable and never requests ads.
    fn elf(names: &[&str]) -> Vec<u8> {
        let mut bytes = vec![0; 256 + names.len() * 24];
        bytes[..7].copy_from_slice(b"\x7fELF\x02\x01\x01");
        bytes[16..18].copy_from_slice(&3u16.to_le_bytes());
        bytes[18..20].copy_from_slice(&62u16.to_le_bytes());
        bytes[40..48].copy_from_slice(&64u64.to_le_bytes());
        bytes[58..60].copy_from_slice(&64u16.to_le_bytes());
        bytes[60..62].copy_from_slice(&3u16.to_le_bytes());
        bytes[132..136].copy_from_slice(&11u32.to_le_bytes());
        bytes[152..160].copy_from_slice(&256u64.to_le_bytes());
        bytes[160..168].copy_from_slice(&(names.len() as u64 * 24).to_le_bytes());
        bytes[168..172].copy_from_slice(&2u32.to_le_bytes());
        bytes[184..192].copy_from_slice(&24u64.to_le_bytes());
        bytes[196..200].copy_from_slice(&3u32.to_le_bytes());
        let strings_start = bytes.len();
        bytes.push(0);
        for (index, name) in names.iter().enumerate() {
            let offset = (bytes.len() - strings_start) as u32;
            let sym = 256 + index * 24;
            bytes[sym..sym + 4].copy_from_slice(&offset.to_le_bytes());
            bytes[sym + 4] = 0x12;
            bytes[sym + 6] = 1;
            bytes.extend_from_slice(name.as_bytes());
            bytes.push(0);
        }
        bytes[216..224].copy_from_slice(&(strings_start as u64).to_le_bytes());
        let size = bytes.len() - strings_start;
        bytes[224..232].copy_from_slice(&(size as u64).to_le_bytes());
        bytes
    }

    #[test]
    fn exported_features_reject_mixing_and_incidental_strings() {
        for (names, flavor) in [
            (vec![], Flavor::Plain),
            (vec![TEST, CALLBACK], Flavor::Test),
            (vec![PRODUCTION, CALLBACK], Flavor::Production),
        ] {
            let bytes = elf(&names);
            for requested in [Flavor::Plain, Flavor::Test, Flavor::Production] {
                assert_eq!(
                    validate(&bytes, "x86_64", requested).is_ok(),
                    flavor == requested
                );
            }
        }
        assert!(validate(
            &elf(&[TEST, PRODUCTION, CALLBACK]),
            "x86_64",
            Flavor::Production
        )
        .is_err());
        assert!(validate(&elf(&[CALLBACK]), "x86_64", Flavor::Test).is_err());
        let mut plain = elf(&[]);
        plain.extend_from_slice(PRODUCTION.as_bytes());
        assert!(validate(&plain, "x86_64", Flavor::Plain).is_ok());
        let mut undefined = elf(&[PRODUCTION, CALLBACK]);
        undefined[262] = 0;
        assert!(validate(&undefined, "x86_64", Flavor::Production).is_err());
        assert!(validate(&elf(&[]), "arm64-v8a", Flavor::Plain).is_err());
        assert!(validate(&[0; 16], "x86_64", Flavor::Plain).is_err());
    }
}
