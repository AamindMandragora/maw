fn main() {
    if let Err(error) = maw::cli::main() {
        eprintln!("error: {error:#}");
        std::process::exit(1);
    }
}
