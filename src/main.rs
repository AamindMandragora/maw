fn main() {
    // die quietly like other unix tools when piped into something that stops reading, e.g. `maw query | head`
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }

    if let Err(error) = maw::cli::main() {
        eprintln!("error: {error:#}");
        std::process::exit(1);
    }
}
