use purewave_engine::Pattern;

fn main() {
    // This temporary shell intentionally exercises only the app-to-engine dependency boundary.
    // It does not open an audio/MIDI backend; the JACK app is the playable standalone target.
    let pattern = Pattern::default_drum_grid();

    println!(
        "Purewave engine ready: {} tracks, {} steps",
        pattern.tracks.len(),
        pattern.step_count()
    );
}
