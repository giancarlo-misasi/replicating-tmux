use std::{
    env,
    io::{self, stdin, stdout, Read, Write},
    os::{fd::AsRawFd, unix::net::UnixStream},
    sync::{
        atomic::{AtomicBool, Ordering::Relaxed},
        Arc,
    },
    thread, time::Duration,
};

use termion::{clear, cursor, raw::IntoRawMode, terminal_size};

struct Client {
    stop: Arc<AtomicBool>,
}

impl Client {
    pub fn new() -> Self {
        Self {
            stop: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn run(&self) -> io::Result<()> {
        let args: Vec<String> = env::args().collect();
        if args.len() != 2 {
            eprintln!("Usage: {} <session_name>", args[0]);
            std::process::exit(1);
        }

        let session_name = &args[1];
        let socket_path = format!("/tmp/rstmux/{}.sock", session_name);
        let stream = UnixStream::connect(socket_path)?;

        self.draw(&stream)?;
        self.process_input(&stream)?;

        Ok(())
    }

    fn draw(&self, stream: &UnixStream) -> io::Result<()> {
        let (mut cols, mut rows) = terminal_size().unwrap();
        let mut stdout = stdout().into_raw_mode().unwrap();
        let mut server_out = stream.try_clone()?;
        let stop = self.stop.clone();
        let mut buf = [0u8; 128 * 128];

        thread::spawn(move || {
            write!(stdout, "{}{}", clear::All, cursor::Goto(1, 1)).unwrap();

            loop {
                if stop.load(Relaxed) {
                    break;
                }

                match server_out.read(&mut buf) {
                    Ok(bytes_read) => {
                        if bytes_read == 0 {
                            break;
                        }

                        let data = buf[..bytes_read].to_vec();
                        if stdout.write_all(&data).is_err() {
                            break;
                        }
                        if stdout.flush().is_err() {
                            break;
                        }
                    }
                    _ => break,
                }

                if let Ok((c, r)) = terminal_size() {
                    if c != cols || r != rows {
                        (rows, cols) = (r, c);
                        // TODO: allow resize requests to server
                        // let _ = pty.resize(rows, cols); // ignore resize failures
                    }
                }
            }

            stop.store(true, Relaxed);
        });

        Ok(())
    }

    fn process_input(&self, stream: &UnixStream) -> io::Result<()> {
        let mut server_in = stream.try_clone()?;
        let mut stdin = stdin().lock();
        let stop = self.stop.clone();
        let mut buf = [0u8; 128]; // at least one row at a time

        // make stdin non-blocking
        let fd = stdin.as_raw_fd();
        unsafe {
            let flags = libc::fcntl(fd, libc::F_GETFL, 0);
            libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
        }

        loop {
            if stop.load(Relaxed) {
                break;
            }

            match stdin.read(&mut buf) {
                Ok(bytes_read) => {
                    if stop.load(Relaxed) {
                        break;
                    }

                    if server_in.write(&buf[..bytes_read]).is_err() {
                        break;
                    }
                }
                Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(5));
                }
                _ => break,
            }
        }
        stop.store(true, Relaxed);

        Ok(())
    }
}

fn main() {
    let client = Client::new();
    client.run().unwrap();
    println!("rstmux client exited");
}
