//! Interruptible relay-worker readiness waits.

use std::{
    io::{self, Read, Write},
    os::fd::{AsFd, BorrowedFd},
    os::unix::net::UnixStream,
    sync::{Arc, Mutex},
    time::Duration,
};

use nix::poll::{PollFd, PollFlags, PollTimeout, poll};

use crate::RelayPortError;

#[derive(Clone, Debug)]
pub(crate) struct WorkerWake {
    writer: Arc<Mutex<UnixStream>>,
}

#[derive(Debug)]
pub(crate) struct WorkerWaiter {
    reader: UnixStream,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum WorkerReady {
    Wake,
    Connection,
    Deadline,
}

pub(crate) fn worker_readiness() -> Result<(WorkerWake, WorkerWaiter), RelayPortError> {
    let (reader, writer) = UnixStream::pair().map_err(|_| RelayPortError::Unavailable)?;
    reader
        .set_nonblocking(true)
        .and_then(|()| writer.set_nonblocking(true))
        .map_err(|_| RelayPortError::Unavailable)?;
    Ok((
        WorkerWake {
            writer: Arc::new(Mutex::new(writer)),
        },
        WorkerWaiter { reader },
    ))
}

impl WorkerWake {
    pub(crate) fn signal(&self) -> Result<(), RelayPortError> {
        let mut writer = self
            .writer
            .lock()
            .map_err(|_| RelayPortError::Unavailable)?;
        match writer.write(&[1]) {
            Ok(_) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => Ok(()),
            Err(_) => Err(RelayPortError::Unavailable),
        }
    }
}

impl WorkerWaiter {
    pub(crate) fn wait(
        &mut self,
        connection: Option<BorrowedFd<'_>>,
        timeout: Option<Duration>,
    ) -> Result<WorkerReady, RelayPortError> {
        let timeout = timeout
            .map_or(Ok(PollTimeout::NONE), PollTimeout::try_from)
            .map_err(|_| RelayPortError::InvalidInput)?;
        let mut descriptors = Vec::with_capacity(2);
        descriptors.push(PollFd::new(
            self.reader.as_fd(),
            PollFlags::POLLIN | PollFlags::POLLHUP | PollFlags::POLLERR,
        ));
        if let Some(connection) = connection {
            descriptors.push(PollFd::new(
                connection,
                PollFlags::POLLIN | PollFlags::POLLHUP | PollFlags::POLLERR,
            ));
        }
        if poll(&mut descriptors, timeout).map_err(|_| RelayPortError::Unavailable)? == 0 {
            return Ok(WorkerReady::Deadline);
        }
        if descriptors[0].revents().is_some_and(|events| {
            events.intersects(PollFlags::POLLIN | PollFlags::POLLHUP | PollFlags::POLLERR)
        }) {
            self.drain()?;
            return Ok(WorkerReady::Wake);
        }
        Ok(WorkerReady::Connection)
    }

    fn drain(&mut self) -> Result<(), RelayPortError> {
        let mut bytes = [0_u8; 64];
        let mut received = false;
        loop {
            match self.reader.read(&mut bytes) {
                Ok(0) if received => return Ok(()),
                Ok(0) => return Err(RelayPortError::Unavailable),
                Ok(_) => received = true,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(()),
                Err(_) => return Err(RelayPortError::Unavailable),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use std::{
        io::Write,
        os::fd::AsFd,
        os::unix::net::UnixStream,
        thread,
        time::{Duration, Instant},
    };

    use super::*;

    #[test]
    fn wake_racing_wait_registration_is_observed_and_bursts_coalesce() {
        let (wake, mut waiter) = worker_readiness().expect("readiness constructs");
        for _ in 0..32 {
            wake.signal().expect("wake publishes");
        }
        assert_eq!(
            waiter.wait(None, Some(Duration::from_secs(1))),
            Ok(WorkerReady::Wake)
        );
        assert_eq!(
            waiter.wait(None, Some(Duration::ZERO)),
            Ok(WorkerReady::Deadline),
            "the sole waiter drains a coalesced burst"
        );
    }

    #[test]
    fn connection_readiness_and_exact_deadline_are_distinct() {
        let (_wake, mut waiter) = worker_readiness().expect("readiness constructs");
        let (reader, mut writer) = UnixStream::pair().expect("connection pair constructs");
        writer.write_all(&[1]).expect("connection becomes ready");
        assert_eq!(
            waiter.wait(Some(reader.as_fd()), Some(Duration::from_secs(1))),
            Ok(WorkerReady::Connection)
        );

        let started = Instant::now();
        assert_eq!(
            waiter.wait(None, Some(Duration::from_millis(5))),
            Ok(WorkerReady::Deadline)
        );
        assert!(started.elapsed() >= Duration::from_millis(5));
    }

    #[test]
    fn an_idle_wait_is_interrupted_without_a_periodic_deadline() {
        let (wake, mut waiter) = worker_readiness().expect("readiness constructs");
        let publisher = thread::spawn(move || wake.signal().expect("wake publishes"));
        assert_eq!(waiter.wait(None, None), Ok(WorkerReady::Wake));
        publisher.join().expect("publisher joins");
    }
}
