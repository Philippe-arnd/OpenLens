//! Write the hostile inputs of `openlens-testkit` as the first corpus of
//! each target: `cargo run --bin seed` (from `fuzz/`).

fn main() -> std::io::Result<()> {
    let bodies = openlens_testkit::hostile_capture_bodies();
    for target in ["capture_batch", "feature_map", "error_stack", "page_url"] {
        let dir = std::path::Path::new("corpus").join(target);
        std::fs::create_dir_all(&dir)?;
        for (i, body) in bodies.iter().enumerate() {
            std::fs::write(dir.join(format!("hostile-{i}")), body)?;
        }
    }
    println!("seeded {} inputs per target", bodies.len());
    Ok(())
}
