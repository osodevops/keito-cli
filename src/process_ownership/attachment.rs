//! Complete-or-absent cancellation attachment. This is never capture authority.
use std::{
    io,
    os::fd::{AsRawFd, FromRawFd},
    os::unix::net::UnixStream,
};
pub(super) const KEYS: [&str; 5] = [
    "KEITO_PROCESS_OWNER_FD",
    "KEITO_PROCESS_OWNER_NONCE",
    "KEITO_PROCESS_OWNER_PID",
    "KEITO_PROCESS_OWNER_END_NS",
    "KEITO_PROCESS_OWNER_CONTRACT",
];
pub(super) fn present() -> bool {
    KEYS.into_iter().chain(["KEITO_SOURCE_OWNER_FD", "KEITO_SOURCE_OWNER_NONCE"])
        .any(|key| std::env::var_os(key).is_some())
}
fn error() -> io::Error {
    io::Error::other("OWNER_ATTACHMENT")
}
pub(super) struct Attachment {
    pub nonce: String,
    pub parent: u32,
    pub end: u64,
}
fn number(value: &str) -> Option<u64> {
    let n = value.parse::<u64>().ok()?;
    (n > 0 && n.to_string() == value).then_some(n)
}
pub(super) fn parse(
    values: &[Option<String>; 5],
    parent: u32,
    at: u64,
    end: u64,
) -> io::Result<Option<Attachment>> {
    if values.iter().all(Option::is_none) {
        return Ok(None);
    }
    let values = values.each_ref().map(|v| v.as_deref().ok_or_else(error));
    let [fd, nonce, pid, inherited, contract] = values;
    let (fd, nonce, pid, inherited, contract) = (fd?, nonce?, pid?, inherited?, contract?);
    let inherited = number(inherited).filter(|n| *n > at).ok_or_else(error)?;
    if fd != "4"
        || contract != "2"
        || number(pid) != Some(u64::from(parent))
        || parent == 0
        || parent > i32::MAX as u32
        || nonce.len() != 32
        || !nonce
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(error());
    }
    Ok(Some(Attachment {
        nonce: nonce.into(),
        parent,
        end: end.min(inherited),
    }))
}
pub(super) fn consume(at: u64, end: u64) -> io::Result<Option<(Attachment, UnixStream)>> {
    // Legacy metadata is a partial/incompatible attachment, never ignored.
    if ["KEITO_SOURCE_OWNER_FD", "KEITO_SOURCE_OWNER_NONCE"]
        .iter()
        .any(|k| std::env::var_os(k).is_some())
    {
        return Err(error());
    }
    let mut values: [Option<String>; 5] = Default::default();
    for (i, key) in KEYS.iter().enumerate() {
        values[i] = match std::env::var(key) {
            Ok(v) => Some(v),
            Err(std::env::VarError::NotPresent) => None,
            Err(_) => return Err(error()),
        };
    }
    let Some(attachment) = parse(&values, unsafe { libc::getppid() } as u32, at, end)? else {
        return Ok(None);
    };
    let mut kind = 0;
    let mut length = std::mem::size_of::<i32>() as libc::socklen_t;
    let mut address = unsafe { std::mem::zeroed::<libc::sockaddr_storage>() };
    let mut size = std::mem::size_of_val(&address) as libc::socklen_t;
    if unsafe {
        libc::getsockopt(
            4,
            libc::SOL_SOCKET,
            libc::SO_TYPE,
            (&mut kind as *mut i32).cast(),
            &mut length,
        )
    } != 0
        || kind != libc::SOCK_STREAM
        || unsafe {
            libc::getsockname(
                4,
                (&mut address as *mut libc::sockaddr_storage).cast(),
                &mut size,
            )
        } != 0
        || i32::from(address.ss_family) != libc::AF_UNIX
    {
        return Err(error());
    }
    let stream = unsafe { UnixStream::from_raw_fd(4) };
    if unsafe { libc::fcntl(stream.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) } < 0 {
        return Err(error());
    }
    // Retain the tuple as a fail-closed sentinel for any convenience API that
    // has not received the explicit owner. It cannot consume this CLOEXEC FD.
    Ok(Some((attachment, stream)))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn r41_parent_tuple_is_complete_canonical_direct_and_cancellation_only() {
        let good =
            ["4", "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "42", "1000", "2"].map(|s| Some(s.into()));
        assert!(parse(&Default::default(), 42, 100, 900).unwrap().is_none());
        let a = parse(&good, 42, 100, 900).unwrap().unwrap();
        assert_eq!(a.end, 900);
        assert_eq!(a.parent, 42);
        for i in 0..5 {
            let mut partial = good.clone();
            partial[i] = None;
            assert!(parse(&partial, 42, 100, 900).is_err());
        }
        for (i, invalid) in [
            (0, "3"),
            (0, "04"),
            (1, "Aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
            (2, "43"),
            (2, "042"),
            (3, "100"),
            (3, "01000"),
            (4, "1"),
        ] {
            let mut bad = good.clone();
            bad[i] = Some(invalid.into());
            assert!(parse(&bad, 42, 100, 900).is_err());
        }
    }
}
