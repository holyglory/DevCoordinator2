use std::fs::{self, File};
use std::io;
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::path::{Path, PathBuf};
use std::time::Duration;

use rustix::fs::{FlockOperation, Mode, OFlags, flock, open};
use tokio::net::{UnixListener, UnixStream};

struct EndpointLease(File);

impl EndpointLease {
    fn acquire(path: &Path) -> io::Result<Self> {
        let mut name = path
            .file_name()
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "socket filename is missing")
            })?
            .to_os_string();
        name.push(".lock");
        let file = File::from(open(
            path.with_file_name(name),
            OFlags::RDWR | OFlags::CREATE | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::from_raw_mode(0o600),
        )?);
        if !file.metadata()?.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "endpoint lease is not a regular file",
            ));
        }
        flock(&file, FlockOperation::NonBlockingLockExclusive).map_err(|error| {
            if error == rustix::io::Errno::WOULDBLOCK {
                io::Error::new(
                    io::ErrorKind::AddrInUse,
                    "another daemon owns this endpoint",
                )
            } else {
                error.into()
            }
        })?;
        Ok(Self(file))
    }
}

impl Drop for EndpointLease {
    fn drop(&mut self) {
        let _ = flock(&self.0, FlockOperation::Unlock);
    }
}

pub(crate) struct SocketEndpoint {
    pub(crate) listener: UnixListener,
    path: PathBuf,
    identity: (u64, u64),
    _lease: EndpointLease,
}

impl SocketEndpoint {
    pub(crate) async fn bind(path: &Path) -> io::Result<Self> {
        let lease = EndpointLease::acquire(path)?;
        match fs::symlink_metadata(path) {
            Ok(metadata) if metadata.file_type().is_socket() => {
                match tokio::time::timeout(Duration::from_secs(1), UnixStream::connect(path)).await
                {
                    Ok(Err(error)) if error.kind() == io::ErrorKind::ConnectionRefused => {
                        let current = fs::symlink_metadata(path)?;
                        if (metadata.dev(), metadata.ino()) != (current.dev(), current.ino()) {
                            return Err(io::Error::new(
                                io::ErrorKind::AddrInUse,
                                "socket ownership changed during startup",
                            ));
                        }
                        fs::remove_file(path)?;
                    }
                    _ => {
                        return Err(io::Error::new(
                            io::ErrorKind::AddrInUse,
                            "another listener already uses this endpoint",
                        ));
                    }
                }
            }
            Ok(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "refusing to replace a non-socket endpoint",
                ));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        let listener = UnixListener::bind(path)?;
        let metadata = fs::symlink_metadata(path)?;
        Ok(Self {
            listener,
            path: path.to_owned(),
            identity: (metadata.dev(), metadata.ino()),
            _lease: lease,
        })
    }

    pub(crate) fn recover_missing(&mut self) -> io::Result<bool> {
        match fs::symlink_metadata(&self.path) {
            Ok(metadata)
                if metadata.file_type().is_socket()
                    && (metadata.dev(), metadata.ino()) == self.identity =>
            {
                Ok(false)
            }
            Ok(_) => Err(io::Error::new(
                io::ErrorKind::AddrInUse,
                "control endpoint was replaced; refusing to remove another owner",
            )),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let listener = UnixListener::bind(&self.path)?;
                let metadata = fs::symlink_metadata(&self.path)?;
                self.listener = listener;
                self.identity = (metadata.dev(), metadata.ino());
                Ok(true)
            }
            Err(error) => Err(error),
        }
    }
}

impl Drop for SocketEndpoint {
    fn drop(&mut self) {
        if let Ok(metadata) = fs::symlink_metadata(&self.path)
            && metadata.file_type().is_socket()
            && (metadata.dev(), metadata.ino()) == self.identity
        {
            let _ = fs::remove_file(&self.path);
        }
    }
}

#[cfg(target_os = "linux")]
pub(crate) struct SocketEvents(tokio::io::unix::AsyncFd<std::os::fd::OwnedFd>);

