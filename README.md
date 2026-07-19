# Purewave

Purewave is a step sequencer intended to grow toward a full digital audio
workstation over time. The early architecture should keep sequencing, audio
I/O, MIDI I/O, plugin-format entry points, and platform backends separated enough
that the project can expand without rewriting the timing core.

## Current Product Requirements

Long-term Purewave must provide sample-accurate sequencing for both MIDI and
audio output. The MVP emits MIDI only.

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

Audio dependency policy:

- Do not use third-party audio libraries beyond raw language bindings to the
  required platform and plugin APIs.
- Plugin generation libraries are acceptable when they do not constrain the
  engine or prevent Purewave from meeting timing, platform, and format
  requirements.

Architecture layering:

- Keep a separate engine layer for sequencing, timing, transport, MIDI/audio
  rendering, and backend-facing realtime contracts.
- Keep a separate app layer for frontend applications, standalone shells, plugin
  entry points, UI workflows, and platform presentation concerns.
- The layer boundary should make it straightforward to add more frontend apps
  without rewriting or forking the engine.

## Engineering Direction

The timing engine is the center of the application. Work that touches scheduling,
transport, plugin callbacks, MIDI emission, or audio rendering should preserve
sample-accurate behavior across standalone and plugin modes.

Application and plugin frontends should call into the engine through narrow
interfaces. They should not own sequencing semantics, transport rules, or
sample-accurate scheduling.

Backend support should be abstracted behind explicit platform interfaces. JACK,
ASIO, WASAPI, CoreAudio, and plugin-format integrations should not leak into the
core sequencing model. Those integrations should be built on raw language
bindings rather than higher-level third-party audio libraries or engines.

Raspberry Pi 5 is a first-class target, so performance, dependency footprint,
startup behavior, and real-time safety should be considered during design rather
than treated as late portability work.

Comments should explain decisions that are difficult to recover from code alone:
timing math, realtime constraints, ownership/lifetime assumptions, raw API
boundaries, and recovery behavior. Straightforward assignments and control flow
need not be narrated.

## Repository Layout

- `crates/purewave-engine`: reusable sequencing, timing, transport, and MIDI
  scheduling engine.
- `apps/purewave-cli`: temporary app-layer smoke-test shell until the Tauri/Solid
  standalone frontend is added.
- `apps/purewave-jack`: Linux JACK MIDI standalone app for the first playable
  MVP path.

## Current Status

Purewave is not yet a complete standalone sequencer with an editable UI. The
current build contains the first engine slice: the default drum-grid model, MIDI
note message types, and sample-position scheduling tests.

The temporary CLI only confirms that the app layer can link the engine:

```sh
cargo run -p purewave-cli
```

Expected output:

```text
Purewave engine ready: 6 tracks, 16 steps
```

The temporary CLI does not create a JACK client, open MIDI ports, emit MIDI to an
external DAW, or provide a Tauri/Solid UI.

The JACK app is the first playable target. It creates a `purewave:midi_out`
JACK MIDI port and a `Purewave MIDI` ALSA sequencer output for DAW discovery:

```sh
cargo run -p purewave-jack
```

For JACK-aware instruments and DAWs, connect `purewave:midi_out` to a MIDI
input. When JACK transport is rolling, Purewave follows its sample position and
tempo when JACK supplies valid BBT tempo data. When JACK transport is not
rolling (including when a native PipeWire DAW does not drive it), Purewave uses
its own continuously running 120 BPM sample clock. The JACK path is the
sample-accurate MIDI output path.

For Bitwig Studio on Linux, add a `Generic` > `MIDI Keyboard` controller in
Bitwig's Dashboard settings and select the `Purewave MIDI` input port. Use that
controller as a note source for an armed instrument track. This ALSA sequencer
output exists for DAW compatibility; it is
delivered from a dedicated app thread and is not sample-accurate. It must not be
used as the timing reference for future sample-accurate DAW/plugin integration.
It uses nonblocking delivery; if the compatibility queue overflows, Purewave
temporarily discards bridge events and attempts best-effort active-note cleanup
before resuming. Recovery is not guaranteed while the ALSA destination remains
unavailable.

Purewave also reserves an isolated diagnostic report for the first non-
`WouldBlock` ALSA bridge output error, even when per-event debug logging is off.

To confirm that Purewave is forwarding MIDI through the ALSA compatibility
bridge, enable debug event logging:

```sh
PUREWAVE_LOG=debug cargo run -p purewave-jack
```

The terminal may report normal sequenced messages and active-note cleanup with
channel, note, and MIDI bytes; note-ons also include velocity. ALSA output
errors report an error code. It runs on a dedicated diagnostic worker: a slow
terminal can drop debug lines without delaying MIDI. The logs do not confirm
that Bitwig has consumed an event.

The seeded pattern uses Kick on steps 1/5/9/13, Snare and Clap on 5/13, Hi-hat
on every odd-numbered step, and Cymbal on step 1.

If JACK is not running, the app exits with a message asking whether the JACK
server is running.

## MVP Scope

The first implementation target is a Linux JACK standalone application, with
Raspberry Pi 5 support coming next. The standalone frontend should use Tauri
with Solid and call into the engine in-process.

The MVP sequencer is a 16-step, one-bar, 4/4 grid. It starts with six tracks:

- Tom.
- Kick.
- Snare.
- Hi-hat.
- Cymbal.
- Clap.

The MVP emits MIDI only to an external destination. All tracks may initially use
the same MIDI channel, with different MIDI note numbers per sound. The default
mapping should follow General MIDI drum conventions where practical:

- Kick: note 36.
- Snare: note 38.
- Clap: note 39.
- Hi-hat: note 42.
- Tom: note 45.
- Cymbal: note 49.

Each step stores whether it is enabled, plus note, velocity, and gate length.
Standalone mode should provide internal BPM, play/stop, and loop length controls.
When running as a plugin, Purewave should respect host tempo and transport.
Standalone JACK integration should use JACK transport where practical.

CLAP is part of the MVP plugin target because Bitwig Studio supports CLAP and
VST/VST3, but not LV2 as a primary plugin path. VST3 and LV2 remain required
future plugin formats.

## TODO

- Add the Tauri/Solid grid UI.
- Add MIDI control events after the initial note-output MVP.
