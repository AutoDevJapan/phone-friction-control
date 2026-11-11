#[cfg(target_os = "ios")]
fn main() -> eframe::Result {
    phone_friction_control::run_ios_runtime()
}

#[cfg(not(target_os = "ios"))]
fn main() {
    eprintln!("This entry requires an iOS target.");
    std::process::exit(1);
}
