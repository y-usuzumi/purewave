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

    pub const fn status_nibble(self) -> u8 {
        self.0 - 1
    }
}

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
