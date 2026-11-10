//! Bounded transient pipes. Only read(0) establishes output EOF.
//! Buffers deliberately have no Debug/Serialize implementation.
use std::{
    io::{self, Read, Write},
    os::fd::AsRawFd,
    process::{ChildStderr, ChildStdin, ChildStdout},
};

pub const MAX_STREAM_BYTES: usize = 1024 * 1024;
const TURN_BYTES: usize = 4096;

pub struct IoPlan {
    input: Vec<u8>,
    stdout_cap: usize,
    stderr_cap: usize,
}
impl IoPlan {
    pub fn new(input: Vec<u8>, stdout_cap: usize, stderr_cap: usize) -> io::Result<Self> {
        if input.len() > MAX_STREAM_BYTES
            || stdout_cap > MAX_STREAM_BYTES
            || stderr_cap > MAX_STREAM_BYTES
        {
            return Err(io::Error::other("IO_LIMIT"));
        }
        Ok(Self { input, stdout_cap, stderr_cap })
    }
    pub(super) fn capacity(&self) -> usize {
        self.input.len() + self.stdout_cap + self.stderr_cap
    }
    pub(super) fn prepare(self) -> io::Result<PipeIo> {
        Ok(PipeIo {
            input: Input { bytes: self.input, offset: 0, closed: false },
            stdout: Output::new(self.stdout_cap)?,
            stderr: Output::new(self.stderr_cap)?,
            stdin_pipe: None,
            stdout_pipe: None,
            stderr_pipe: None,
            fault: None,
        })
    }
}

struct Input {
    bytes: Vec<u8>,
    offset: usize,
    closed: bool,
}
impl Input {
    fn poll(&mut self, writer: &mut impl Write) -> Option<&'static str> {
        if self.closed { return None; }
        if self.offset == self.bytes.len() {
            self.closed = true;
            return None;
        }
        let end = (self.offset + TURN_BYTES).min(self.bytes.len());
        match writer.write(&self.bytes[self.offset..end]) {
            Ok(0) => { self.closed = true; Some("IO_STDIN_ZERO") }
            Ok(count) if count <= end - self.offset => {
                self.offset += count;
                if self.offset == self.bytes.len() { self.closed = true; }
                None
            }
            Ok(_) => { self.closed = true; Some("IO_STDIN_WRITE") }
            Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted) => None,
            Err(_) => { self.closed = true; Some("IO_STDIN_WRITE") }
        }
    }
}
struct Output {
    bytes: Vec<u8>,
    cap: usize,
    count: usize,
    eof: bool,
    fault: Option<&'static str>,
}
impl Output {
    fn new(cap: usize) -> io::Result<Self> {
        let mut bytes = Vec::new();
        bytes.try_reserve_exact(cap).map_err(|_| io::Error::other("IO_ALLOCATION"))?;
        Ok(Self { bytes, cap, count: 0, eof: false, fault: None })
    }
    fn poll(&mut self, reader: &mut impl Read) -> Option<&'static str> {
        if self.eof { return self.fault; }
        let mut scratch = [0u8; TURN_BYTES];
        match reader.read(&mut scratch) {
            Ok(0) => self.eof = true,
            Ok(count) if count <= scratch.len() => {
                self.count = self.count.saturating_add(count);
                let room = self.cap - self.bytes.len();
                self.bytes.extend_from_slice(&scratch[..count.min(room)]);
                if count > room { self.fault.get_or_insert("IO_OUTPUT_LIMIT"); }
            }
            Ok(_) => { self.fault.get_or_insert("IO_OUTPUT_READ"); }
            Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted) => {}
            Err(_) => { self.fault.get_or_insert("IO_OUTPUT_READ"); }
        }
        self.fault
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct IoFacts {
    pub stdin_closed: bool,
    pub stdout_eof: bool,
    pub stderr_eof: bool,
    pub stdout_bytes: usize,
    pub stderr_bytes: usize,
    pub first_fault: Option<&'static str>,
}
impl IoFacts {
    pub fn complete(&self) -> bool {
        self.stdin_closed && self.stdout_eof && self.stderr_eof
    }
}

