# Agent Notes

## Project Intent

Purewave is a Rust step sequencer that may grow into a full DAW. Keep changes
small, documented, and friendly to future standalone and plugin builds.

## Standing Collaboration Rules

- Make incremental commits as work progresses.
- Ask the user before amending any commit before `HEAD`.
- Confirm with the user when unsure about UX or technical direction unless they
  have explicitly allowed proceeding without confirmation.
- Spawn a clean agent to cross-check changes before final handoff.
- Keep `README.md` and `AGENTS.md` updated as requirements and architecture
  decisions evolve.

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
  Hi-hat, Cymbal, and Clap tracks.
- The MVP emits MIDI only to an external destination.
- All MVP tracks may initially share one MIDI channel, using distinct General
  MIDI drum notes where practical.
- MVP steps store enabled state, note, velocity, and gate length.
- Standalone mode owns internal BPM, play/stop, and loop length controls.
- Plugin mode respects host tempo and transport.
- CLAP is part of the MVP plugin target for Bitwig compatibility. VST3 and LV2
  remain required future plugin formats.
- MIDI control events are deferred to the TODO list.

## Engineering Guidance

- Keep the sequencing/timing core independent from platform audio backends and
  plugin-format glue.
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
- Favor readable code over performant-but-hacky code. Optimize only with a clear
  realtime reason and keep the result understandable.
- Standalone JACK integration should use JACK transport where practical.
- Keep CoreAudio MIDI timing concerns in mind for future macOS support; do not
  assume MIDI scheduling is independent from audio timing on every platform.
- Treat Raspberry Pi 5 constraints as design inputs for performance-sensitive
  code.
- Update this file when new requirements change architecture expectations.
