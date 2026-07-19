use purewave_engine::MidiMessage;
use std::cell::UnsafeCell;
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::mem::MaybeUninit;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicUsize, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

const DEBUG_LOG_QUEUE_CAPACITY: usize = 512;

/// Identifies whether ALSA accepted a sequenced event or an active-note cleanup message.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DebugMidiEventKind {
    Scheduled,
    Cleanup,
}

/// Bounded diagnostic sink for ALSA MIDI events and bridge failures.
///
/// Per-event messages are opt-in, while an ALSA output error is always reported. Terminal I/O
/// runs on a separate worker so a slow or broken stderr can drop diagnostics but never block the
/// ALSA bridge or JACK callback.
pub struct DebugMidiLogger {
    producer: DebugMidiLogProducer,
    running: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl DebugMidiLogger {
    pub fn open() -> io::Result<Self> {
        // Allocate the queue before the app activates JACK. The ALSA worker is its sole producer
        // and this logger thread is its sole consumer.
        let queue = Arc::new(DebugMidiLogQueue::new());
        let running = Arc::new(AtomicBool::new(true));
        let worker_queue = Arc::clone(&queue);
        let worker_running = Arc::clone(&running);
        let worker = thread::Builder::new()
            .name("purewave-midi-debug".to_owned())
            .spawn(move || run_debug_midi_worker(worker_queue, worker_running))?;

        Ok(Self {
            producer: DebugMidiLogProducer {
                queue,
                midi_event_logging_enabled: false,
            },
            running,
            worker: Some(worker),
        })
    }

    pub fn producer(&self, midi_event_logging_enabled: bool) -> DebugMidiLogProducer {
        DebugMidiLogProducer {
            queue: Arc::clone(&self.producer.queue),
            midi_event_logging_enabled,
        }
    }

    fn stop(&mut self) {
        self.running.store(false, Ordering::Release);

        // Never join a diagnostic writer: a blocked terminal must not delay Purewave shutdown.
        // Dropping the join handle detaches the worker, and process exit cleans it up if needed.
        let _ = self.worker.take();
    }
}

impl Drop for DebugMidiLogger {
    fn drop(&mut self) {
        self.stop();
    }
}

#[derive(Clone)]
pub struct DebugMidiLogProducer {
    queue: Arc<DebugMidiLogQueue>,
    midi_event_logging_enabled: bool,
}

impl DebugMidiLogProducer {
    pub fn log(&self, message: MidiMessage, kind: DebugMidiEventKind) {
        if self.midi_event_logging_enabled {
            self.push(DebugMidiEvent::Midi { message, kind });
        }
    }