#[cfg(target_os = "linux")]
impl SocketEvents {
    pub(crate) fn new(parent: &Path) -> io::Result<Self> {
        use rustix::fs::inotify::{self, CreateFlags, WatchFlags};
        let descriptor = inotify::init(CreateFlags::CLOEXEC | CreateFlags::NONBLOCK)?;
        inotify::add_watch(
            &descriptor,
            parent,
            WatchFlags::CREATE
                | WatchFlags::DELETE
                | WatchFlags::MOVED_FROM
                | WatchFlags::MOVED_TO
                | WatchFlags::DELETE_SELF
                | WatchFlags::MOVE_SELF,
        )?;
        Ok(Self(tokio::io::unix::AsyncFd::new(descriptor)?))
    }

    pub(crate) async fn changed(&mut self) -> io::Result<()> {
        loop {
            let mut ready = self.0.readable().await?;
            match ready.try_io(|descriptor| {
                let mut buffer = [0_u8; 4096];
                rustix::io::read(descriptor.get_ref(), &mut buffer).map_err(io::Error::from)
            }) {
                Ok(Ok(0)) => {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "endpoint watcher closed",
                    ));
                }
                Ok(Ok(_)) => return Ok(()),
                Ok(Err(error)) => return Err(error),
                Err(_) => continue,
            }
        }
    }
}

#[cfg(not(target_os = "linux"))]
pub(crate) struct SocketEvents(tokio::time::Interval);

#[cfg(not(target_os = "linux"))]
impl SocketEvents {
    pub(crate) fn new(_parent: &Path) -> io::Result<Self> {
        Ok(Self(tokio::time::interval(Duration::from_millis(100))))
    }

    pub(crate) async fn changed(&mut self) -> io::Result<()> {
        self.0.tick().await;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn stale_socket_is_recovered_but_a_live_unleased_listener_is_preserved() {
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("daemon.sock");
        let live = UnixListener::bind(&path).unwrap();
        assert!(
            matches!(SocketEndpoint::bind(&path).await, Err(error) if error.kind() == io::ErrorKind::AddrInUse)
        );
        assert!(UnixStream::connect(&path).await.is_ok());
        drop(live);
        let endpoint = SocketEndpoint::bind(&path)
            .await
            .expect("stale endpoint recovery");
        assert!(UnixStream::connect(&path).await.is_ok());
        drop(endpoint);
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn cleanup_never_removes_a_replacement_socket_or_regular_file() {
        for replacement_is_socket in [true, false] {
            let temporary = tempfile::tempdir().unwrap();
            let path = temporary.path().join("daemon.sock");
            let mut endpoint = SocketEndpoint::bind(&path).await.unwrap();
            fs::remove_file(&path).unwrap();
            let replacement = if replacement_is_socket {
                Some(UnixListener::bind(&path).unwrap())
            } else {
                fs::write(&path, b"preserve").unwrap();
                None
            };
            assert_eq!(
                endpoint.recover_missing().unwrap_err().kind(),
                io::ErrorKind::AddrInUse
            );
            drop(endpoint);
            assert!(path.exists());
            if replacement.is_some() {
                assert!(UnixStream::connect(&path).await.is_ok());
            } else {
                assert_eq!(fs::read(&path).unwrap(), b"preserve");
            }
        }
    }

    #[tokio::test]
    async fn startup_preserves_non_socket_paths_and_symlink_targets() {
        for use_symlink in [false, true] {
            let temporary = tempfile::tempdir().unwrap();
            let path = temporary.path().join("daemon.sock");
            let target = temporary.path().join("keep");
            fs::write(&target, b"preserve").unwrap();
            if use_symlink {
                std::os::unix::fs::symlink(&target, &path).unwrap();
            } else {
                fs::write(&path, b"preserve").unwrap();
            }
            assert!(matches!(SocketEndpoint::bind(&path).await, Err(error)
                if error.kind() == io::ErrorKind::AlreadyExists));
            assert_eq!(fs::read(&target).unwrap(), b"preserve");
            assert_eq!(fs::read(&path).unwrap(), b"preserve");
            assert_eq!(
                fs::symlink_metadata(&path)
                    .unwrap()
                    .file_type()
                    .is_symlink(),
                use_symlink
            );
        }
    }
}
