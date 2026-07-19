use alsa_sys::{
    SND_SEQ_ADDRESS_SUBSCRIBERS, SND_SEQ_ADDRESS_UNKNOWN, SND_SEQ_NONBLOCK, SND_SEQ_OPEN_OUTPUT,
    SND_SEQ_QUEUE_DIRECT, snd_midi_event_encode, snd_midi_event_free, snd_midi_event_new,
    snd_midi_event_reset_encode, snd_midi_event_t, snd_seq_close, snd_seq_create_simple_port,
    snd_seq_event_output_direct, snd_seq_event_t, snd_seq_open, snd_seq_set_client_name, snd_seq_t,
};
use jack_sys::{
    JackPortIsOutput, JackPositionBBT, JackTransportRolling, JackUseExactName, RAW_MIDI_TYPE,
    jack_activate, jack_client_close, jack_client_open, jack_client_t, jack_deactivate,
    jack_get_sample_rate, jack_midi_clear_buffer, jack_midi_event_write, jack_nframes_t,
    jack_port_get_buffer, jack_port_register, jack_port_t, jack_position_t,
    jack_set_process_callback, jack_status_t, jack_transport_query,
};
use purewave_engine::{
    DrumSound, MidiChannel, MidiMessage, MidiNote, Pattern, Scheduler, TimedMidiEvent, Transport,
};
use std::ffi::{CString, c_void};
use std::io;
use std::mem::MaybeUninit;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

const CLIENT_NAME: &str = "purewave";
const MIDI_OUT_PORT: &str = "midi_out";
const ALSA_CLIENT_NAME: &str = "Purewave MIDI";
const ALSA_MIDI_OUT_PORT: &str = "midi_out";
const DEFAULT_TEMPO_BPM: f64 = 120.0;
// The scheduler must never expand this buffer from JACK's process callback.
const MAX_EVENTS_PER_BLOCK: usize = 128;
// One slot remains empty in the ring buffer to distinguish "full" from "empty".
const ALSA_EVENT_QUEUE_CAPACITY: usize = 512;
// ALSA source-port capabilities: another client may subscribe and read MIDI from it.
const ALSA_PORT_CAPABILITIES: u32 = (1 << 0) | (1 << 5);
// Mark the virtual port as a MIDI-capable software application endpoint.
const ALSA_PORT_TYPE: u32 = (1 << 1) | (1 << 17) | (1 << 20);

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), JackAppError> {
    // Bitwig discovers ALSA sequencer clients, while JACK-aware peers consume the direct JACK
    // port. Both receive the engine's same scheduled MIDI messages.
    let mut alsa_midi = AlsaMidiBridge::open()?;
    let mut client = JackMidiClient::open(default_pattern(), alsa_midi.producer())?;
    client.activate()?;

    println!("Purewave JACK MIDI client is running.");
    println!("Connect purewave:midi_out to your DAW or instrument, then start JACK transport.");
    println!("For Bitwig, choose Purewave MIDI as a Generic MIDI Keyboard input.");
    println!("Press Enter to stop.");

    wait_for_enter();
    client.deactivate();
    alsa_midi.stop();

    let dropped_events = alsa_midi.dropped_event_count();
    if dropped_events != 0 {
        eprintln!("dropped {dropped_events} MIDI events from the ALSA compatibility bridge");
    }

    Ok(())
}

struct JackMidiClient {
    // JACK owns these raw handles. `_state` stays boxed at a stable address because JACK stores
    // its pointer as the process callback's user data.
    client: *mut jack_client_t,
    _state: Box<ProcessState>,
    active: bool,
}

