pub mod cli;
pub mod platform;
pub mod utils;
pub mod vpn;

pub type Error = String;

pub type Result<T> = std::result::Result<T, Error>;

