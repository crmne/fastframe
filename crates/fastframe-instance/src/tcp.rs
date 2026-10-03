//! The channel on platforms without Unix sockets: an ephemeral loopback
//! port, published with a token in the slot's directory.

use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::path::Path;

use super::{exchange, new_token, read_key, serve, spawn, write_key};

pub(super) fn send(dir: &Path, prefix: &str, request: &str) -> std::io::Result<String> {
    let (port, token) = read_key(dir)?;
    let stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port))?;
    exchange(stream, Some(&token), prefix, request)
}

/// Listens on an ephemeral loopback port and publishes it with a new token.
/// Only the lock holder gets here, so it replaces the file of an earlier run.
pub(super) fn listen(
    dir: &Path,
    prefix: String,
    handle: impl FnMut(&str) -> Option<String> + Send + 'static,
) -> std::io::Result<()> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    let token = new_token()?;
    write_key(dir, listener.local_addr()?.port(), &token)?;
    spawn(move || serve(listener.incoming(), Some(&token), &prefix, handle))
}
