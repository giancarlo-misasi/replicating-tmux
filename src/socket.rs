use std::{fs, io};
use std::os::unix::net::UnixListener;
use std::path::Path;

pub fn bind_unix_socket(socket_path: &str) -> io::Result<UnixListener> {
    // create directories if missing
    if let Some(parent) = Path::new(socket_path).parent() {
        fs::create_dir_all(parent)?;
    }

    // cleanup existing socket files
    if Path::new(socket_path).exists() {
        fs::remove_file(socket_path)?;
    }

    // bind the socket
    UnixListener::bind(socket_path)
}
