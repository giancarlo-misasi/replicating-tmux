extern crate termion;

pub mod pty;
pub mod fd;

use std::{process::exit, sync::{mpsc::{channel, Sender}, Arc, Mutex}, thread};
use std::io::{self, stdin, stdout, Read, Write};
use pty::{Pty, PtySize};
use termion::{raw::IntoRawMode, terminal_size};

#[derive(Clone)]
pub(crate) struct WriterWrapper {
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
}

impl WriterWrapper {
    pub fn new(writer: Box<dyn Write + Send>) -> Self {
        Self {
            writer: Arc::new(Mutex::new(writer)),
        }
    }
}

impl std::io::Write for WriterWrapper {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let mut writer = self.writer.lock().unwrap();
        writer.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        let mut writer = self.writer.lock().unwrap();
        writer.flush()
    }
}

#[derive(Clone)]
pub(crate) struct ReaderWrapper {
    reader: Arc<Mutex<Box<dyn Read + Send>>>,
}

impl ReaderWrapper {
    pub fn new(reader: Box<dyn Read + Send>) -> Self {
        Self {
            reader: Arc::new(Mutex::new(reader)),
        }
    }

    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let mut reader = self.reader.lock().unwrap();
        reader.read(buf)
    }
}

const BUFSIZE: usize = 128 * 128;

fn read_from_shell(mut reader: ReaderWrapper, tx: Sender<Vec<u8>>) {
    thread::spawn(move || {
        let mut buf: Vec<u8> = vec![0; BUFSIZE]; // Reusable buffer
        loop {
            match reader.read(&mut buf) {
                Ok(bytes_read) => {
                    let data = buf[..bytes_read].to_vec();
                    if let Err(e) = tx.send(data) {
                        eprintln!("Failed to send message: {}", e);
                        break;
                    }
                }
                _ => {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
            }
        }
    });
}

fn pass_input_to_shell(mut writer: WriterWrapper) {
    thread::spawn(move || {
        let mut stdin = stdin().lock();
        let mut buf = [0u8; 1024];
        loop {
            if let Ok(bytes_read) = stdin.read(&mut buf) {
                if bytes_read > 0 && buf[0] == 0x11 {
                    exit(0);
                }

                let _ = writer.write(&buf[..bytes_read]);
                let _ = writer.flush();
            }
        }
    });
}

fn main() {
    println!("start");

    // setup
    let cmd = std::process::Command::new("zsh");
    let pty = Pty::open(cmd).unwrap();
    let reader = ReaderWrapper::new(pty.try_clone_reader().unwrap());
    let writer = WriterWrapper::new(pty.take_writer().unwrap());
    let (tx, rx) = channel();

    read_from_shell(reader.clone(), tx.clone());
    pass_input_to_shell(writer.clone());

    let mut stdout = stdout().into_raw_mode().unwrap();
    write!(stdout, "{}{}", termion::clear::All, termion::cursor::Goto(1, 1)).unwrap();

    // capture initial terminal size to check for changes
    let (mut cols, mut rows) = terminal_size().unwrap();
    pty.resize(PtySize {
        rows,
        cols,
        pixel_width: 0,
        pixel_height: 0,
    }).unwrap();

    // start output loop
    loop {
        let output = rx.recv().unwrap();
        if let Ok(str) = std::str::from_utf8(&output) {
            write!(stdout, "{}", str).unwrap();
            stdout.flush().unwrap();
        }

        // check if we need a resize
        let (c, r) = terminal_size().unwrap();
        if c != cols || r != rows {
            rows = r;
            cols = c;
            pty.resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            }).unwrap();
        }
    }
}
