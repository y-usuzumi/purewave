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

## Engineering Guidance

- Keep the sequencing/timing core independent from platform audio backends and
  plugin-format glue.
- Do not put allocation, blocking I/O, logging, or lock-heavy coordination on an
  audio callback path without an explicit design note.
- Prefer small interfaces around platform-specific backends so JACK, ASIO,
  WASAPI, CoreAudio, VST3, CLAP, and LV2 can evolve independently.
- Treat Raspberry Pi 5 constraints as design inputs for performance-sensitive
  code.
- Update this file when new requirements change architecture expectations.
