# Agent Notes

## Project Intent

Purewave is a Rust step sequencer that may grow into a full DAW. Keep changes
small, documented, and friendly to future standalone and plugin builds.

## Product Requirements To Preserve

- Sample-accurate timing is mandatory for both MIDI and audio output.
- Standalone application mode is required.
- Plugin mode is required for VST3, CLAP, and LV2.
- Linux JACK and Windows ASIO are first-class audio backends.
- Raspberry Pi 5 is a first-class platform target.
- Windows WASAPI and macOS CoreAudio are second-class audio backends.
- Third-party audio libraries are not allowed beyond raw language bindings to
  platform and plugin APIs.
- Plugin generation libraries may be used when they do not constrain the engine
  or compromise timing, platform, or plugin-format requirements.
- Architecture must keep a separate app layer and engine layer so additional
  frontend apps can be added without duplicating engine behavior.
- The MVP targets Linux JACK first, with Raspberry Pi 5 coming next.
- The MVP standalone frontend should use Tauri with Solid.
- The MVP sequencer is a 16-step, one-bar, 4/4 MIDI grid with Tom, Kick, Snare,
  HH Closed, HH Open, Cymbal, and Clap tracks.
- The MVP emits MIDI only to an external destination.
- All MVP tracks may initially share one MIDI channel. Their default notes use
  Bitwig labels, where C1 is MIDI note 36: Tom A#1 (46), Kick C1 (36), Snare
  C#1 (37), HH Closed D1 (38), HH Open D#1 (39), Cymbal G1 (43), and Clap C#2
  (49).
- MVP steps store enabled state, note, velocity, and gate length.
- Standalone mode owns internal BPM, play/stop, and loop length controls.
- Plugin mode respects host tempo and transport.
- CLAP is part of the MVP plugin target for Bitwig compatibility. VST3 and LV2
  remain required future plugin formats.
- MIDI control events are deferred to the TODO list.

## Engineering Guidance

- Keep the sequencing/timing core independent from platform audio backends and
  plugin-format glue.
- Put reusable sequencing, timing, transport, and MIDI scheduling code in
  `crates/purewave-engine`.
- Treat `apps/purewave-cli` as a temporary app-layer smoke-test shell until the
  Tauri/Solid frontend is added.
- Keep Linux JACK standalone MIDI output in `apps/purewave-jack`.
- The JACK standalone app also exposes an ALSA sequencer output named
  `Purewave MIDI` so Bitwig can discover it as a Generic MIDI Keyboard input.
  Treat this as a compatibility bridge only: it runs outside the JACK callback
  and does not provide a sample-accurate timing guarantee.
- Keep ALSA bridge delivery off the JACK callback. The JACK MIDI path remains
  the sample-accurate standalone output path.
- Keep ALSA bridge delivery nonblocking so backpressure cannot delay shutdown.
  If its bounded compatibility queue overflows, suppress bridge events, discard
  the backlog, and request a best-effort active note cleanup before resuming.
- `PUREWAVE_LOG=debug` may report successful ALSA bridge sends and active-note
  cleanup messages from its dedicated diagnostic worker. JACK and ALSA bridge
  workers may enqueue bounded diagnostic records but must never perform terminal
  I/O. The reporter must reserve the first non-`WouldBlock` ALSA output error
  from routine diagnostic queue pressure. Do not interpret a successful ALSA
  send as DAW receipt. Dropped diagnostics must not delay MIDI.
- The initial playable seed pattern is Kick on steps 1/5/9/13, Snare and Clap on
  5/13, HH Closed on every odd-numbered step, and Cymbal on step 1. HH Open
  starts empty.
- Keep frontend applications, standalone shells, plugin entry points, and UI
  workflows in the app layer. Keep sequencing, timing, transport, MIDI/audio
  rendering, and backend-facing realtime contracts in the engine layer.
- App-layer code should depend on narrow engine interfaces; it should not own
  sequencing semantics or sample-accurate scheduling.
- Do not put allocation, blocking I/O, logging, or lock-heavy coordination on an
  audio callback path without an explicit design note.
- Prefer small interfaces around platform-specific backends so JACK, ASIO,
  WASAPI, CoreAudio, VST3, CLAP, and LV2 can evolve independently.
- Use raw language bindings for audio and plugin APIs; do not introduce
  higher-level third-party audio libraries or engines.
- Optimize realtime paths only with a clear realtime reason and keep the result
  understandable.
- For realtime code, comments must explain timing math, realtime constraints,
  ownership/lifetime invariants, unsafe or raw-API boundaries, and recovery
  behavior when applicable.
- Standalone JACK integration should use JACK transport where practical.
- The current JACK standalone MVP follows JACK only while it is rolling. In all
  other JACK states it runs a 120 BPM callback-driven internal clock so native
  PipeWire DAWs can receive MIDI without acting as JACK transport controllers.
  A future frontend must expose explicit standalone play/stop and BPM controls.
- Keep CoreAudio MIDI timing concerns in mind for future macOS support; do not
  assume MIDI scheduling is independent from audio timing on every platform.
- Treat Raspberry Pi 5 constraints as design inputs for performance-sensitive
  code.
