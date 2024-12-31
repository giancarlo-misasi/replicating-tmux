use replicating_tmux::pty::Pty;
use replicating_tmux::socket::bind_unix_socket;
use std::env;
use std::io::{self, Read, Write};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;
// use termion::terminal_size;

struct Client {
    stream: UnixStream,
    stop: Arc<AtomicBool>,
}

impl Client {
    pub fn new(stream: UnixStream) -> Self {
        Self {
            stream,
            stop: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn start(
        &self,
        server_in: Sender<Vec<u8>>,
        pty_out: Box<dyn Read + Send>,
    ) -> io::Result<()> {
        self.process_output(pty_out)?;
        self.process_input(server_in)?;
        Ok(())
    }

    pub fn stop(&self) -> io::Result<()> {
        self.stream.shutdown(Shutdown::Both)?;
        self.stop.store(true, Relaxed);
        Ok(())
    }

    pub fn stopped(&self) -> bool {
        self.stop.load(Relaxed)
    }

    fn process_input(&self, server_in: Sender<Vec<u8>>) -> io::Result<()> {
        let mut client_out = self.stream.try_clone()?;
        let stop = self.stop.clone();

        // keep running until stop or failure
        std::thread::spawn(move || {
            let mut inbuf = [0u8; 1024];
            loop {
                if stop.load(Relaxed) {
                    break;
                }

                match client_out.read(&mut inbuf) {
                    Ok(bytes_read) => {
                        if bytes_read == 0 {
                            break; // EOF
                        }

                        let data = inbuf[..bytes_read].to_vec();
                        if server_in.send(data).is_err() {
                            break;
                        }
                    }
                    _ => break,
                }
            }
            println!("should stop because of client input");
            stop.store(true, Relaxed);
        });

        Ok(())
    }

    fn process_output(&self, mut pty_out: Box<dyn Read + Send>) -> io::Result<()> {
        let mut client_in = self.stream.try_clone()?;
        let stop = self.stop.clone();

        // keep running until stop or failure
        std::thread::spawn(move || {
            let mut outbuf = [0u8; 128 * 128];
            loop {
                if stop.load(Relaxed) {
                    break;
                }

                // pass pty output back to client
                match pty_out.read(&mut outbuf) {
                    Ok(bytes_read) => {
                        if bytes_read == 0 {
                            break; // EOF
                        }

                        let data = outbuf[..bytes_read].to_vec();
                        if client_in.write(&data).is_err() {
                            break;
                        }
                    }
                    _ => break,
                }
            }
            println!("should stop because of process output");
            stop.store(true, Relaxed);
        });

        Ok(())
    }
}

struct Server {
    pty: Arc<Mutex<Pty>>,
    clients: Arc<Mutex<Vec<Client>>>,
    stop: Arc<AtomicBool>,
}

impl Server {
    pub fn new(pty: Pty) -> Self {
        Server {
            pty: Arc::new(Mutex::new(pty)),
            clients: Arc::new(Mutex::new(vec![])),
            stop: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn run(&self, session_name: &str) -> io::Result<()> {
        let (tx, rx) = channel();
        self.accept_clients(session_name, tx)?;
        self.process_input(rx)
    }

    fn accept_clients(&self, session_name: &str, server_in: Sender<Vec<u8>>) -> io::Result<()> {
        let socket_path = format!("/tmp/rstmux/{}.sock", session_name);
        let listener = bind_unix_socket(&socket_path)?;
        listener.set_nonblocking(true)?;
        let pty = self.pty.clone();
        let clients = self.clients.clone();
        let stop = self.stop.clone();

        std::thread::spawn(move || {
            loop {
                if stop.load(Relaxed) {
                    break;
                }

                match listener.accept() {
                    Ok((stream, _)) => {
                        let client = Client::new(stream);
                        let server_in = server_in.clone();
                        let pty_out = pty.lock().unwrap().try_clone_reader().unwrap();
                        client.start(server_in, pty_out).unwrap();
                        println!("client connected");

                        let mut clients = clients.lock().unwrap();
                        clients.retain(|c| !c.stopped());
                        clients.push(client);
                    },
                    Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(100));
                    },
                    _ => break,
                }

                if pty.lock().unwrap().stopped().unwrap() {
                    stop.store(true, Relaxed);
                }
            }

            let clients = clients.lock().unwrap();
            for client in clients.iter() {
                let _ = client.stop();
            }

            stop.store(true, Relaxed);
            println!("accept clients done");
        });

        Ok(())
    }

    fn process_input(&self, aggregated_input: Receiver<Vec<u8>>) -> io::Result<()> {
        let mut pty_in = self.pty.lock().unwrap().take_writer()?;
        let stop = self.stop.clone();

        loop {
            if stop.load(Relaxed) {
                break;
            }

            match aggregated_input.recv() {
                Ok(buf) => {
                    println!("input received: {}", buf.len());
                    if pty_in.write(&buf).is_err() {
                        break;
                    }
                }
                _ => break,
            }
        }
        stop.store(true, Relaxed);

        Ok(())
    }
}

fn run() -> io::Result<()> {
    let args: Vec<String> = env::args().collect();
    if args.len() != 2 {
        eprintln!("Usage: {} <session_name>", args[0]);
        std::process::exit(1);
    }

    // resize the client
    // let (mut cols, mut rows) = terminal_size().unwrap();
    // pty.resize(rows, cols).unwrap();

    let session_name = &args[1];
    let cmd = std::process::Command::new("zsh");
    let pty = Pty::open(cmd)?;
    let server = Server::new(pty);
    server.run(session_name)
}

fn main() {
    run().unwrap();
}
