# Purewave

Purewave is a step sequencer intended to grow toward a full digital audio
workstation over time. The early architecture should keep sequencing, audio
I/O, MIDI I/O, plugin hosting/export concerns, and platform backends separated
enough that the project can expand without rewriting the timing core.

## Current Product Requirements

Purewave must provide sample-accurate sequencing for both MIDI and audio
output.

Target deployment modes:

- Standalone application.
- DAW plugin formats: VST3, CLAP, and LV2.

First-class audio backend support:

- Linux JACK.
- Windows ASIO.

First-class platform targets:

- Raspberry Pi 5.

Second-class audio backend support:

- Windows WASAPI.
- macOS CoreAudio.

## Engineering Direction

The timing engine is the center of the application. Work that touches scheduling,
transport, plugin callbacks, MIDI emission, or audio rendering should preserve
sample-accurate behavior across standalone and plugin modes.

Backend support should be abstracted behind explicit platform interfaces. JACK,
ASIO, WASAPI, CoreAudio, and plugin-format integrations should not leak into the
core sequencing model.

Raspberry Pi 5 is a first-class target, so performance, dependency footprint,
startup behavior, and real-time safety should be considered during design rather
than treated as late portability work.
