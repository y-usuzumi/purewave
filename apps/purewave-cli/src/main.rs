use purewave_engine::Pattern;

fn main() {
    let pattern = Pattern::default_drum_grid();

    println!(
        "Purewave engine ready: {} tracks, {} steps",
        pattern.tracks.len(),
        pattern.step_count()
    );
}
