//! One local owner endpoint: a loopback socket and a private capability record.
//! Owner-only modes/ACLs and handle-relative access are provided by private-fs;
//! no filesystem permissions are widened or repaired when acquiring an existing endpoint.
//! The safe exclusive socket option is upstream socket2 commit
//! 8b8b96fdaa1301698fd2b6f4de188635df4a8bd1 (github.com/rust-lang/socket2/pull/678).
use super::wire::CONNECTION_MAX;
use amphora_api::{self as api, Refusal};
use private_fs::{FileAccess, FileCreation, OpenOptions, PrivateDir, PrivateFile};
use socket2::{Domain, Protocol, Socket, Type};
use std::{
    fs::File,
    io::Read,
    net::{Ipv4Addr, SocketAddr, SocketAddrV4},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

#[cfg(unix)]
use std::os::fd::AsFd;
#[cfg(windows)]
use std::os::windows::io::AsHandle;

pub(super) type Stream = std::net::TcpStream;
pub(super) type Listener = std::net::TcpListener;

const RECORD_BYTES: usize = 34;
const RECORD_NAME: &str = "owner";

pub(super) struct Guard {
    // Keep the validated directory and both handles alive until the shared owner stops.
    // The record is separate: Windows exclusive byte-range locks also prohibit peer reads.
    _directory: PrivateDir,
    _private_lock: PrivateFile,
    _lock: File,
}

pub(super) struct Bound {
    pub listener: Listener,
    pub token: [u8; 32],
    pub guard: Guard,
}

fn unavailable(error: impl std::fmt::Display) -> Refusal {
    Refusal::Unavailable(error.to_string())
}

fn directory(path: &Path) -> Result<PathBuf, Refusal> {
    let canonical = path.canonicalize().map_err(unavailable)?;
    for (depth, _) in canonical.components().enumerate() {
        api::bound("database directory depth", depth, 256)?;
    }
    let mut sidecar = canonical.into_os_string();
    sidecar.push(".client");
    Ok(PathBuf::from(sidecar))
}

fn remaining(until: Instant) -> Result<Duration, Refusal> {
    let left = until.saturating_duration_since(Instant::now());
    if left.is_zero() {
        Err(Refusal::Timeout)
    } else {
        Ok(left)
    }
}

#[cfg(unix)]
fn clone_lock(file: &PrivateFile) -> Result<File, Refusal> {
    file.as_fd()
        .try_clone_to_owned()
        .map(File::from)
        .map_err(unavailable)
}

#[cfg(windows)]
fn clone_lock(file: &PrivateFile) -> Result<File, Refusal> {
    file.as_handle()
        .try_clone_to_owned()
        .map(File::from)
        .map_err(unavailable)
}

pub(super) fn bind(path: &Path) -> Result<Bound, Refusal> {
    let directory = PrivateDir::ensure(directory(path)?).map_err(unavailable)?;
    let private_lock = directory
        .open_file_with(
            "lock",
            OpenOptions {
                access: FileAccess::ReadWrite,
                creation: FileCreation::OpenOrCreate,
                ..OpenOptions::default()
            },
        )
        .map_err(unavailable)?;
    let lock = clone_lock(&private_lock)?;
    lock.try_lock().map_err(unavailable)?;

    let socket =
        Socket::new(Domain::IPV4, Type::STREAM, Some(Protocol::TCP)).map_err(unavailable)?;
    // SO_REUSEADDR can otherwise redirect Windows connections to another local process.
    // Microsoft: learn.microsoft.com/windows/win32/winsock/using-so-reuseaddr-and-so-exclusiveaddruse.
    #[cfg(windows)]
    socket.set_exclusive_address(true).map_err(unavailable)?;
    // Fresh Unix sockets retain the OS defaults: SO_REUSEADDR and SO_REUSEPORT are disabled.
    socket
        .bind(&SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0).into())
        .map_err(unavailable)?;
    socket
        .listen(i32::try_from(CONNECTION_MAX).map_err(unavailable)?)
        .map_err(unavailable)?;
    socket.set_nonblocking(true).map_err(unavailable)?;
    let listener = Listener::from(socket);
    let mut token = [0; 32];
    getrandom::fill(&mut token).map_err(unavailable)?;
    let mut record = Vec::with_capacity(RECORD_BYTES);
    record.extend_from_slice(
        &listener
            .local_addr()
            .map_err(unavailable)?
            .port()
            .to_be_bytes(),
    );
    record.extend_from_slice(&token);
    directory
        .write_atomic(RECORD_NAME, &record)
        .map_err(unavailable)?;
    Ok(Bound {
        listener,
        token,
        guard: Guard {
            _directory: directory,
            _private_lock: private_lock,
            _lock: lock,
        },
    })
}