impl JackMidiClient {
    fn open(pattern: Pattern, alsa_midi: AlsaMidiProducer) -> Result<Self, JackAppError> {
        let client_name = CString::new(CLIENT_NAME)?;
        let port_name = CString::new(MIDI_OUT_PORT)?;
        let midi_type = CString::new(RAW_MIDI_TYPE)?;
        let mut status = MaybeUninit::<jack_status_t>::zeroed();

        // Exact naming makes the documented port name deterministic: `purewave:midi_out`.
        let client = unsafe {
            jack_client_open(client_name.as_ptr(), JackUseExactName, status.as_mut_ptr())
        };

        if client.is_null() {
            let status = unsafe { status.assume_init() };
            return Err(JackAppError::OpenClient(status));
        }

        // JACK MIDI uses a port buffer owned by JACK for each audio processing block.
        let midi_out = unsafe {
            jack_port_register(
                client,
                port_name.as_ptr(),
                midi_type.as_ptr(),
                JackPortIsOutput as u64,
                0,
            )
        };

        if midi_out.is_null() {
            unsafe {
                jack_client_close(client);
            }
            return Err(JackAppError::RegisterPort);
        }

        let mut state = Box::new(ProcessState::new(client, midi_out, pattern, alsa_midi));
        // `state` is retained in the client for the callback's entire lifetime.
        let callback_arg = (&mut *state) as *mut ProcessState as *mut c_void;
        let callback_result =
            unsafe { jack_set_process_callback(client, Some(process_callback), callback_arg) };

        if callback_result != 0 {
            unsafe {
                jack_client_close(client);
            }
            return Err(JackAppError::SetProcessCallback(callback_result));
        }

        Ok(Self {
            client,
            _state: state,
            active: false,
        })
    }

    fn activate(&mut self) -> Result<(), JackAppError> {
        let result = unsafe { jack_activate(self.client) };

        if result == 0 {
            self.active = true;
            Ok(())
        } else {
            Err(JackAppError::Activate(result))
        }
    }

    fn deactivate(&mut self) {
        if self.active {
            unsafe {
                jack_deactivate(self.client);
            }
            self.active = false;
        }
    }
}

impl Drop for JackMidiClient {
    fn drop(&mut self) {
        self.deactivate();

        unsafe {
            jack_client_close(self.client);
        }
    }
}

struct ProcessState {
    // Everything in this structure is touched on JACK's realtime process thread.
    client: *mut jack_client_t,
    midi_out: *mut jack_port_t,
    pattern: Pattern,
    scheduler: Scheduler,
    events: Vec<TimedMidiEvent>,
    alsa_midi: AlsaMidiProducer,
    fallback_tempo_bpm: f64,
}

impl ProcessState {
    fn new(
        client: *mut jack_client_t,
        midi_out: *mut jack_port_t,
        pattern: Pattern,
        alsa_midi: AlsaMidiProducer,
    ) -> Self {
        // Allocate once before activation. The callback uses the scheduler's bounded API so this
        // vector cannot grow while JACK is processing audio.
        let mut events = Vec::with_capacity(MAX_EVENTS_PER_BLOCK);
        events.reserve_exact(MAX_EVENTS_PER_BLOCK.saturating_sub(events.capacity()));

        Self {
            client,
            midi_out,
            pattern,
            scheduler: Scheduler,
            events,
            alsa_midi,
            fallback_tempo_bpm: DEFAULT_TEMPO_BPM,
        }
    }

    fn process(&mut self, frame_count: jack_nframes_t) {
        let Some(midi_buffer) = self.midi_buffer(frame_count) else {
            return;
        };

        // Each callback owns a fresh JACK MIDI buffer; clear stale events before writing ours.
        unsafe {
            jack_midi_clear_buffer(midi_buffer);
        }

        let transport = self.transport();
        // Frame offsets produced here are sample offsets within this exact JACK block.
        self.scheduler.schedule_midi_block_into_existing_capacity(
            &self.pattern,
            transport,
            frame_count,
            &mut self.events,
        );

        for event in &self.events {
            let bytes = event.message.bytes();
            unsafe {
                jack_midi_event_write(midi_buffer, event.frame_offset, bytes.as_ptr(), bytes.len());
            }

            // The ALSA route is intentionally only an enqueue. It never calls ALSA or blocks on
            // the JACK thread, and therefore is not the sample-accurate output path.
            self.alsa_midi.send(event.message);
        }
    }

    fn midi_buffer(&mut self, frame_count: jack_nframes_t) -> Option<*mut c_void> {
        let buffer = unsafe { jack_port_get_buffer(self.midi_out, frame_count) };

        if buffer.is_null() { None } else { Some(buffer) }
    }