pub struct CommandOutput {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}
pub(super) struct PipeIo {
    input: Input,
    stdout: Output,
    stderr: Output,
    stdin_pipe: Option<ChildStdin>,
    stdout_pipe: Option<ChildStdout>,
    stderr_pipe: Option<ChildStderr>,
    fault: Option<&'static str>,
}
fn flags(pipe: &impl AsRawFd) -> io::Result<()> {
    let fd = pipe.as_raw_fd();
    let old = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if old < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, old | libc::O_NONBLOCK) } < 0 {
        return Err(io::Error::other("IO_FLAGS"));
    }
    let old = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if old < 0 || unsafe { libc::fcntl(fd, libc::F_SETFD, old | libc::FD_CLOEXEC) } < 0 {
        return Err(io::Error::other("IO_FLAGS"));
    }
    Ok(())
}
impl PipeIo {
    // Move ALL handles in before any fallible flags operation. Failure leaves
    // them retained; callers must continue collection under their original end.
    pub(super) fn attach(&mut self, input: Option<ChildStdin>, output: Option<ChildStdout>, error: Option<ChildStderr>) -> io::Result<()> {
        self.stdin_pipe = input;
        self.stdout_pipe = output;
        self.stderr_pipe = error;
        let mut failed = false;
        if let Some(pipe) = &self.stdin_pipe { failed |= flags(pipe).is_err(); } else { failed = true; }
        if let Some(pipe) = &self.stdout_pipe { failed |= flags(pipe).is_err(); } else { failed = true; }
        if let Some(pipe) = &self.stderr_pipe { failed |= flags(pipe).is_err(); } else { failed = true; }
        if failed {
            self.fault.get_or_insert("IO_FLAGS");
            self.cancel_input();
            // Failed descriptors remain retained, but MUST NOT be read if their
            // nonblocking status is unknown. EOF therefore remains unknown.
            return Err(io::Error::other("IO_FLAGS"));
        }
        Ok(())
    }
    pub(super) fn cancel_input(&mut self) {
        self.stdin_pipe.take();
        self.input.closed = true;
    }
    pub(super) fn poll(&mut self) {
        if self.fault == Some("IO_FLAGS") { return; }
        if let Some(pipe) = &mut self.stdin_pipe {
            if let Some(reason) = self.input.poll(pipe) { self.fault.get_or_insert(reason); }
            if self.input.closed { self.stdin_pipe.take(); }
        }
        if let Some(pipe) = &mut self.stdout_pipe {
            if let Some(reason) = self.stdout.poll(pipe) { self.fault.get_or_insert(reason); }
        }
        if let Some(pipe) = &mut self.stderr_pipe {
            if let Some(reason) = self.stderr.poll(pipe) { self.fault.get_or_insert(reason); }
        }
    }
    pub(super) fn facts(&self) -> IoFacts {
        IoFacts { stdin_closed: self.input.closed, stdout_eof: self.stdout.eof, stderr_eof: self.stderr.eof,
            stdout_bytes: self.stdout.count, stderr_bytes: self.stderr.count, first_fault: self.fault }
    }
    pub(super) fn take_output(&mut self) -> CommandOutput {
        CommandOutput { stdout: std::mem::take(&mut self.stdout.bytes), stderr: std::mem::take(&mut self.stderr.bytes) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    enum ReadStep { Data(Vec<u8>), Block, Interrupt, Fail, Eof }
    struct Reader { steps: VecDeque<ReadStep>, calls: usize }
    impl Read for Reader {
        fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
            self.calls += 1;
            match self.steps.pop_front().unwrap_or(ReadStep::Block) {
                ReadStep::Data(bytes) => { assert!(bytes.len() <= out.len()); out[..bytes.len()].copy_from_slice(&bytes); Ok(bytes.len()) }
                ReadStep::Block => Err(io::ErrorKind::WouldBlock.into()),
                ReadStep::Interrupt => Err(io::ErrorKind::Interrupted.into()),
                ReadStep::Fail => Err(io::ErrorKind::BrokenPipe.into()),
                ReadStep::Eof => Ok(0),
            }
        }
    }
    fn reader(steps: Vec<ReadStep>) -> Reader { Reader { steps: steps.into(), calls: 0 } }
    #[test]
    fn output_requires_read_zero_not_wouldblock_interruption_or_error() {
        let mut output = Output::new(8).unwrap();
        let mut input = reader(vec![ReadStep::Block, ReadStep::Interrupt, ReadStep::Fail, ReadStep::Eof]);
        for _ in 0..3 { output.poll(&mut input); assert!(!output.eof); }
        output.poll(&mut input);
        assert!(output.eof);
        assert_eq!(output.fault, Some("IO_OUTPUT_READ"));
        assert_eq!(input.calls, 4);
    }
    #[test]
    fn exact_cap_is_valid_cap_plus_one_remains_failed_after_drain_and_eof() {
        let mut output = Output::new(3).unwrap();
        let mut input = reader(vec![ReadStep::Data(b"abc".to_vec()), ReadStep::Block, ReadStep::Data(b"d".to_vec()), ReadStep::Data(vec![0; TURN_BYTES]), ReadStep::Eof]);
        output.poll(&mut input); assert_eq!(output.fault, None); assert!(!output.eof);
        output.poll(&mut input); assert!(!output.eof);
        output.poll(&mut input); assert_eq!(output.fault, Some("IO_OUTPUT_LIMIT"));
        output.poll(&mut input); output.poll(&mut input);
        assert!(output.eof); assert_eq!(output.bytes, b"abc"); assert_eq!(output.count, TURN_BYTES + 4);
        assert_eq!(output.fault, Some("IO_OUTPUT_LIMIT"));
    }
    struct Writer { bytes: Vec<u8>, block: bool, fail: bool, calls: usize }
    impl Write for Writer {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.calls += 1;
            if self.fail { return Err(io::ErrorKind::BrokenPipe.into()); }
            if self.block { self.block = false; return Err(io::ErrorKind::WouldBlock.into()); }
            let count = bytes.len().min(2); self.bytes.extend_from_slice(&bytes[..count]); Ok(count)
        }
        fn flush(&mut self) -> io::Result<()> { panic!("no blocking flush") }
    }
    #[test]
    fn stdin_partial_backpressure_and_epipe_preserve_offset_and_close() {
        let mut state = Input { bytes: b"abcde".to_vec(), offset: 0, closed: false };
        let mut writer = Writer { bytes: vec![], block: true, fail: false, calls: 0 };
        assert_eq!(state.poll(&mut writer), None); assert_eq!(state.offset, 0);
        assert_eq!(state.poll(&mut writer), None); assert_eq!(state.offset, 2);
        writer.fail = true;
        assert_eq!(state.poll(&mut writer), Some("IO_STDIN_WRITE")); assert!(state.closed);
        state.poll(&mut writer); assert_eq!(writer.calls, 3); assert_eq!(writer.bytes, b"ab");
    }
    #[test]
    fn input_completion_and_cancellation_are_separate_from_output_eof() {
        let mut io = IoPlan::new(vec![], 0, 0).unwrap().prepare().unwrap();
        io.cancel_input();
        assert!(io.facts().stdin_closed); assert!(!io.facts().complete());
        assert!(!io.facts().stdout_eof); assert!(!io.facts().stderr_eof);
        assert!(IoPlan::new(vec![], MAX_STREAM_BYTES+1, 0).is_err());
    }
    #[test]
    fn each_output_turn_is_one_bounded_read_and_zero_cap_still_needs_eof() {
        let mut output = Output::new(0).unwrap();
        let mut input = reader(vec![ReadStep::Data(vec![0; TURN_BYTES]), ReadStep::Eof]);
        output.poll(&mut input); assert_eq!(input.calls, 1); assert!(!output.eof);
        assert_eq!(output.bytes.len(), 0); assert_eq!(output.fault, Some("IO_OUTPUT_LIMIT"));
        output.poll(&mut input); assert!(output.eof);
    }
}
