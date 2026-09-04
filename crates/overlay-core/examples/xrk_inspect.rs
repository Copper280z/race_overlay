use overlay_core::{AdapterRegistry, SourceId};
use serde_json::json;
use std::path::PathBuf;

fn main() {
    let paths: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    if paths.is_empty() {
        eprintln!("usage: cargo run -p overlay-core --example xrk_inspect -- FILE.xrk [...]");
        std::process::exit(2);
    }
    let registry = AdapterRegistry::with_builtins();
    for path in paths {
        match registry.load("aim_xrk", SourceId::new(), &path, &json!({})) {
            Ok(dataset) => {
                println!(
                    "{}: {} channels, {} laps",
                    path.display(),
                    dataset.channels.len(),
                    dataset.laps.len()
                );
                for channel in dataset.channels.values() {
                    let range = channel.series.samples.iter().fold(
                        (f64::INFINITY, f64::NEG_INFINITY),
                        |(minimum, maximum), sample| {
                            (minimum.min(sample.value), maximum.max(sample.value))
                        },
                    );
                    let start = channel.series.samples.first().map(|sample| sample.time);
                    let end = channel.series.samples.last().map(|sample| sample.time);
                    println!(
                        "  {:28} {:7} n={:7} t={:8.3?}..{:8.3?} value={:10.4}..{:10.4}",
                        channel.descriptor.name,
                        channel.descriptor.unit.symbol(),
                        channel.series.samples.len(),
                        start,
                        end,
                        range.0,
                        range.1,
                    );
                }
                for lap in &dataset.laps {
                    println!(
                        "  lap {}: {:.3}..{:.3} s ({})",
                        lap.number, lap.start_time, lap.end_time, lap.lap_type
                    );
                }
                println!("  metadata: {:?}", dataset.metadata);
            }
            Err(error) => eprintln!("{}: {error}", path.display()),
        }
    }
}
