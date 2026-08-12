fn main() {
    std::process::exit(packager::cli::run(std::env::args().skip(1)));
}
