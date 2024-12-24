use libc::{self, ioctl, winsize, TIOCSWINSZ};
use std::{
    cell::Cell,
    io::{ErrorKind, Read, Write},
    os::fd::AsRawFd, thread, time::Duration,
};
use std::{io, os::unix::process::CommandExt};

use crate::fd::FileDescriptor;

pub struct PtySize {
    pub rows: u16,
    pub cols: u16,
    pub pixel_width: u16,
    pub pixel_height: u16,
}

pub struct Pty {
    controller: PtyController,
    child: std::process::Child,
}

struct PtyController {
    fd: FileDescriptor,
    writer_taken: Cell<bool>,
}

struct PtyWorker {
    fd: FileDescriptor,
}

impl Pty {
    pub fn open(cmd: std::process::Command) -> io::Result<Pty> {
        const FLAGS: i32 = libc::O_RDWR | libc::O_NOCTTY | libc::O_CLOEXEC;

        // open the master PTY with O_CLOEXEC
        let controller_fd = unsafe { libc::posix_openpt(FLAGS) };
        if controller_fd < 0 {
            return Err(io::Error::last_os_error());
        }

        // grant access to the worker PTY
        if unsafe { libc::grantpt(controller_fd) } != 0 {
            return Err(io::Error::last_os_error());
        }

        // unlock the worker PTY
        if unsafe { libc::unlockpt(controller_fd) } != 0 {
            return Err(io::Error::last_os_error());
        }

        // get the name of the worker PTY
        let worker_name_ptr = unsafe { libc::ptsname(controller_fd) };
        if worker_name_ptr.is_null() {
            return Err(io::Error::last_os_error());
        }

        // open the worker PTY with O_CLOEXEC
        let worker_fd = unsafe { libc::open(worker_name_ptr, FLAGS) };
        if worker_fd < 0 {
            return Err(io::Error::last_os_error());
        }

        // spawn the command, it will cleanup the worker fd when it goes out of scope
        // since it is only needed when spawning the command
        let worker = PtyWorker::new(FileDescriptor::new(worker_fd));
        let child = worker.spawn_command(cmd)?;

        Ok(Pty {
            controller: PtyController::new(FileDescriptor::new(controller_fd)),
            child,
        })
    }

    pub fn try_clone_reader(&self) -> io::Result<Box<dyn Read + Send>> {
        self.controller.try_clone_reader()
    }

    pub fn take_writer(&self) -> io::Result<Box<dyn Write + Send>> {
        self.controller.take_writer()
    }

    pub fn resize(&self, size: PtySize) -> io::Result<()> {
        self.controller.resize(size)
    }
}

impl Drop for Pty {
    fn drop(&mut self) {
        let _ = self.child.kill();
        // controller fd will drop itself
    }
}

impl PtyController {
    pub fn new(fd: FileDescriptor) -> Self {
        PtyController {
            fd,
            writer_taken: Cell::new(false),
        }
    }

    pub fn resize(&self, size: PtySize) -> io::Result<()> {
        let size = winsize {
            ws_row: size.rows,
            ws_col: size.cols,
            ws_xpixel: size.pixel_width,
            ws_ypixel: size.pixel_height,
        };

        let ret = unsafe { ioctl(self.fd.as_raw_fd(), TIOCSWINSZ, &size) };
        if ret < 0 {
            return Err(std::io::Error::last_os_error());
        }

        Ok(())
    }

    pub fn try_clone_reader(&self) -> io::Result<Box<dyn Read + Send>> {
        let fd = self.fd.duplicate()?;
        Ok(Box::new(fd))
    }

    pub fn take_writer(&self) -> io::Result<Box<dyn Write + Send>> {
        if self.writer_taken.get() {
            Err(io::Error::new(ErrorKind::Other, "writer already taken"))
        } else {
            let fd = self.fd.duplicate()?;
            self.writer_taken.set(true);
            Ok(Box::new(fd))
        }
    }
}

impl PtyWorker {
    pub fn new(fd: FileDescriptor) -> Self {
        PtyWorker { fd }
    }

    pub fn spawn_command(&self, mut cmd: std::process::Command) -> io::Result<std::process::Child> {
        // prepare command for spawning
        unsafe {
            cmd.stdin(self.fd.as_stdio()?)
                .stdout(self.fd.as_stdio()?)
                .stderr(self.fd.as_stdio()?)
                .pre_exec(Self::prepare_for_spawn)
        };

        // spawn the command
        let mut child = cmd.spawn()?;

        // close the child fds as we will use the controller fds
        child.stdin.take();
        child.stdout.take();
        child.stderr.take();

        Ok(child)
    }

    fn prepare_for_spawn() -> io::Result<()> {
        unsafe {
            // reset all signal handlers to default behavior
            for signo in &[
                libc::SIGCHLD,
                libc::SIGHUP,
                libc::SIGINT,
                libc::SIGQUIT,
                libc::SIGTERM,
                libc::SIGALRM,
            ] {
                libc::signal(*signo, libc::SIG_DFL);
            }

            // unmask all signals, unblocking them
            let empty_set: libc::sigset_t = std::mem::zeroed();
            libc::sigprocmask(libc::SIG_SETMASK, &empty_set, std::ptr::null_mut());

            // establish ourselves as a session leader.
            if libc::setsid() == -1 {
                return Err(io::Error::last_os_error());
            }

            // set the pty as the controlling terminal
            // #[allow(clippy::cast_lossless)]
            if libc::ioctl(0, libc::TIOCSCTTY as _, 0) == -1 {
                return Err(io::Error::last_os_error());
            }

            // closing all descriptors except for the stdio streams
            if let Ok(dir) = std::fs::read_dir("/dev/fd") {
                let mut fds = vec![];
                for entry in dir {
                    if let Some(num) = entry
                        .ok()
                        .map(|e| e.file_name())
                        .and_then(|s| s.into_string().ok())
                        .and_then(|n| n.parse::<libc::c_int>().ok())
                    {
                        if num > 2 {
                            fds.push(num);
                        }
                    }
                }
                for fd in fds {
                    libc::close(fd);
                }
            }
        }

        Ok(())
    }
}
