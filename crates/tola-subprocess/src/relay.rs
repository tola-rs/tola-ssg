//! Stream reading and bounded relay to the command observer.

use std::{
    io::{self, Read},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use crate::capture::{Captured, Stream};
use crate::command::Observer;
use crate::{Error, Result, sys};

const PIPE_DRAIN_GRACE: Duration = Duration::from_millis(250);
/// Bytes allowed after a stop request so an in-flight EOF still completes while live writers are bounded.
const STOP_DRAIN_BYTES: usize = 1 << 20;
const OUTPUT_CHUNK_BYTES: usize = 16 * 1024;
const OUTPUT_RELAY_CAPACITY: usize = 2;

#[derive(Debug)]
pub(crate) enum StreamEnd {
    Eof,
    Stopped,
}

pub(crate) struct StreamCapture {
    pub(crate) captured: Captured,
    pub(crate) end: io::Result<StreamEnd>,
}

pub(crate) enum RelayEvent {
    Output(Stream, Box<[u8]>),
    Finished(Stream, Result<Captured>),
}

/// Keeps only the receiver so dropping the relay unblocks senders, and sender drop closes the channel.
pub(crate) struct Relay {
    pub(crate) stop: sys::ReaderStop,
    pub(crate) workers: [Option<thread::JoinHandle<()>>; 2],
    rx: Option<mpsc::Receiver<RelayEvent>>,
    captures: [Option<Captured>; 2],
    relay_enabled: bool,
}

impl Relay {
    pub(crate) fn new(relay_enabled: bool) -> (Self, mpsc::SyncSender<RelayEvent>) {
        let (tx, rx) = mpsc::sync_channel(OUTPUT_RELAY_CAPACITY);
        (
            Self {
                stop: sys::ReaderStop::default(),
                workers: [None, None],
                rx: Some(rx),
                captures: [None, None],
                relay_enabled,
            },
            tx,
        )
    }

    pub(crate) fn spawn(
        &mut self,
        pipe: impl sys::Pipe,
        stream: Stream,
        tx: &mpsc::SyncSender<RelayEvent>,
    ) -> Result<()> {
        let idx = stream.index();
        let reader = sys::Reader::new(pipe, self.stop.clone()).map_err(Error::Reader)?;
        let stop = self.stop.clone();
        let tx = tx.clone();
        let relay = self.relay_enabled;
        self.workers[idx] = Some(
            thread::Builder::new()
                .name(format!("tola-subprocess-{}", stream.name()))
                .spawn(move || {
                    let cap = capture_from(read_stream(reader, stop, &tx, stream, relay));
                    let _ = tx.send(RelayEvent::Finished(stream, cap));
                })
                .map_err(Error::Reader)?,
        );
        Ok(())
    }

    pub(crate) fn receive(&mut self, timeout: Duration, obs: &mut impl Observer) -> Result<()> {
        let disconnected = match self
            .rx
            .as_ref()
            .expect("the relay is receiving")
            .recv_timeout(timeout)
        {
            Ok(ev) => {
                self.deliver(ev, obs)?;
                false
            }
            Err(mpsc::RecvTimeoutError::Timeout) => false,
            Err(mpsc::RecvTimeoutError::Disconnected) => true,
        };
        for worker in &mut self.workers {
            if worker.as_ref().is_some_and(thread::JoinHandle::is_finished) {
                join_reader(worker.take().expect("the reader finished"))?;
            }
        }
        if disconnected {
            // Closed pipes do not imply child exit; keep the cancellation polling cadence.
            thread::sleep(timeout);
        }
        Ok(())
    }

    fn deliver(&mut self, ev: RelayEvent, obs: &mut impl Observer) -> Result<()> {
        match ev {
            RelayEvent::Output(stream, bytes) => obs.output(stream, &bytes),
            RelayEvent::Finished(stream, cap) => {
                self.captures[stream.index()] = Some(cap?);
                if self.relay_enabled {
                    obs.finished(stream);
                }
            }
        }
        Ok(())
    }

    fn request_stop(&mut self) {
        self.stop.request();
        self.rx.take();
    }

    pub(crate) fn finish(mut self, obs: &mut impl Observer) -> Result<[Captured; 2]> {
        let deadline = Instant::now() + PIPE_DRAIN_GRACE;
        while !self.captures.iter().all(Option::is_some) {
            let Some(rem) = deadline.checked_duration_since(Instant::now()) else {
                break;
            };
            self.receive(rem, obs)?;
        }
        self.stop.request();
        // Drain remaining queued events before dropping rx so callbacks complete.
        if let Some(rx) = self.rx.take() {
            for ev in rx {
                self.deliver(ev, obs)?;
            }
        }
        for worker in &mut self.workers {
            if let Some(worker) = worker.take() {
                join_reader(worker)?;
            }
        }
        Ok(self
            .captures
            .each_mut()
            .map(|cap| cap.take().expect("both stream captures completed")))
    }
}

impl Drop for Relay {
    fn drop(&mut self) {
        self.request_stop();
        for worker in &mut self.workers {
            if let Some(worker) = worker.take() {
                let _ = worker.join();
            }
        }
    }
}

pub(crate) fn join_reader(worker: thread::JoinHandle<()>) -> Result<()> {
    worker
        .join()
        .map_err(|_| Error::Read(io::Error::other("a stream reader panicked")))
}

pub(crate) fn capture_from(cap: StreamCapture) -> Result<Captured> {
    match cap.end {
        Ok(StreamEnd::Eof) => Ok(cap.captured),
        Ok(StreamEnd::Stopped) => {
            let mut captured = cap.captured;
            captured.mark_stopped_early();
            Ok(captured)
        }
        Err(err) => Err(Error::Read(err)),
    }
}

pub(crate) fn read_stream(
    mut pipe: impl Read,
    stop: sys::ReaderStop,
    tx: &mpsc::SyncSender<RelayEvent>,
    stream: Stream,
    relay_enabled: bool,
) -> StreamCapture {
    let mut captured = Captured::default();
    let mut buf = [0u8; OUTPUT_CHUNK_BYTES];
    let mut drained = 0usize;
    let end = loop {
        match pipe.read(&mut buf) {
            Ok(0) => break Ok(StreamEnd::Eof),
            Ok(n) => {
                captured.extend(&buf[..n]);
                if relay_enabled
                    && tx
                        .send(RelayEvent::Output(stream, buf[..n].into()))
                        .is_err()
                {
                    break Ok(StreamEnd::Stopped);
                }
                if stop.is_requested() {
                    drained = drained.saturating_add(n);
                    if drained > STOP_DRAIN_BYTES {
                        break Ok(StreamEnd::Stopped);
                    }
                }
            }
            Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
            Err(err) if err.get_ref().is_some_and(|e| e.is::<sys::Stopped>()) => {
                break Ok(StreamEnd::Stopped);
            }
            Err(err) => break Err(err),
        }
    };
    // Drop the pipe reader before notifying the owner so platform monitors join first.
    drop(pipe);
    StreamCapture { captured, end }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    const CAPTURE_DEADLINE: Duration = Duration::from_secs(5);

    #[test]
    fn stop_preserves_eof() {
        let (reader, mut writer) = io::pipe().unwrap();
        writer.write_all(b"body").unwrap();
        drop(writer);
        let stop = sys::ReaderStop::default();
        stop.request();
        let (tx, rx) = mpsc::sync_channel(OUTPUT_RELAY_CAPACITY);
        let cap = read_stream(
            sys::Reader::new(reader, stop.clone()).unwrap(),
            stop,
            &tx,
            Stream::Stdout,
            false,
        );
        drop(rx);

        assert!(matches!(cap.end, Ok(StreamEnd::Eof)));
        assert_eq!(cap.captured.render(|_| String::new()), b"body");
    }

    #[test]
    fn stop_bounds_live_writer() {
        let (reader, mut writer) = io::pipe().unwrap();
        let stop = sys::ReaderStop::default();
        let (tx, rx) = mpsc::sync_channel(OUTPUT_RELAY_CAPACITY);
        let read_stop = stop.clone();
        let worker = thread::spawn(move || {
            read_stream(
                sys::Reader::new(reader, read_stop.clone()).unwrap(),
                read_stop,
                &tx,
                Stream::Stdout,
                false,
            )
        });
        drop(rx);
        stop.request();

        let block = vec![b'x'; OUTPUT_CHUNK_BYTES];
        let deadline = Instant::now() + CAPTURE_DEADLINE;
        loop {
            if writer.write_all(&block).is_err() || worker.is_finished() {
                break;
            }
            assert!(Instant::now() < deadline, "the reader outlived its stop");
        }
        let cap = worker.join().unwrap();

        assert!(matches!(cap.end, Ok(StreamEnd::Stopped)));
    }
}
