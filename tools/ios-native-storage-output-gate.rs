use std::{collections::BTreeSet, env, fs, process};

const NEGATIVE: &str = "test app::tests::runtime_native_documents_failed_load_blocks_overwrite ... runtime_native_documents_failed_load_preserved=true; overwrite_blocked=true; owned_fixture_cleanup=true";
const POSITIVE: &str = "test app::tests::runtime_native_documents_roundtrip_preserves_nondefault_state ... runtime_native_documents_roundtrip_verified=true; initial_fixture_limit=300; file_readback_limit=360; reconstructed_limit=360; os_permission_verified=false; process_restart_verified=false; owned_fixture_cleanup=true";
const SUMMARY_PREFIX: &str = "test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; ";

#[derive(Debug, PartialEq, Eq)]
struct Report {
    batches: usize,
    unique_tests: usize,
}

fn successful_summary(line: &str) -> bool {
    let Some(rest) = line.strip_prefix(SUMMARY_PREFIX) else {
        return false;
    };
    let Some((filtered, elapsed)) = rest.split_once(" filtered out; finished in ") else {
        return false;
    };
    let Some(seconds) = elapsed.strip_suffix('s') else {
        return false;
    };
    filtered.parse::<usize>().is_ok()
        && seconds
            .parse::<f64>()
            .is_ok_and(|n| n.is_finite() && n >= 0.0)
}

fn check(stdout: &str, stderr: &str, quiescent: bool) -> Result<Report, &'static str> {
    if !quiescent {
        return Err("target process collection did not reach quiescence");
    }
    for text in [stdout, stderr] {
        let lower = text.to_ascii_lowercase();
        if lower.contains("panic")
            || text.contains("FAILED")
            || lower.contains("failures:")
            || lower.contains("native documents test cleanup incomplete")
        {
            return Err("captured target output contains a failure");
        }
    }
    // This producer runs exactly the two named tests with nocapture/color never.
    // Parse every complete batch, accepting either test order. An unexpected or
    // truncated trailing record is not discarded after an earlier success.
    let lines: Vec<_> = stdout
        .lines()
        .map(|line| line.trim_end_matches('\r'))
        .filter(|line| !line.trim().is_empty())
        .collect();
    let mut offset = 0;
    let mut batches = 0;
    while offset < lines.len() {
        if lines[offset] != "running 2 tests" {
            return Err("missing or partial batch header");
        }
        offset += 1;
        let mut seen = BTreeSet::new();
        for _ in 0..2 {
            let record = *lines.get(offset).ok_or("incomplete named test record")?;
            let identity = if record == NEGATIVE {
                0
            } else if record == POSITIVE {
                1
            } else {
                return Err("missing or altered named test success assertion");
            };
            if !seen.insert(identity) {
                return Err("duplicate named test within a batch");
            }
            offset += 1;
            if lines.get(offset) != Some(&"ok") {
                return Err("named test has no completed ok record");
            }
            offset += 1;
        }
        if !lines
            .get(offset)
            .is_some_and(|line| successful_summary(line))
        {
            return Err("missing, failed, altered or partial batch summary");
        }
        offset += 1;
        batches += 1;
    }
    if batches == 0 {
        return Err("no complete test batch captured");
    }
    Ok(Report {
        batches,
        unique_tests: 2,
    })
}

fn target_pids(snapshot: &str, executable: &str) -> Result<BTreeSet<u32>, &'static str> {
    if !executable.starts_with('/')
        || !executable.ends_with("/NativeStorageTests.app/native-storage-tests")
    {
        return Err("unexpected installed target executable");
    }
    let mut found = BTreeSet::new();
    let mut records = 0;
    for line in snapshot.lines() {
        let line = line.trim_start();
        let Some(boundary) = line.find(char::is_whitespace) else {
            continue;
        };
        let Ok(pid) = line[..boundary].parse::<u32>() else {
            continue;
        };
        let command = line[boundary..].trim();
        if command.is_empty() {
            continue;
        }
        records += 1;
        if command == executable
            || command == "native-storage-tests"
            || command.ends_with("/NativeStorageTests.app/native-storage-tests")
        {
            found.insert(pid);
        }
    }
    if records == 0 {
        return Err("empty or malformed host process snapshot");
    }
    Ok(found)
}

