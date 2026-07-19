/// A user-facing MIDI channel number in the conventional one-based range 1 through 16.
///
/// MIDI status bytes encode that same channel in the zero-based low nibble, which is handled by
/// `status_nibble` rather than exposing that wire-format detail to callers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MidiChannel(u8);

impl MidiChannel {
    pub const MIN: u8 = 1;
    pub const MAX: u8 = 16;

    pub const fn new(channel: u8) -> Option<Self> {
        if channel >= Self::MIN && channel <= Self::MAX {
            Some(Self(channel))
        } else {
            None
        }
    }

    pub const fn get(self) -> u8 {
        self.0
    }

    /// Converts the public channel number into the zero-based MIDI status-byte field.
    pub const fn status_nibble(self) -> u8 {
        self.0 - 1
    }
}

/// A validated MIDI note number, including the General MIDI drum-note range used by the MVP.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MidiNote(u8);

impl MidiNote {
    pub const MIN: u8 = 0;
    pub const MAX: u8 = 127;

    pub const fn new(note: u8) -> Option<Self> {
        if note <= Self::MAX {
            Some(Self(note))
        } else {
            None
        }
    }

    pub const fn get(self) -> u8 {
        self.0
    }
}

/// A validated MIDI note-on velocity.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MidiVelocity(u8);

impl MidiVelocity {
    pub const MIN: u8 = 0;
    pub const MAX: u8 = 127;

    pub const fn new(velocity: u8) -> Option<Self> {
        if velocity <= Self::MAX {
            Some(Self(velocity))
        } else {
            None
        }
    }

    pub const fn get(self) -> u8 {
        self.0
    }
}

/// The MIDI messages currently emitted by the sequencer.
///
/// Control changes and other message types will be added with the deferred MIDI-control work.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MidiMessage {
    NoteOn {
        channel: MidiChannel,
        note: MidiNote,
        velocity: MidiVelocity,
    },
    NoteOff {
        channel: MidiChannel,
        note: MidiNote,
    },
}

impl MidiMessage {
    /// Encodes this high-level message as its three-byte MIDI 1.0 representation.
    pub const fn bytes(self) -> [u8; 3] {
        match self {
            Self::NoteOn {
                channel,
                note,
                velocity,
            } => [0x90 | channel.status_nibble(), note.get(), velocity.get()],
            Self::NoteOff { channel, note } => [0x80 | channel.status_nibble(), note.get(), 0],
        }
    }

    /// Ensures a note-off is emitted before a same-sample note-on for the same note.
    pub const fn sort_priority(self) -> u8 {
        match self {
            Self::NoteOff { .. } => 0,
            Self::NoteOn { .. } => 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{MidiChannel, MidiMessage, MidiNote, MidiVelocity};

    #[test]
    fn note_on_bytes_use_one_based_public_channels() {
        let message = MidiMessage::NoteOn {
            channel: MidiChannel::new(10).unwrap(),
            note: MidiNote::new(36).unwrap(),
            velocity: MidiVelocity::new(100).unwrap(),
        };

        assert_eq!(message.bytes(), [0x99, 36, 100]);
    }
}
