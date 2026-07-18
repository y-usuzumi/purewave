use crate::midi::{MidiChannel, MidiNote, MidiVelocity};

pub const DEFAULT_STEP_COUNT: usize = 16;
pub const DEFAULT_BEATS_PER_BAR: u8 = 4;
pub const DEFAULT_STEPS_PER_BEAT: u8 = 4;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DrumSound {
    Tom,
    Kick,
    Snare,
    HiHat,
    Cymbal,
    Clap,
}

impl DrumSound {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Tom => "Tom",
            Self::Kick => "Kick",
            Self::Snare => "Snare",
            Self::HiHat => "Hi-hat",
            Self::Cymbal => "Cymbal",
            Self::Clap => "Clap",
        }
    }

    pub const fn default_note(self) -> MidiNote {
        match self {
            Self::Tom => MidiNote::new(45).unwrap(),
            Self::Kick => MidiNote::new(36).unwrap(),
            Self::Snare => MidiNote::new(38).unwrap(),
            Self::HiHat => MidiNote::new(42).unwrap(),
            Self::Cymbal => MidiNote::new(49).unwrap(),
            Self::Clap => MidiNote::new(39).unwrap(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GateLength {
    percent: u8,
}

impl GateLength {
    pub const MIN_PERCENT: u8 = 1;
    pub const MAX_PERCENT: u8 = 100;

    pub const fn new(percent: u8) -> Option<Self> {
        if percent >= Self::MIN_PERCENT && percent <= Self::MAX_PERCENT {
            Some(Self { percent })
        } else {
            None
        }
    }

    pub const fn half_step() -> Self {
        Self { percent: 50 }
    }

    pub const fn percent(self) -> u8 {
        self.percent
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Step {
    pub enabled: bool,
    pub note: MidiNote,
    pub velocity: MidiVelocity,
    pub gate: GateLength,
}

impl Step {
    pub const fn disabled(note: MidiNote) -> Self {
        Self {
            enabled: false,
            note,
            velocity: MidiVelocity::new(100).unwrap(),
            gate: GateLength::half_step(),
        }
    }

    pub const fn enabled(note: MidiNote, velocity: MidiVelocity, gate: GateLength) -> Self {
        Self {
            enabled: true,
            note,
            velocity,
            gate,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Track {
    pub sound: DrumSound,
    pub channel: MidiChannel,
    pub steps: Vec<Step>,
}

impl Track {
    pub fn new(sound: DrumSound, channel: MidiChannel, step_count: usize) -> Self {
        let default_step = Step::disabled(sound.default_note());

        Self {
            sound,
            channel,
            steps: vec![default_step; step_count],
        }
    }

    pub fn enable_step(&mut self, step_index: usize) {
        if let Some(step) = self.steps.get_mut(step_index) {
            step.enabled = true;
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Pattern {
    pub beats_per_bar: u8,
    pub steps_per_beat: u8,
    pub tracks: Vec<Track>,
}

impl Pattern {
    pub fn default_drum_grid() -> Self {
        let channel = MidiChannel::new(10).unwrap();
        let sounds = [
            DrumSound::Tom,
            DrumSound::Kick,
            DrumSound::Snare,
            DrumSound::HiHat,
            DrumSound::Cymbal,
            DrumSound::Clap,
        ];

        let tracks = sounds
            .into_iter()
            .map(|sound| Track::new(sound, channel, DEFAULT_STEP_COUNT))
            .collect();

        Self {
            beats_per_bar: DEFAULT_BEATS_PER_BAR,
            steps_per_beat: DEFAULT_STEPS_PER_BEAT,
            tracks,
        }
    }

    pub fn step_count(&self) -> usize {
        self.tracks
            .first()
            .map(|track| track.steps.len())
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::{DrumSound, Pattern};

    #[test]
    fn default_grid_uses_requested_drum_tracks_and_notes() {
        let pattern = Pattern::default_drum_grid();
        let sounds_and_notes: Vec<_> = pattern
            .tracks
            .iter()
            .map(|track| (track.sound, track.steps[0].note.get(), track.channel.get()))
            .collect();

        assert_eq!(
            sounds_and_notes,
            vec![
                (DrumSound::Tom, 45, 10),
                (DrumSound::Kick, 36, 10),
                (DrumSound::Snare, 38, 10),
                (DrumSound::HiHat, 42, 10),
                (DrumSound::Cymbal, 49, 10),
                (DrumSound::Clap, 39, 10),
            ]
        );
        assert_eq!(pattern.step_count(), 16);
    }
}
