fn main() {
    // `--self-check`: verify the bundled runtime without opening a window (scripts/verify-bundle.sh).
    if std::env::args().any(|a| a == "--self-check") {
        std::process::exit(nlmx_desktop::self_check::run());
    }
    nlmx_desktop::run();
}