pub(super) fn connect(path: &Path, until: Instant) -> Result<(Stream, [u8; 32]), Refusal> {
    remaining(until)?;
    let directory = PrivateDir::open(directory(path)?).map_err(unavailable)?;
    let mut file = directory.open_file(RECORD_NAME).map_err(unavailable)?;
    let mut record = [0; RECORD_BYTES];
    file.read_exact(&mut record).map_err(unavailable)?;
    // A fixed record and one-byte EOF check bound allocation and reject trailing data.
    let mut extra = [0; 1];
    if file.read(&mut extra).map_err(unavailable)? != 0 {
        return Err(api::invalid(
            "owner endpoint record",
            "unexpected trailing bytes",
        ));
    }
    let [first, second, token @ ..] = record;
    let port = u16::from_be_bytes([first, second]);
    if port == 0 {
        return Err(api::invalid(
            "owner endpoint record",
            "missing loopback port",
        ));
    }
    let address = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port));
    let stream = Stream::connect_timeout(&address, remaining(until)?).map_err(|error| {
        if matches!(
            error.kind(),
            std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
        ) {
            Refusal::Timeout
        } else {
            unavailable(error)
        }
    })?;
    stream.set_nodelay(true).map_err(unavailable)?;
    Ok((stream, token))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    #[cfg(windows)]
    use std::{
        process::{Command, Stdio},
        thread,
    };

    fn database() -> (tempfile::TempDir, PathBuf) {
        let temporary = tempfile::tempdir().unwrap();
        let database = temporary.path().join("database");
        File::create(&database).unwrap();
        (temporary, database)
    }

    #[cfg(windows)]
    fn grant_world_read(path: &Path) {
        // Use the vendor ACL tool through an absolute path, without a shell or token output.
        let executable = PathBuf::from(std::env::var_os("SystemRoot").unwrap())
            .join("System32")
            .join("icacls.exe");
        let mut child = Command::new(executable)
            .arg(path)
            .args(["/grant", "*S-1-1-0:R", "/Q"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let until = Instant::now() + Duration::from_secs(5);
        while Instant::now() < until {
            if let Some(status) = child.try_wait().unwrap() {
                assert!(status.success());
                return;
            }
            thread::park_timeout(Duration::from_millis(1));
        }
        child.kill().unwrap();
        child.wait().unwrap();
        panic!("test ACL tool exceeded its deadline");
    }

    #[test]
    fn exclusive_owner_record_is_readable_and_endpoint_exchanges_bytes() {
        // CP-T1.5/U16: independent handles share one local endpoint; Windows locks must not
        // prevent the client reading its separate, protected capability record.
        let (_temporary, database) = database();
        let owner = bind(&database).unwrap();
        assert!(bind(&database).is_err());
        let (mut client, token) =
            connect(&database, Instant::now() + Duration::from_secs(5)).unwrap();
        assert!(token == owner.token);
        let (mut server, address) = owner.listener.accept().unwrap();
        assert!(address.ip().is_loopback());
        server.set_nonblocking(false).unwrap();
        server
            .set_write_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        server.write_all(b"owner").unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut bytes = [0; 5];
        client.read_exact(&mut bytes).unwrap();
        assert_eq!(&bytes, b"owner");
        drop(client);
        drop(server);
        drop(owner);
        assert!(bind(&database).is_ok());
    }

    #[test]
    fn endpoint_record_rejects_short_oversized_and_zero_port_data() {
        // CP-T1.5/U17: the private record parser has a fixed allocation and exact wire length.
        let (_temporary, database) = database();
        let owner = bind(&database).unwrap();
        for record in [vec![0; 33], vec![0; 35], vec![0; RECORD_BYTES]] {
            owner
                .guard
                ._directory
                .write_atomic(RECORD_NAME, &record)
                .unwrap();
            assert!(connect(&database, Instant::now() + Duration::from_secs(5)).is_err());
        }
        assert!(matches!(
            connect(&database, Instant::now()),
            Err(Refusal::Timeout)
        ));
    }

    #[cfg(unix)]
    #[test]
    fn broadened_record_or_directory_permissions_refuse_access_without_repair() {
        // CP-T1.5/U17: group/other access must prevent transport acquisition; an existing
        // endpoint's permissions are never silently repaired during owner selection.
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let (_temporary, database) = database();
        let owner = bind(&database).unwrap();
        let (client, _) = connect(&database, Instant::now() + Duration::from_secs(5)).unwrap();
        drop(client);
        let sidecar = directory(&database).unwrap();
        let record = sidecar.join(RECORD_NAME);
        std::fs::set_permissions(&record, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(owner.guard._directory.open_file(RECORD_NAME).is_err());
        assert!(connect(&database, Instant::now() + Duration::from_secs(5)).is_err());
        assert_eq!(std::fs::metadata(&record).unwrap().mode() & 0o777, 0o644);
        drop(owner);

        std::fs::set_permissions(&record, std::fs::Permissions::from_mode(0o600)).unwrap();
        std::fs::set_permissions(&sidecar, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(bind(&database).is_err());
        assert_eq!(std::fs::metadata(&sidecar).unwrap().mode() & 0o777, 0o755);
    }

    #[cfg(windows)]
    #[test]
    fn exclusive_socket_refuses_a_reuse_address_interceptor() {
        // CP-T1.5/U17: a different local principal cannot intercept authenticated SQL traffic
        // by forcing a Windows SO_REUSEADDR listener onto the advertised port.
        let (_temporary, database) = database();
        let owner = bind(&database).unwrap();
        let interceptor = Socket::new(Domain::IPV4, Type::STREAM, Some(Protocol::TCP)).unwrap();
        interceptor.set_reuse_address(true).unwrap();
        assert!(
            interceptor
                .bind(&owner.listener.local_addr().unwrap().into())
                .is_err()
        );
    }

    #[cfg(windows)]
    #[test]
    fn modified_record_or_directory_acl_refuses_access_without_repair() {
        // CP-T1.5/U17: adding an actual Everyone grant must prevent transport acquisition;
        // the endpoint never widens or silently repairs an existing object's permissions.
        let (_temporary, database) = database();
        let owner = bind(&database).unwrap();
        let (client, _) = connect(&database, Instant::now() + Duration::from_secs(5)).unwrap();
        drop(client);
        let sidecar = directory(&database).unwrap();
        grant_world_read(&sidecar.join(RECORD_NAME));
        assert!(matches!(
            owner.guard._directory.open_file(RECORD_NAME),
            Err(private_fs::Error::InsecureAcl)
        ));
        assert!(connect(&database, Instant::now() + Duration::from_secs(5)).is_err());
        drop(owner);

        grant_world_read(&sidecar);
        assert!(matches!(
            PrivateDir::open(&sidecar),
            Err(private_fs::Error::InsecureAcl)
        ));
        assert!(bind(&database).is_err());
        assert!(matches!(
            PrivateDir::open(&sidecar),
            Err(private_fs::Error::InsecureAcl)
        ));
    }
}
