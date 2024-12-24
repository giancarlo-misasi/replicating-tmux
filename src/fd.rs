use std::{
    io::{self, Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd, IntoRawFd},
        unix::io::RawFd,
    },
};

pub struct FileDescriptor {
    fd: RawFd,
}

impl FileDescriptor {
    pub fn new(fd: RawFd) -> Self {
        Self { fd }
    }

    pub fn as_stdio(&self) -> io::Result<std::process::Stdio> {
        let duped = self.duplicate()?;
        let fd = duped.into_raw_fd();
        let stdio = unsafe { std::process::Stdio::from_raw_fd(fd) };
        Ok(stdio)
    }

    pub fn duplicate(&self) -> io::Result<Self> {
        let fd = self.fd.as_raw_fd();
        let duped = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 0) };
        if duped == -1 {
            return Err(io::Error::last_os_error());
        }
        Ok(FileDescriptor::new(duped))
    }
}

impl AsRawFd for FileDescriptor {
    fn as_raw_fd(&self) -> RawFd {
        self.fd
    }
}

impl IntoRawFd for FileDescriptor {
    fn into_raw_fd(self) -> RawFd {
        let fd = self.fd;
        std::mem::forget(self);
        fd
    }
}

impl Drop for FileDescriptor {
    fn drop(&mut self) {
        let err = unsafe { libc::close(self.fd) };
        if err != 0 {
            eprintln!(
                "Failed to close file descriptor {}: {:?}",
                self.fd,
                io::Error::last_os_error()
            );
        }
    }
}

impl Read for FileDescriptor {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, io::Error> {
        let size = unsafe { libc::read(self.fd, buf.as_mut_ptr() as *mut _, buf.len()) };
        if size == -1 {
            let e = std::io::Error::last_os_error();
            if e.raw_os_error() == Some(libc::EIO) {
                // EIO indicates that the worker pty has been closed.
                // Treat this as EOF so that std::io::Read::read_to_string
                // and similar functions gracefully terminate when they
                // encounter this condition.
                Ok(0)
            } else {
                Err(e)
            }
        } else {
            Ok(size as usize)
        }
    }
}

impl Write for FileDescriptor {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let size = unsafe { libc::write(self.fd, buf.as_ptr() as *const _, buf.len()) };
        if size == -1 {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(size as usize)
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