fn main() {
    let args: Vec<_> = env::args().collect();
    let result =
        (|| -> Result<(), Box<dyn std::error::Error>> {
            match args.get(1).map(String::as_str) {
                Some("check") if args.len() == 5 => {
                    let stdout = fs::read_to_string(&args[2])?;
                    let stderr = fs::read_to_string(&args[3])?;
                    let collection = fs::read_to_string(&args[4])?;
                    let quiescent = collection
                        .lines()
                        .any(|line| line == "target_processes_quiescent=true");
                    let report = check(&stdout, &stderr, quiescent)?;
                    println!("successful_output_batches={}", report.batches);
                    println!("unique_native_tests={}", report.unique_tests);
                    println!("all_collected_batches_complete=true");
                }
                Some("target-pids") if args.len() == 4 => {
                    let snapshot = fs::read_to_string(&args[2])?;
                    for pid in target_pids(&snapshot, &args[3])? {
                        println!("{pid}");
                    }
                }
                _ => return Err(
                    "expected check stdout stderr collection, or target-pids snapshot executable"
                        .into(),
                ),
            }
            Ok(())
        })();
    if let Err(error) = result {
        eprintln!("native storage capture rejected: {error}");
        process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn batch() -> String {
        format!(
            "\nrunning 2 tests\n{NEGATIVE}\nok\n{POSITIVE}\nok\ntest result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 73 filtered out; finished in 0.01s\n"
        )
    }

    #[test]
    fn accepts_single_and_all_successful_batches() {
        assert_eq!(check(&batch(), "", true).unwrap().batches, 1);
        assert_eq!(check(&(batch() + &batch()), "", true).unwrap().batches, 2);
    }

    #[test]
    fn rejects_failure_in_either_batch() {
        let failed = batch().replace(
            "test result: ok. 2 passed; 0 failed;",
            "test result: FAILED. 1 passed; 1 failed;",
        );
        assert!(check(&(failed.clone() + &batch()), "", true).is_err());
        assert!(check(&(batch() + &failed), "", true).is_err());
    }

    #[test]
    fn rejects_partial_or_missing_summaries() {
        let good = batch();
        let no_summary = good[..good.find("test result:").unwrap()].to_owned();
        assert!(check(&(no_summary + &good), "", true).is_err());
        assert!(check(&(good.clone() + "\nrunning 2 te"), "", true).is_err());
        assert!(check(&good[..good.len() - 6], "", true).is_err());
        assert!(check("", "", true).is_err());
    }

    #[test]
    fn rejects_missing_assertions_and_duplicate_names() {
        assert!(
            check(
                &batch().replace(POSITIVE, "test roundtrip ... ok"),
                "",
                true
            )
            .is_err()
        );
        assert!(check(&batch().replace(POSITIVE, NEGATIVE), "", true).is_err());
        assert!(check(&batch().replace("\nok\n", "\n"), "", true).is_err());
    }

    #[test]
    fn rejects_panics_failures_and_unfinished_processes() {
        assert!(check(&batch(), "thread test panicked at native file", true).is_err());
        assert!(check(&(batch() + "\nfailures:\n"), "", true).is_err());
        assert!(check(&batch(), "", false).is_err());
    }

    #[test]
    fn accepts_either_test_order_and_crlf() {
        let reversed = format!(
            "running 2 tests\n{POSITIVE}\nok\n{NEGATIVE}\nok\ntest result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 73 filtered out; finished in 0.01s\n"
        );
        assert!(check(&reversed.replace('\n', "\r\n"), "", true).is_ok());
    }

    #[test]
    fn selects_only_the_target_from_host_ps_snapshot() {
        let path = "/sim/NativeStorageTests.app/native-storage-tests";
        let snapshot = format!(
            " 6881 {path}\n6886 native-storage-tests\n12 /usr/bin/ps\n13 /usr/bin/awk\n14 /sim/Other.app/other\n15 native-storage-tests-helper\n"
        );
        assert_eq!(
            target_pids(&snapshot, path).unwrap(),
            BTreeSet::from([6881, 6886])
        );
        assert!(target_pids("12 /usr/bin/ps\n", path).unwrap().is_empty());
        assert!(target_pids("", path).is_err());
        assert!(target_pids("not a process record", path).is_err());
        assert!(target_pids(&snapshot, "/unexpected").is_err());
    }
}
