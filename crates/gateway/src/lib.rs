//! baucord-gateway as a library: the pieces `main.rs` wires together,
//! exposed so the integration tests can run a [`session::Session`]
//! against a real server on the in-process network.

pub mod bot;
pub mod net;
pub mod session;
pub mod voice;
pub mod wire;
pub mod ws;