    fn transport(&self) -> Transport {
        let mut position = MaybeUninit::<jack_position_t>::zeroed();
        let state = unsafe { jack_transport_query(self.client, position.as_mut_ptr()) };
        let position = unsafe { position.assume_init() };
        let sample_rate = unsafe { jack_get_sample_rate(self.client) };
        // JACK may not publish BBT data. In that case keep scheduling against a usable fallback
        // tempo while still honoring JACK's frame position and rolling/stopped state.
        let tempo_bpm = if position.valid & JackPositionBBT != 0
            && position.beats_per_minute.is_finite()
            && position.beats_per_minute > 0.0
        {
            position.beats_per_minute
        } else {
            self.fallback_tempo_bpm
        };

        Transport::new(
            f64::from(sample_rate),
            tempo_bpm,
            u64::from(position.frame),
            state == JackTransportRolling,
        )
    }
}

unsafe extern "C" fn process_callback(frame_count: jack_nframes_t, arg: *mut c_void) -> i32 {
    if arg.is_null() {
        return 0;
    }

    // `JackMidiClient::open` installed this pointer from its owned boxed ProcessState and keeps
    // it alive until after JACK has been deactivated.
    let state = unsafe { &mut *(arg as *mut ProcessState) };
    state.process(frame_count);
    0
}

fn default_pattern() -> Pattern {
    let mut pattern = Pattern::default_drum_grid();

    // A small fixed rhythm makes the standalone app audible before the UI exists.
    enable_steps(&mut pattern, DrumSound::Kick, &[0, 4, 8, 12]);
    enable_steps(&mut pattern, DrumSound::Snare, &[4, 12]);
    enable_steps(&mut pattern, DrumSound::Clap, &[4, 12]);
    enable_steps(&mut pattern, DrumSound::HiHat, &[0, 2, 4, 6, 8, 10, 12, 14]);
    enable_steps(&mut pattern, DrumSound::Cymbal, &[0]);

    pattern
}

fn enable_steps(pattern: &mut Pattern, sound: DrumSound, steps: &[usize]) {
    let Some(track) = pattern.tracks.iter_mut().find(|track| track.sound == sound) else {
        return;
    };

    for step in steps {
        track.enable_step(*step);
    }
}

fn wait_for_enter() {
    let mut input = String::new();
    let _ = io::stdin().read_line(&mut input);
}

