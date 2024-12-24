extern crate termion;

pub mod fd;
pub mod pty;

use pty::Pty;
use std::io::{self, stdin, stdout, Read, Write};
use std::{sync::mpsc::channel, thread};
use termion::{clear, cursor};
use termion::{raw::IntoRawMode, terminal_size};

fn run() -> io::Result<()> {
    // setup
    let (tx, rx) = channel();
    let (exit_tx, exit_rx) = channel();
    let pty = Pty::open(std::process::Command::new("zsh"))?;
    let mut reader = pty.try_clone_reader()?;
    let mut writer = pty.take_writer()?;

    // resize the client
    let (mut cols, mut rows) = terminal_size().unwrap();
    pty.resize(rows, cols).unwrap();

    // pipe pty output to tx
    let exit_tx_clone = exit_tx.clone();
    thread::spawn(move || {
        let mut buf = [0u8; 128 * 128]; // at least rows * cols buffer size
        loop {
            match reader.read(&mut buf) {
                Ok(bytes_read) => {
                    if bytes_read == 0 {
                        eprintln!("worker pty has been closed, terminating.");
                        break;
                    }

                    let data = buf[..bytes_read].to_vec();
                    if let Err(e) = tx.send(data) {
                        eprintln!("failed to pipe pty output to tx: {}", e);
                        break;
                    }
                }
                Err(e) => {
                    eprintln!("failed to read pty output: {}", e);
                    break;
                }
            }
        }
        let _ = exit_tx_clone.send(0);
    });

    // pipe stdin to pty
    let exit_tx_clone = exit_tx.clone();
    thread::spawn(move || {
        let mut stdin = stdin().lock();
        let mut buf = [0u8; 128]; // at least one row at a time
        loop {
            match stdin.read(&mut buf) {
                Ok(bytes_read) => {
                    if bytes_read == 0 {
                        eprintln!("stdin has been closed, terminating.");
                        break;
                    }

                    if let Err(e) = writer.write(&buf[..bytes_read]) {
                        eprintln!("failed to pipe stdin to pty: {}", e);
                        break;
                    }
                }
                Err(e) => {
                    eprintln!("failed to read stdin: {}", e);
                    break;
                }
            }
        }
        let _ = exit_tx_clone.send(1);
    });

    // pipe rx to stdout
    let exit_tx_clone = exit_tx.clone();
    thread::spawn(move || {
        let mut stdout = stdout().into_raw_mode().unwrap();
        write!(stdout, "{}{}", clear::All, cursor::Goto(1, 1)).unwrap();

        loop {
            match rx.recv() {
                Ok(output) => match std::str::from_utf8(&output) {
                    Ok(str) => {
                        if let Err(e) = write!(stdout, "{}", str) {
                            eprintln!("failed to pipe rx to stdout: {}", e);
                            break;
                        }
                        if let Err(e) = stdout.flush() {
                            eprintln!("failed to flush rx to stdout: {}", e);
                            break;
                        }
                    }
                    Err(e) => {
                        eprintln!("failed to parse utf8: {}", e);
                        break;
                    }
                },
                Err(e) => {
                    eprintln!("failed to read rx: {}", e);
                    break;
                }
            }

            if let Ok((c, r)) = terminal_size() {
                if c != cols || r != rows {
                    (rows, cols) = (r, c);
                    let _ = pty.resize(rows, cols); // ignore resize failures
                }
            }
        }

        let _ = exit_tx_clone.send(2);
    });

    // wait until any of these threads terminate
    exit_rx.recv().unwrap();

    Ok(())
}

fn main() {
    run().unwrap()
}
