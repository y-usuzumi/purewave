use crate::midi::MidiMessage;
use crate::sequencer::{Pattern, Step};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transport {
    pub sample_rate_hz: f64,
    pub tempo_bpm: f64,
    pub position_samples: u64,
    pub playing: bool,
}

impl Transport {
    pub const fn new(
        sample_rate_hz: f64,
        tempo_bpm: f64,
        position_samples: u64,
        playing: bool,
    ) -> Self {
        Self {
            sample_rate_hz,
            tempo_bpm,
            position_samples,
            playing,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BlockPosition {
    pub start_sample: u64,
    pub frame_count: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TimedMidiEvent {
    pub frame_offset: u32,
    pub message: MidiMessage,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DroppedEventCount(pub usize);

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Scheduler;

impl Scheduler {
    pub fn schedule_midi_block(
        &self,
        pattern: &Pattern,
        transport: Transport,
        frame_count: u32,
    ) -> Vec<TimedMidiEvent> {
        let mut events = Vec::new();
        self.schedule_midi_block_into(pattern, transport, frame_count, &mut events);
        events
    }

    pub fn schedule_midi_block_into(
        &self,
        pattern: &Pattern,
        transport: Transport,
        frame_count: u32,
        events: &mut Vec<TimedMidiEvent>,
    ) {
        events.clear();

        if !transport.playing
            || frame_count == 0
            || pattern.step_count() == 0
            || pattern.steps_per_beat == 0
            || !is_positive_finite(transport.sample_rate_hz)
            || !is_positive_finite(transport.tempo_bpm)
        {
            return;
        }

        let block = BlockPosition {
            start_sample: transport.position_samples,
            frame_count,
        };

        self.schedule_playing_block(pattern, transport, block, events, false);
    }

    pub fn schedule_midi_block_into_existing_capacity(
        &self,
        pattern: &Pattern,
        transport: Transport,
        frame_count: u32,
        events: &mut Vec<TimedMidiEvent>,
    ) -> DroppedEventCount {
        events.clear();

        if !transport.playing
            || frame_count == 0
            || pattern.step_count() == 0
            || pattern.steps_per_beat == 0
            || !is_positive_finite(transport.sample_rate_hz)
            || !is_positive_finite(transport.tempo_bpm)
        {
            return DroppedEventCount::default();
        }

        let block = BlockPosition {
            start_sample: transport.position_samples,
            frame_count,
        };

        self.schedule_playing_block(pattern, transport, block, events, true)
    }

    fn schedule_playing_block(
        &self,
        pattern: &Pattern,
        transport: Transport,
        block: BlockPosition,
        events: &mut Vec<TimedMidiEvent>,
        use_existing_capacity: bool,
    ) -> DroppedEventCount {
        let samples_per_step = samples_per_step(pattern, transport);
        let block_start = block.start_sample;
        let block_end = block_start + u64::from(block.frame_count);
        let scan_start_step = first_step_to_scan(block_start, samples_per_step);
        let scan_end_step = ((block_end as f64 / samples_per_step).ceil() as i64) + 1;
        let mut dropped = 0;

        for absolute_step in scan_start_step..=scan_end_step {
            if absolute_step < 0 {
                continue;
            }

            let pattern_step = absolute_step as usize % pattern.step_count();
            let note_start = sample_for_step(absolute_step, samples_per_step);

            for track in &pattern.tracks {
                let Some(step) = track.steps.get(pattern_step) else {
                    continue;
                };

                if !step.enabled {
                    continue;
                }

                let note_end = note_start + gate_samples(*step, samples_per_step);

                if note_start >= block_start && note_start < block_end {
                    let event = TimedMidiEvent {
                        frame_offset: (note_start - block_start) as u32,
                        message: MidiMessage::NoteOn {
                            channel: track.channel,
                            note: step.note,
                            velocity: step.velocity,
                        },
                    };

                    if push_event(events, event, use_existing_capacity) {
                        dropped += 1;
                    }
                }

                if note_end >= block_start && note_end < block_end {
                    let event = TimedMidiEvent {
                        frame_offset: (note_end - block_start) as u32,
                        message: MidiMessage::NoteOff {
                            channel: track.channel,
                            note: step.note,
                        },
                    };

                    if push_event(events, event, use_existing_capacity) {
                        dropped += 1;
                    }
                }
            }
        }

        events.sort_unstable_by_key(|event| (event.frame_offset, event.message.sort_priority()));
        DroppedEventCount(dropped)
    }
}

fn push_event(
    events: &mut Vec<TimedMidiEvent>,
    event: TimedMidiEvent,
    use_existing_capacity: bool,
) -> bool {
    if use_existing_capacity && events.len() == events.capacity() {
        return true;
    }

    events.push(event);
    false
}

fn samples_per_step(pattern: &Pattern, transport: Transport) -> f64 {
    let samples_per_beat = transport.sample_rate_hz * 60.0 / transport.tempo_bpm;
    samples_per_beat / f64::from(pattern.steps_per_beat)
}

fn first_step_to_scan(block_start: u64, samples_per_step: f64) -> i64 {
    let earliest_possible_note_start = block_start.saturating_sub(samples_per_step.ceil() as u64);
    (earliest_possible_note_start as f64 / samples_per_step).floor() as i64
}

fn sample_for_step(absolute_step: i64, samples_per_step: f64) -> u64 {
    (absolute_step as f64 * samples_per_step).round() as u64
}

fn gate_samples(step: Step, samples_per_step: f64) -> u64 {
    ((samples_per_step * f64::from(step.gate.percent())) / 100.0).round() as u64
}

fn is_positive_finite(value: f64) -> bool {
    value.is_finite() && value > 0.0
}

#[cfg(test)]
mod tests {
    use super::{Scheduler, Transport};
    use crate::midi::MidiMessage;
    use crate::sequencer::{DrumSound, Pattern};

    #[test]
    fn scheduler_places_note_events_at_sample_offsets() {
        let mut pattern = Pattern::default_drum_grid();
        let kick = pattern
            .tracks
            .iter_mut()
            .find(|track| track.sound == DrumSound::Kick)
            .unwrap();
        kick.enable_step(0);

        let transport = Transport::new(48_000.0, 120.0, 0, true);
        let events = Scheduler.schedule_midi_block(&pattern, transport, 6_001);

        assert_eq!(events.len(), 2);
        assert_eq!(events[0].frame_offset, 0);
        assert_eq!(events[0].message.bytes(), [0x99, 36, 100]);
        assert_eq!(events[1].frame_offset, 3_000);
        assert_eq!(events[1].message, note_off(36));
    }

    #[test]
    fn scheduler_includes_note_off_when_note_started_before_block() {
        let mut pattern = Pattern::default_drum_grid();
        let kick = pattern
            .tracks
            .iter_mut()
            .find(|track| track.sound == DrumSound::Kick)
            .unwrap();
        kick.enable_step(0);

        let transport = Transport::new(48_000.0, 120.0, 2_999, true);
        let events = Scheduler.schedule_midi_block(&pattern, transport, 4);

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].frame_offset, 1);
        assert_eq!(events[0].message, note_off(36));
    }

    #[test]
    fn scheduler_wraps_pattern_at_loop_boundary() {
        let mut pattern = Pattern::default_drum_grid();
        let clap = pattern
            .tracks
            .iter_mut()
            .find(|track| track.sound == DrumSound::Clap)
            .unwrap();
        clap.enable_step(15);

        let step_samples = 6_000;
        let transport = Transport::new(48_000.0, 120.0, 15 * step_samples, true);
        let events = Scheduler.schedule_midi_block(&pattern, transport, (step_samples + 1) as u32);

        assert_eq!(events.len(), 2);
        assert_eq!(events[0].frame_offset, 0);
        assert_eq!(events[0].message.bytes(), [0x99, 39, 100]);
        assert_eq!(events[1].frame_offset, 3_000);
        assert_eq!(events[1].message, note_off(39));
    }

    #[test]
    fn scheduler_restarts_at_step_zero_after_loop_boundary() {
        let mut pattern = Pattern::default_drum_grid();
        let kick = pattern
            .tracks
            .iter_mut()
            .find(|track| track.sound == DrumSound::Kick)
            .unwrap();
        kick.enable_step(0);

        let transport = Transport::new(48_000.0, 120.0, 95_999, true);
        let events = Scheduler.schedule_midi_block(&pattern, transport, 4);

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].frame_offset, 1);
        assert_eq!(events[0].message.bytes(), [0x99, 36, 100]);
    }

    #[test]
    fn scheduler_returns_no_events_when_stopped() {
        let mut pattern = Pattern::default_drum_grid();
        pattern.tracks[0].enable_step(0);

        let transport = Transport::new(48_000.0, 120.0, 0, false);
        let events = Scheduler.schedule_midi_block(&pattern, transport, 512);

        assert!(events.is_empty());
    }

    #[test]
    fn scheduler_returns_no_events_for_invalid_clock_values() {
        let mut pattern = Pattern::default_drum_grid();
        pattern.tracks[0].enable_step(0);

        let zero_tempo = Transport::new(48_000.0, 0.0, 0, true);
        let zero_sample_rate = Transport::new(0.0, 120.0, 0, true);

        assert!(
            Scheduler
                .schedule_midi_block(&pattern, zero_tempo, 512)
                .is_empty()
        );
        assert!(
            Scheduler
                .schedule_midi_block(&pattern, zero_sample_rate, 512)
                .is_empty()
        );
    }

    #[test]
    fn scheduler_can_reuse_existing_event_storage() {
        let mut pattern = Pattern::default_drum_grid();
        pattern.tracks[0].enable_step(0);
        let transport = Transport::new(48_000.0, 120.0, 0, true);
        let mut events = Vec::with_capacity(32);
        let initial_capacity = events.capacity();

        Scheduler.schedule_midi_block_into(&pattern, transport, 6_001, &mut events);

        assert_eq!(events.len(), 2);
        assert_eq!(events.capacity(), initial_capacity);
    }

    #[test]
    fn bounded_scheduler_drops_events_instead_of_growing_storage() {
        let mut pattern = Pattern::default_drum_grid();
        for track in &mut pattern.tracks {
            track.enable_step(0);
        }
        let transport = Transport::new(48_000.0, 120.0, 0, true);
        let mut events = Vec::with_capacity(4);
        let initial_capacity = events.capacity();

        let dropped = Scheduler.schedule_midi_block_into_existing_capacity(
            &pattern,
            transport,
            6_001,
            &mut events,
        );

        assert_eq!(events.capacity(), initial_capacity);
        assert_eq!(events.len(), initial_capacity);
        assert_eq!(dropped.0, 8);
    }

    fn note_off(note: u8) -> MidiMessage {
        MidiMessage::NoteOff {
            channel: crate::midi::MidiChannel::new(10).unwrap(),
            note: crate::midi::MidiNote::new(note).unwrap(),
        }
    }
}