struct AlsaMidiBridge {
    // The producer is shared with the JACK callback; the worker exclusively owns ALSA handles.
    producer: AlsaMidiProducer,
    running: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl AlsaMidiBridge {
    fn open() -> Result<Self, AlsaMidiError> {
        // Construct all shared state before spawning. Startup waits for the worker to create the
        // ALSA port so the app never claims to be ready before Bitwig can discover it.
        let queue = Arc::new(AlsaEventQueue::new());
        let dropped_events = Arc::new(AtomicUsize::new(0));
        let note_cleanup_requested = Arc::new(AtomicBool::new(false));
        let recovery_gate = Arc::new(AlsaRecoveryGate::new());
        let running = Arc::new(AtomicBool::new(true));
        let (ready_sender, ready_receiver) = mpsc::sync_channel(1);
        let worker_queue = Arc::clone(&queue);
        let worker_running = Arc::clone(&running);
        let worker_note_cleanup_requested = Arc::clone(&note_cleanup_requested);
        let worker_recovery_gate = Arc::clone(&recovery_gate);

        let worker = thread::Builder::new()
            .name("purewave-alsa-midi".to_owned())
            .spawn(move || {
                run_alsa_midi_worker(
                    worker_queue,
                    worker_running,
                    worker_note_cleanup_requested,
                    worker_recovery_gate,
                    ready_sender,
                )
            })
            .map_err(AlsaMidiError::SpawnWorker)?;

        match ready_receiver.recv() {
            Ok(Ok(())) => Ok(Self {
                producer: AlsaMidiProducer {
                    queue,
                    dropped_events,
                    note_cleanup_requested,
                    recovery_gate,
                },
                running,
                worker: Some(worker),
            }),
            Ok(Err(error)) => {
                let _ = worker.join();
                Err(error)
            }
            Err(_) => {
                let _ = worker.join();
                Err(AlsaMidiError::WorkerEndedBeforeReady)
            }
        }
    }

    fn producer(&self) -> AlsaMidiProducer {
        self.producer.clone()
    }

    fn dropped_event_count(&self) -> usize {
        self.producer.dropped_events.load(Ordering::Relaxed)
    }

    fn stop(&mut self) {
        // Join outside the JACK callback, after deactivation, so the worker can safely finish its
        // best-effort note cleanup without delaying audio processing.
        self.running.store(false, Ordering::Release);

        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl Drop for AlsaMidiBridge {
    fn drop(&mut self) {
        self.stop();
    }
}

#[derive(Clone)]
struct AlsaMidiProducer {
    // This is the callback-facing half of the compatibility bridge. Its only work is bounded
    // atomic bookkeeping and an SPSC enqueue.
    queue: Arc<AlsaEventQueue>,
    dropped_events: Arc<AtomicUsize>,
    note_cleanup_requested: Arc<AtomicBool>,
    recovery_gate: Arc<AlsaRecoveryGate>,
}

impl AlsaMidiProducer {
    fn send(&self, message: MidiMessage) {
        // During overflow recovery, dropping compatibility events is safer than letting a queued
        // note-on slip past the all-note-off cleanup. JACK output is still emitted above.
        if !self.recovery_gate.try_enter() {
            self.dropped_events.fetch_add(1, Ordering::Relaxed);
            return;
        }

        if !self.queue.try_push(message) {
            self.dropped_events.fetch_add(1, Ordering::Relaxed);
            self.note_cleanup_requested.store(true, Ordering::Release);
        }

        self.recovery_gate.leave();
    }
}

fn run_alsa_midi_worker(
    queue: Arc<AlsaEventQueue>,
    running: Arc<AtomicBool>,
    note_cleanup_requested: Arc<AtomicBool>,
    recovery_gate: Arc<AlsaRecoveryGate>,
    ready_sender: mpsc::SyncSender<Result<(), AlsaMidiError>>,
) {
    // All ALSA initialization and I/O live on this non-realtime thread.
    let mut output = match AlsaSequencerOutput::open() {
        Ok(output) => output,
        Err(error) => {
            let _ = ready_sender.send(Err(error));
            return;
        }
    };

    if ready_sender.send(Ok(())).is_err() {
        return;
    }

    let mut reported_output_error = false;

    while running.load(Ordering::Acquire) {
        if !drain_alsa_events(
            &queue,
            &mut output,
            &mut reported_output_error,
            &note_cleanup_requested,
            &recovery_gate,
        ) {
            thread::sleep(Duration::from_millis(1));
        }
    }

    drain_alsa_events(
        &queue,
        &mut output,
        &mut reported_output_error,
        &note_cleanup_requested,
        &recovery_gate,
    );

    // A disappearing ALSA client can otherwise leave a receiving instrument sustaining a note.
    // The retry limit bounds shutdown if the destination remains unavailable.
    for _ in 0..10 {
        if output.send_note_off_for_active_notes() {
            break;
        }

        thread::sleep(Duration::from_millis(1));
    }
}

fn drain_alsa_events(
    queue: &AlsaEventQueue,
    output: &mut AlsaSequencerOutput,
    reported_output_error: &mut bool,
    note_cleanup_requested: &AtomicBool,
    recovery_gate: &AlsaRecoveryGate,
) -> bool {
    // Overflow or a nonblocking ALSA backpressure error starts a transactional recovery instead
    // of attempting to preserve a potentially incomplete note-on/note-off sequence.
    if note_cleanup_requested.load(Ordering::Acquire) {
        return recover_from_alsa_overflow(queue, output, note_cleanup_requested, recovery_gate);
    }

    let mut sent_event = false;

    while let Some(message) = queue.try_pop() {
        if note_cleanup_requested.load(Ordering::Acquire) {
            return sent_event
                | recover_from_alsa_overflow(queue, output, note_cleanup_requested, recovery_gate);
        }

        sent_event = true;

        if let Err(code) = output.send(message) {
            if is_would_block(code) {
                note_cleanup_requested.store(true, Ordering::Release);
            } else if !*reported_output_error {
                eprintln!("ALSA MIDI compatibility bridge stopped delivering events: {code}");
                *reported_output_error = true;
            }
        }
    }

    // The producer may request cleanup after the last pop but before this loop
    // observes the queue as empty.
    if note_cleanup_requested.load(Ordering::Acquire) {
        sent_event |=
            recover_from_alsa_overflow(queue, output, note_cleanup_requested, recovery_gate);
    }

    sent_event
}

fn recover_from_alsa_overflow(
    queue: &AlsaEventQueue,
    output: &mut AlsaSequencerOutput,
    note_cleanup_requested: &AtomicBool,
    recovery_gate: &AlsaRecoveryGate,
) -> bool {
    // Close admission before flushing. `close_and_wait` proves no callback is still writing a
    // queue slot, so no MIDI event can appear after the flush and before note cleanup.
    recovery_gate.close_and_wait();

    // The queue is intentionally abandoned as a unit: replaying only part of an overloaded MIDI
    // sequence could deliver a note-on without its matching note-off.
    while queue.try_pop().is_some() {}

    if output.send_note_off_for_active_notes() {
        note_cleanup_requested.store(false, Ordering::Release);
        recovery_gate.open();
        true
    } else {
        note_cleanup_requested.store(true, Ordering::Release);
        false
    }
}

struct AlsaRecoveryGate {
    // Bit zero closes admission. Every active producer increments by two, so
    // closing the gate and observing active producers use the same atomic state.
    state: AtomicUsize,
}

impl AlsaRecoveryGate {
    const CLOSED: usize = 1;
    const PRODUCER_UNIT: usize = 2;

    fn new() -> Self {
        Self {
            state: AtomicUsize::new(0),
        }
    }

    fn try_enter(&self) -> bool {
        let mut state = self.state.load(Ordering::Acquire);

        loop {
            if state & Self::CLOSED != 0 {
                return false;
            }

            match self.state.compare_exchange_weak(
                state,
                state + Self::PRODUCER_UNIT,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return true,
                Err(updated_state) => state = updated_state,
            }
        }
    }

    fn leave(&self) {
        self.state.fetch_sub(Self::PRODUCER_UNIT, Ordering::Release);
    }

    fn close_and_wait(&self) {
        let mut state = self.state.load(Ordering::Acquire);

        loop {
            if state & Self::CLOSED == 0 {
                match self.state.compare_exchange_weak(
                    state,
                    state | Self::CLOSED,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                ) {
                    Ok(closed_state) => state = closed_state | Self::CLOSED,
                    Err(updated_state) => {
                        state = updated_state;
                        continue;
                    }
                }
            }

            if state == Self::CLOSED {
                return;
            }

            thread::yield_now();
            state = self.state.load(Ordering::Acquire);
        }
    }

    fn open(&self) {
        self.state.fetch_and(!Self::CLOSED, Ordering::Release);
    }
}

fn is_would_block(code: i32) -> bool {
    std::io::Error::from_raw_os_error(-code).kind() == std::io::ErrorKind::WouldBlock
}

struct AlsaSequencerOutput {
    // These raw ALSA handles are confined to the worker thread after construction.
    sequencer: *mut snd_seq_t,
    encoder: *mut snd_midi_event_t,
    port: u8,
    active_notes: [[bool; 128]; 16],
}

impl AlsaSequencerOutput {
    fn open() -> Result<Self, AlsaMidiError> {
        let default_device =
            CString::new("default").expect("default ALSA device contains no nulls");
        let client_name =
            CString::new(ALSA_CLIENT_NAME).expect("ALSA client name contains no nulls");
        let port_name = CString::new(ALSA_MIDI_OUT_PORT).expect("ALSA port name contains no nulls");
        let mut sequencer = std::ptr::null_mut();

        // Nonblocking mode means a slow ALSA destination returns EAGAIN instead of stalling
        // shutdown or the compatibility worker indefinitely.
        let open_result = unsafe {
            snd_seq_open(
                &mut sequencer,
                default_device.as_ptr(),
                SND_SEQ_OPEN_OUTPUT,
                SND_SEQ_NONBLOCK,
            )
        };
        if open_result < 0 {
            return Err(AlsaMidiError::OpenSequencer(open_result));
        }

        let set_name_result = unsafe { snd_seq_set_client_name(sequencer, client_name.as_ptr()) };
        if set_name_result < 0 {
            unsafe {
                snd_seq_close(sequencer);
            }
            return Err(AlsaMidiError::SetClientName(set_name_result));
        }

        let port = unsafe {
            snd_seq_create_simple_port(
                sequencer,
                port_name.as_ptr(),
                ALSA_PORT_CAPABILITIES,
                ALSA_PORT_TYPE,
            )
        };
        if port < 0 {
            unsafe {
                snd_seq_close(sequencer);
            }
            return Err(AlsaMidiError::CreatePort(port));
        }

        let mut encoder = std::ptr::null_mut();
        let encoder_result = unsafe { snd_midi_event_new(3, &mut encoder) };
        if encoder_result < 0 {
            unsafe {
                snd_seq_close(sequencer);
            }
            return Err(AlsaMidiError::CreateEncoder(encoder_result));
        }

        Ok(Self {
            sequencer,
            encoder,
            port: port as u8,
            active_notes: [[false; 128]; 16],
        })
    }

    fn send(&mut self, message: MidiMessage) -> Result<(), i32> {
        self.send_message(message)?;
        self.record_active_note(message);
        Ok(())
    }

    fn send_note_off_for_active_notes(&mut self) -> bool {
        // Track successful note-ons locally because the ALSA API has no query for a subscriber's
        // sounding notes. Only those notes need cleanup after an interrupted sequence.
        let mut all_notes_sent = true;

        for channel_index in 0..self.active_notes.len() {
            for note_index in 0..self.active_notes[channel_index].len() {
                if !self.active_notes[channel_index][note_index] {
                    continue;
                }

                let Some(channel) = MidiChannel::new((channel_index + 1) as u8) else {
                    continue;
                };
                let Some(note) = MidiNote::new(note_index as u8) else {
                    continue;
                };

                if self
                    .send_message(MidiMessage::NoteOff { channel, note })
                    .is_ok()
                {
                    self.active_notes[channel_index][note_index] = false;
                } else {
                    all_notes_sent = false;
                }
            }
        }

        all_notes_sent
    }

    fn send_message(&self, message: MidiMessage) -> Result<(), i32> {
        let bytes = message.bytes();
        let mut event = MaybeUninit::<snd_seq_event_t>::zeroed();

        // ALSA's MIDI encoder translates the three raw MIDI bytes into its sequencer event
        // struct, avoiding hand-written union initialization.
        unsafe {
            snd_midi_event_reset_encode(self.encoder);
            let encoded = snd_midi_event_encode(
                self.encoder,
                bytes.as_ptr(),
                bytes.len() as std::os::raw::c_long,
                event.as_mut_ptr(),
            );
            if encoded != bytes.len() as std::os::raw::c_long {
                return Err(encoded as i32);
            }

            let event = &mut *event.as_mut_ptr();
            event.source.port = self.port;
            // Subscribers (for example Bitwig's virtual MIDI input) receive this direct event.
            event.dest.client = SND_SEQ_ADDRESS_SUBSCRIBERS;
            event.dest.port = SND_SEQ_ADDRESS_UNKNOWN;
            event.queue = SND_SEQ_QUEUE_DIRECT;

            let output_result = snd_seq_event_output_direct(self.sequencer, event);
            if output_result < 0 {
                return Err(output_result);
            }
        }

        Ok(())
    }

    fn record_active_note(&mut self, message: MidiMessage) {
        match message {
            MidiMessage::NoteOn {
                channel,
                note,
                velocity,
            } => {
                self.active_notes[channel.status_nibble() as usize][note.get() as usize] =
                    velocity.get() != 0;
            }
            MidiMessage::NoteOff { channel, note } => {
                self.active_notes[channel.status_nibble() as usize][note.get() as usize] = false;
            }
        }
    }
}

impl Drop for AlsaSequencerOutput {
    fn drop(&mut self) {
        unsafe {
            snd_midi_event_free(self.encoder);
            snd_seq_close(self.sequencer);
        }
    }
}

struct AlsaEventQueue {
    // Fixed slots keep the callback allocation-free. UnsafeCell is required because producer and
    // consumer write distinct slots without a mutex.
    slots: Box<[std::cell::UnsafeCell<MaybeUninit<MidiMessage>>; ALSA_EVENT_QUEUE_CAPACITY]>,
    read_index: AtomicUsize,
    write_index: AtomicUsize,
}

// Safety: JACK invokes one process callback per client, making this a single
// producer. The ALSA worker is the sole consumer. The release/acquire index
// handoff makes initialized slots visible before the consumer reads them.
unsafe impl Sync for AlsaEventQueue {}

impl AlsaEventQueue {
    fn new() -> Self {
        Self {
            slots: Box::new(std::array::from_fn(|_| {
                std::cell::UnsafeCell::new(MaybeUninit::uninit())
            })),
            read_index: AtomicUsize::new(0),
            write_index: AtomicUsize::new(0),
        }
    }

    fn try_push(&self, message: MidiMessage) -> bool {
        let write_index = self.write_index.load(Ordering::Relaxed);
        let next_write_index = (write_index + 1) % ALSA_EVENT_QUEUE_CAPACITY;

        if next_write_index == self.read_index.load(Ordering::Acquire) {
            return false;
        }

        // The producer owns `write_index`; the release store below publishes this initialized
        // slot to the consumer.
        unsafe {
            (*self.slots[write_index].get()).write(message);
        }
        self.write_index.store(next_write_index, Ordering::Release);
        true
    }

    fn try_pop(&self) -> Option<MidiMessage> {
        let read_index = self.read_index.load(Ordering::Relaxed);

        if read_index == self.write_index.load(Ordering::Acquire) {
            return None;
        }

        // The acquire load above pairs with the producer's release store, so this slot has been
        // initialized before it is read. The consumer exclusively owns `read_index`.
        let message = unsafe { (*self.slots[read_index].get()).assume_init_read() };
        let next_read_index = (read_index + 1) % ALSA_EVENT_QUEUE_CAPACITY;
        self.read_index.store(next_read_index, Ordering::Release);
        Some(message)
    }
}

#[derive(Debug)]
enum JackAppError {
    Activate(i32),
    OpenClient(jack_status_t),
    RegisterPort,
    SetProcessCallback(i32),
    StringContainsNull(std::ffi::NulError),
    Alsa(AlsaMidiError),
}

impl From<std::ffi::NulError> for JackAppError {
    fn from(error: std::ffi::NulError) -> Self {
        Self::StringContainsNull(error)
    }
}

impl From<AlsaMidiError> for JackAppError {
    fn from(error: AlsaMidiError) -> Self {
        Self::Alsa(error)
    }
}

impl std::fmt::Display for JackAppError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Activate(code) => write!(formatter, "failed to activate JACK client: {code}"),
            Self::OpenClient(status) => write!(
                formatter,
                "failed to open JACK client; is the JACK server running? status: {status:?}"
            ),
            Self::RegisterPort => write!(formatter, "failed to register JACK MIDI output port"),
            Self::SetProcessCallback(code) => {
                write!(formatter, "failed to set JACK process callback: {code}")
            }
            Self::StringContainsNull(error) => write!(formatter, "invalid JACK name: {error}"),
            Self::Alsa(error) => write!(formatter, "failed to start ALSA MIDI bridge: {error}"),
        }
    }
}