    pub fn log_alsa_output_error(&self, code: i32) {
        // Error reporting has reserved atomic storage so a full debug-event queue cannot hide a
        // bridge failure. The first nonzero ALSA error is preserved until the logger reads it.
        let _ = self.queue.output_error_code.compare_exchange(
            0,
            code,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
    }

    fn push(&self, event: DebugMidiEvent) {
        if !self.queue.try_push(event) {
            self.queue
                .dropped_event_count
                .fetch_add(1, Ordering::Relaxed);
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DebugMidiEvent {
    Midi {
        message: MidiMessage,
        kind: DebugMidiEventKind,
    },
    AlsaOutputError(i32),
}

fn run_debug_midi_worker(queue: Arc<DebugMidiLogQueue>, running: Arc<AtomicBool>) {
    // Opening a dedicated handle avoids taking Rust's process-wide stderr lock. The logger may
    // block on this handle, but it can no longer make an ALSA-worker error report wait on it.
    let mut stderr = OpenOptions::new().write(true).open("/dev/stderr").ok();

    while running.load(Ordering::Acquire) || !queue.is_empty() || queue.has_output_error() {
        let wrote_event = drain_debug_midi_events(&queue, &mut |line| {
            write_debug_line(&mut stderr, line);
        });

        if !wrote_event {
            thread::sleep(Duration::from_millis(1));
        }
    }
}

fn drain_debug_midi_events(queue: &DebugMidiLogQueue, write_line: &mut impl FnMut(&str)) -> bool {
    let mut wrote_event = false;

    // The error slot is independent from queued debug traffic and is drained first so a bridge
    // failure is not buried behind a large backlog of normal note events.
    let output_error_code = queue.output_error_code.swap(0, Ordering::AcqRel);
    if output_error_code != 0 {
        write_line(&format_debug_midi_event(DebugMidiEvent::AlsaOutputError(
            output_error_code,
        )));
        wrote_event = true;
    }

    while let Some(event) = queue.try_pop() {
        write_line(&format_debug_midi_event(event));
        wrote_event = true;
    }

    let dropped_event_count = queue.dropped_event_count.swap(0, Ordering::AcqRel);
    if dropped_event_count != 0 {
        write_line(&format!(
            "DEBUG purewave: dropped {dropped_event_count} MIDI diagnostic events because the debug logger was busy"
        ));
        wrote_event = true;
    }

    wrote_event
}

fn write_debug_line(stderr: &mut Option<File>, line: &str) {
    // `writeln!` returns an error instead of panicking. Disable output after a failure so a broken
    // destination cannot consume further diagnostic-worker time.
    let write_failed = stderr
        .as_mut()
        .is_some_and(|stderr| writeln!(stderr, "{line}").is_err());

    if write_failed {
        *stderr = None;
    }
}

fn format_debug_midi_event(event: DebugMidiEvent) -> String {
    // A successful ALSA send only proves ALSA accepted the event for subscribers; it cannot prove
    // that Bitwig or another receiver processed it.
    match event {
        DebugMidiEvent::Midi { message, kind } => {
            let kind = match kind {
                DebugMidiEventKind::Scheduled => "scheduled",
                DebugMidiEventKind::Cleanup => "cleanup",
            };
            let bytes = message.bytes();

            match message {
                MidiMessage::NoteOn {
                    channel,
                    note,
                    velocity,
                } => format!(
                    "DEBUG purewave: ALSA bridge sent {kind} note-on channel={} note={} velocity={} bytes=[{:02X}, {:02X}, {:02X}]",
                    channel.get(),
                    note.get(),
                    velocity.get(),
                    bytes[0],
                    bytes[1],
                    bytes[2],
                ),
                MidiMessage::NoteOff { channel, note } => format!(
                    "DEBUG purewave: ALSA bridge sent {kind} note-off channel={} note={} bytes=[{:02X}, {:02X}, {:02X}]",
                    channel.get(),
                    note.get(),
                    bytes[0],
                    bytes[1],
                    bytes[2],
                ),
            }
        }
        DebugMidiEvent::AlsaOutputError(code) => {
            format!("Purewave ALSA bridge encountered an output error: {code}")
        }
    }
}

struct DebugMidiLogQueue {
    // A fixed ring buffer makes the diagnostic producer nonblocking even if terminal output is
    // stalled. Unlike the ALSA queue, this producer is the ALSA worker rather than JACK itself.
    slots: Box<[UnsafeCell<MaybeUninit<DebugMidiEvent>>; DEBUG_LOG_QUEUE_CAPACITY]>,
    read_index: AtomicUsize,
    write_index: AtomicUsize,
    dropped_event_count: AtomicUsize,
    output_error_code: AtomicI32,
}

// Safety: the ALSA worker exclusively owns writes while the debug worker exclusively owns reads.
// Release/acquire index handoff publishes initialized slots before the consumer observes them.
unsafe impl Sync for DebugMidiLogQueue {}

impl DebugMidiLogQueue {
    fn new() -> Self {
        Self {
            slots: Box::new(std::array::from_fn(|_| {
                UnsafeCell::new(MaybeUninit::uninit())
            })),
            read_index: AtomicUsize::new(0),
            write_index: AtomicUsize::new(0),
            dropped_event_count: AtomicUsize::new(0),
            output_error_code: AtomicI32::new(0),
        }
    }

    fn try_push(&self, event: DebugMidiEvent) -> bool {
        let write_index = self.write_index.load(Ordering::Relaxed);
        let next_write_index = (write_index + 1) % DEBUG_LOG_QUEUE_CAPACITY;

        if next_write_index == self.read_index.load(Ordering::Acquire) {
            return false;
        }

        unsafe {
            (*self.slots[write_index].get()).write(event);
        }
        self.write_index.store(next_write_index, Ordering::Release);
        true
    }

    fn try_pop(&self) -> Option<DebugMidiEvent> {
        let read_index = self.read_index.load(Ordering::Relaxed);

        if read_index == self.write_index.load(Ordering::Acquire) {
            return None;
        }

        let event = unsafe { (*self.slots[read_index].get()).assume_init_read() };
        let next_read_index = (read_index + 1) % DEBUG_LOG_QUEUE_CAPACITY;
        self.read_index.store(next_read_index, Ordering::Release);
        Some(event)
    }

    fn is_empty(&self) -> bool {
        self.read_index.load(Ordering::Acquire) == self.write_index.load(Ordering::Acquire)
    }

    fn has_output_error(&self) -> bool {
        self.output_error_code.load(Ordering::Acquire) != 0
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DEBUG_LOG_QUEUE_CAPACITY, DebugMidiEvent, DebugMidiEventKind, DebugMidiLogProducer,
        DebugMidiLogQueue, DebugMidiLogger, drain_debug_midi_events, format_debug_midi_event,
    };
    use purewave_engine::{MidiChannel, MidiMessage, MidiNote, MidiVelocity};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::thread;

    #[test]
    fn formats_scheduled_note_on_for_routing_diagnostics() {
        let event = DebugMidiEvent::Midi {
            kind: DebugMidiEventKind::Scheduled,
            message: MidiMessage::NoteOn {
                channel: MidiChannel::new(10).unwrap(),
                note: MidiNote::new(36).unwrap(),
                velocity: MidiVelocity::new(100).unwrap(),
            },
        };

        assert_eq!(
            format_debug_midi_event(event),
            "DEBUG purewave: ALSA bridge sent scheduled note-on channel=10 note=36 velocity=100 bytes=[99, 24, 64]"
        );
    }

    #[test]
    fn formats_cleanup_note_off_for_recovery_diagnostics() {
        let event = DebugMidiEvent::Midi {
            kind: DebugMidiEventKind::Cleanup,
            message: MidiMessage::NoteOff {
                channel: MidiChannel::new(10).unwrap(),
                note: MidiNote::new(36).unwrap(),
            },
        };

        assert_eq!(
            format_debug_midi_event(event),
            "DEBUG purewave: ALSA bridge sent cleanup note-off channel=10 note=36 bytes=[89, 24, 00]"
        );
    }

    #[test]
    fn formats_alsa_output_errors_for_routing_diagnostics() {
        assert_eq!(
            format_debug_midi_event(DebugMidiEvent::AlsaOutputError(-5)),
            "Purewave ALSA bridge encountered an output error: -5"
        );
    }

    #[test]
    fn debug_queue_drops_events_when_full() {
        let queue = Arc::new(DebugMidiLogQueue::new());
        let producer = DebugMidiLogProducer {
            queue: Arc::clone(&queue),
            midi_event_logging_enabled: true,
        };

        for note in 0..(DEBUG_LOG_QUEUE_CAPACITY - 1) {
            producer.log(event((note % 128) as u8), DebugMidiEventKind::Scheduled);
        }
        producer.log(event(0), DebugMidiEventKind::Scheduled);

        assert_eq!(queue.dropped_event_count.load(Ordering::Acquire), 1);
    }

    #[test]
    fn disabled_event_logging_keeps_cleanup_messages_out_of_the_queue() {
        let queue = Arc::new(DebugMidiLogQueue::new());
        let producer = DebugMidiLogProducer {
            queue: Arc::clone(&queue),
            midi_event_logging_enabled: false,
        };

        producer.log(event(36), DebugMidiEventKind::Cleanup);

        assert!(queue.is_empty());
    }

    #[test]
    fn output_error_has_reserved_storage_when_event_queue_is_full() {
        let queue = Arc::new(DebugMidiLogQueue::new());
        let producer = DebugMidiLogProducer {
            queue: Arc::clone(&queue),
            midi_event_logging_enabled: true,
        };

        for note in 0..(DEBUG_LOG_QUEUE_CAPACITY - 1) {
            producer.log(event((note % 128) as u8), DebugMidiEventKind::Scheduled);
        }
        producer.log_alsa_output_error(-5);

        let mut lines = Vec::new();
        assert!(drain_debug_midi_events(&queue, &mut |line| {
            lines.push(line.to_owned());
        }));

        assert_eq!(
            lines.first().map(String::as_str),
            Some("Purewave ALSA bridge encountered an output error: -5")
        );
    }

    #[test]
    fn reserved_output_error_keeps_an_empty_event_queue_pending() {
        let queue = Arc::new(DebugMidiLogQueue::new());
        let producer = DebugMidiLogProducer {
            queue: Arc::clone(&queue),
            midi_event_logging_enabled: false,
        };

        producer.log_alsa_output_error(-5);

        assert!(queue.is_empty());
        assert!(queue.has_output_error());
    }

    #[test]
    fn debug_queue_preserves_events_during_concurrent_wraparound() {
        const EVENT_COUNT: usize = DEBUG_LOG_QUEUE_CAPACITY * 32;

        let queue = Arc::new(DebugMidiLogQueue::new());
        let producer_queue = Arc::clone(&queue);
        let producer = thread::spawn(move || {
            for index in 0..EVENT_COUNT {
                let event = DebugMidiEvent::Midi {
                    message: event((index % 128) as u8),
                    kind: DebugMidiEventKind::Scheduled,
                };

                while !producer_queue.try_push(event) {
                    thread::yield_now();
                }
            }
        });

        for index in 0..EVENT_COUNT {
            let expected = DebugMidiEvent::Midi {
                message: event((index % 128) as u8),
                kind: DebugMidiEventKind::Scheduled,
            };
            let received = loop {
                if let Some(event) = queue.try_pop() {
                    break event;
                }

                thread::yield_now();
            };

            assert_eq!(received, expected);
        }

        producer.join().unwrap();
    }

    #[test]
    fn debug_queue_drains_events_pending_at_shutdown() {
        let queue = DebugMidiLogQueue::new();
        assert!(queue.try_push(DebugMidiEvent::Midi {
            message: event(36),
            kind: DebugMidiEventKind::Cleanup,
        }));
        let mut lines = Vec::new();

        assert!(drain_debug_midi_events(&queue, &mut |line| {
            lines.push(line.to_owned());
        }));

        assert!(queue.is_empty());
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("cleanup note-on"));
    }

    #[test]
    fn stopping_logger_detaches_a_blocked_diagnostic_worker() {
        let queue = Arc::new(DebugMidiLogQueue::new());
        let running = Arc::new(AtomicBool::new(true));
        let release_worker = Arc::new(AtomicBool::new(false));
        let worker_release_signal = Arc::clone(&release_worker);
        let worker = thread::spawn(move || {
            while !worker_release_signal.load(Ordering::Acquire) {
                thread::yield_now();
            }
        });
        let mut logger = DebugMidiLogger {
            producer: DebugMidiLogProducer {
                queue,
                midi_event_logging_enabled: false,
            },
            running: Arc::clone(&running),
            worker: Some(worker),
        };

        // This simulated writer cannot observe the stop flag. `stop` must detach instead of join.
        logger.stop();

        assert!(!running.load(Ordering::Acquire));
        assert!(logger.worker.is_none());
        release_worker.store(true, Ordering::Release);
    }

    fn event(note: u8) -> MidiMessage {
        MidiMessage::NoteOn {
            channel: MidiChannel::new(10).unwrap(),
            note: MidiNote::new(note).unwrap(),
            velocity: MidiVelocity::new(100).unwrap(),
        }
    }
}
