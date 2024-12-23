extern crate termion;

use std::{process::exit, sync::{mpsc::{channel, Sender}, Arc, Mutex}, thread};
use std::io::{self, stdin, stdout, Read, Write};
use portable_pty::{Child, CommandBuilder, NativePtySystem, PtySize, PtySystem, MasterPty};
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

pub struct ZshPair {
    pub parent: Box<dyn MasterPty + Send>,
    pub child: Box<dyn Child + Send>,
}

fn spawn_zsh() -> ZshPair {
    let pty_system = NativePtySystem::default();

    let pair = pty_system
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();

    let cmd = CommandBuilder::new("zsh");
    let child = pair.slave.spawn_command(cmd).unwrap();

    // Release any handles owned by the slave: we don't need it now
    // that we've spawned the child.
    drop(pair.slave);

    ZshPair {
        parent: pair.master,
        child,
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
    let pair = spawn_zsh();
    let reader = ReaderWrapper::new(pair.parent.try_clone_reader().unwrap());
    let writer = WriterWrapper::new(pair.parent.take_writer().unwrap());
    let (tx, rx) = channel();

    read_from_shell(reader.clone(), tx.clone());
    pass_input_to_shell(writer.clone());

    let mut stdout = stdout().into_raw_mode().unwrap();
    write!(stdout, "{}{}", termion::clear::All, termion::cursor::Goto(1, 1)).unwrap();

    // capture initial terminal size to check for changes
    let (mut cols, mut rows) = terminal_size().unwrap();

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
            cols = c;
            rows = r;
            let _ = pair.parent.resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            });
        }
    }

    // try and wait for the child to complete
    if let Some(status) = pair.child.try_wait().unwrap() {
        println!("child status: {status}")
    } else {
        println!("killing child process because it took to long");
        pair.child.kill().unwrap(); // force kill it
    }

    // Take care to drop the master after our processes are
    // done, as some platforms get unhappy if it is dropped
    // sooner than that.
    drop(pair.parent);

    // let output = rx.recv().unwrap();
    // print!("output: ");
    // for c in output.escape_debug() {
    //     print!("{}", c);
    // }
}
