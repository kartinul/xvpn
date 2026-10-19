use clap::Parser;

use xvpn::cli::{dispatch, Cli};

fn main() {
    // exit quietly like other unix tools when piped into `head` instead of panicking
    unsafe { libc::signal(libc::SIGPIPE, libc::SIG_DFL) };
    dispatch(Cli::parse());
}
