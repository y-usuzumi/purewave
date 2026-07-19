//! Purewave's backend-independent sequencing model and sample-position MIDI scheduler.
//!
//! Applications provide transport information and consume `TimedMidiEvent` values; this crate
//! deliberately knows nothing about JACK, ALSA, a UI framework, or a plugin format.

pub mod midi;
pub mod scheduler;
pub mod sequencer;

pub use midi::{MidiChannel, MidiMessage, MidiNote, MidiVelocity};
pub use scheduler::{BlockPosition, Scheduler, TimedMidiEvent, Transport};
pub use sequencer::{DrumSound, GateLength, Pattern, Step, Track};