impl std::error::Error for JackAppError {}

#[derive(Debug)]
enum AlsaMidiError {
    CreateEncoder(i32),
    CreatePort(i32),
    OpenSequencer(i32),
    SetClientName(i32),
    SpawnWorker(io::Error),
    WorkerEndedBeforeReady,
}

impl std::fmt::Display for AlsaMidiError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CreateEncoder(code) => write!(formatter, "failed to create MIDI encoder: {code}"),
            Self::CreatePort(code) => write!(formatter, "failed to create output port: {code}"),
            Self::OpenSequencer(code) => write!(formatter, "failed to open sequencer: {code}"),
            Self::SetClientName(code) => write!(formatter, "failed to set client name: {code}"),
            Self::SpawnWorker(error) => write!(formatter, "failed to spawn worker: {error}"),
            Self::WorkerEndedBeforeReady => {
                write!(formatter, "worker ended before creating the port")
            }
        }
    }
}

impl std::error::Error for AlsaMidiError {}

#[cfg(test)]
mod tests {
    use super::{
        ALSA_EVENT_QUEUE_CAPACITY, AlsaEventQueue, AlsaRecoveryGate, DrumSound, default_pattern,
    };
    use purewave_engine::{MidiChannel, MidiMessage, MidiNote, MidiVelocity, Track};
    use std::sync::Arc;
    use std::thread;

