pub mod gateway;
pub mod network;

pub use gateway::{active_device, router};
pub use network::dns_servers;
pub use network::{os, os_name, register, supports_gateway, Networks, Os};
