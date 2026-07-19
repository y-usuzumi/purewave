use jack_sys::{
    JackPortIsOutput, JackPositionBBT, JackTransportRolling, JackUseExactName, RAW_MIDI_TYPE,
    jack_activate, jack_client_close, jack_client_open, jack_client_t, jack_deactivate,
    jack_get_sample_rate, jack_midi_clear_buffer, jack_midi_event_write, jack_nframes_t,
    jack_port_get_buffer, jack_port_register, jack_port_t, jack_position_t,
    jack_set_process_callback, jack_status_t, jack_transport_query,
};
use purewave_engine::{DrumSound, Pattern, Scheduler, TimedMidiEvent, Transport};
use std::ffi::{CString, c_void};
use std::io;
use std::mem::MaybeUninit;
use std::process::ExitCode;

const CLIENT_NAME: &str = "purewave";
const MIDI_OUT_PORT: &str = "midi_out";
const DEFAULT_TEMPO_BPM: f64 = 120.0;
const MAX_EVENTS_PER_BLOCK: usize = 128;

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
    let mut client = JackMidiClient::open(default_pattern())?;
    client.activate()?;

    println!("Purewave JACK MIDI client is running.");
    println!("Connect purewave:midi_out to your DAW or instrument, then start JACK transport.");
    println!("Press Enter to stop.");

    wait_for_enter();
    client.deactivate();

    Ok(())
}

struct JackMidiClient {
    client: *mut jack_client_t,
    _state: Box<ProcessState>,
    active: bool,
}

impl JackMidiClient {
    fn open(pattern: Pattern) -> Result<Self, JackAppError> {
        let client_name = CString::new(CLIENT_NAME)?;
        let port_name = CString::new(MIDI_OUT_PORT)?;
        let midi_type = CString::new(RAW_MIDI_TYPE)?;
        let mut status = MaybeUninit::<jack_status_t>::zeroed();

        let client = unsafe {
            jack_client_open(client_name.as_ptr(), JackUseExactName, status.as_mut_ptr())
        };

        if client.is_null() {
            let status = unsafe { status.assume_init() };
            return Err(JackAppError::OpenClient(status));
        }

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

        let mut state = Box::new(ProcessState::new(client, midi_out, pattern));
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
    client: *mut jack_client_t,
    midi_out: *mut jack_port_t,
    pattern: Pattern,
    scheduler: Scheduler,
    events: Vec<TimedMidiEvent>,
    fallback_tempo_bpm: f64,
}

impl ProcessState {
    fn new(client: *mut jack_client_t, midi_out: *mut jack_port_t, pattern: Pattern) -> Self {
        let mut events = Vec::with_capacity(MAX_EVENTS_PER_BLOCK);
        events.reserve_exact(MAX_EVENTS_PER_BLOCK.saturating_sub(events.capacity()));

        Self {
            client,
            midi_out,
            pattern,
            scheduler: Scheduler,
            events,
            fallback_tempo_bpm: DEFAULT_TEMPO_BPM,
        }
    }

    fn process(&mut self, frame_count: jack_nframes_t) {
        let Some(midi_buffer) = self.midi_buffer(frame_count) else {
            return;
        };

        unsafe {
            jack_midi_clear_buffer(midi_buffer);
        }

        let transport = self.transport();
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

    let state = unsafe { &mut *(arg as *mut ProcessState) };
    state.process(frame_count);
    0
}

fn default_pattern() -> Pattern {
    let mut pattern = Pattern::default_drum_grid();

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

#[derive(Debug)]
enum JackAppError {
    Activate(i32),
    OpenClient(jack_status_t),
    RegisterPort,
    SetProcessCallback(i32),
    StringContainsNull(std::ffi::NulError),
}

impl From<std::ffi::NulError> for JackAppError {
    fn from(error: std::ffi::NulError) -> Self {
        Self::StringContainsNull(error)
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
        }
    }
}

impl std::error::Error for JackAppError {}

#[cfg(test)]
mod tests {
    use super::{DrumSound, default_pattern};
    use purewave_engine::Track;

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
}