    #[test]
    fn default_pattern_uses_the_initial_playable_rhythm() {
        let pattern = default_pattern();

        assert_eq!(
            enabled_steps(track(&pattern.tracks, DrumSound::Kick)),
            vec![0, 4, 8, 12]
        );
        assert_eq!(
            enabled_steps(track(&pattern.tracks, DrumSound::Snare)),
            vec![4, 12]
        );
        assert_eq!(
            enabled_steps(track(&pattern.tracks, DrumSound::Clap)),
            vec![4, 12]
        );
        assert_eq!(
            enabled_steps(track(&pattern.tracks, DrumSound::HiHat)),
            vec![0, 2, 4, 6, 8, 10, 12, 14]
        );
        assert_eq!(
            enabled_steps(track(&pattern.tracks, DrumSound::Cymbal)),
            vec![0]
        );
    }

    #[test]
    fn alsa_event_queue_preserves_messages_in_order() {
        let queue = AlsaEventQueue::new();
        let first_message = note_on(36);
        let second_message = note_on(38);

        assert!(queue.try_push(first_message));
        assert!(queue.try_push(second_message));

        assert_eq!(queue.try_pop(), Some(first_message));
        assert_eq!(queue.try_pop(), Some(second_message));
        assert_eq!(queue.try_pop(), None);
    }

    #[test]
    fn alsa_event_queue_rejects_events_when_full() {
        let queue = AlsaEventQueue::new();

        for note in 0..(ALSA_EVENT_QUEUE_CAPACITY - 1) {
            assert!(queue.try_push(note_on((note % 128) as u8)));
        }

        assert!(!queue.try_push(note_on(0)));
    }

    #[test]
    fn alsa_event_queue_handles_concurrent_wraparound() {
        const MESSAGE_COUNT: usize = ALSA_EVENT_QUEUE_CAPACITY * 32;

        let queue = Arc::new(AlsaEventQueue::new());
        let producer_queue = Arc::clone(&queue);
        let producer = thread::spawn(move || {
            for index in 0..MESSAGE_COUNT {
                let message = note_on((index % 128) as u8);

                while !producer_queue.try_push(message) {
                    thread::yield_now();
                }
            }
        });

        for index in 0..MESSAGE_COUNT {
            let expected = note_on((index % 128) as u8);
            let message = loop {
                if let Some(message) = queue.try_pop() {
                    break message;
                }

                thread::yield_now();
            };

            assert_eq!(message, expected);
        }

        producer.join().unwrap();
    }

    #[test]
    fn alsa_recovery_gate_blocks_new_producers_until_reopened() {
        let gate = AlsaRecoveryGate::new();

        assert!(gate.try_enter());
        gate.leave();
        gate.close_and_wait();

        assert!(!gate.try_enter());

        gate.open();
        assert!(gate.try_enter());
        gate.leave();
    }

    fn track(tracks: &[Track], sound: DrumSound) -> &Track {
        tracks.iter().find(|track| track.sound == sound).unwrap()
    }

    fn enabled_steps(track: &Track) -> Vec<usize> {
        track
            .steps
            .iter()
            .enumerate()
            .filter_map(|(index, step)| step.enabled.then_some(index))
            .collect()
    }

    fn note_on(note: u8) -> MidiMessage {
        MidiMessage::NoteOn {
            channel: MidiChannel::new(10).unwrap(),
            note: MidiNote::new(note).unwrap(),
            velocity: MidiVelocity::new(100).unwrap(),
        }
    }
}
