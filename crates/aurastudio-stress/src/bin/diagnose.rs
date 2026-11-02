//! `cargo run --release -p aurastudio-stress --bin diagnose [-- out_dir]`:
//! every scenario twice, then `diag-report.json` and `diag-report.html`.

use aurastudio_diag::alloc::Counting;
use aurastudio_diag::report::Rules;

#[global_allocator]
static ALLOC: Counting<std::alloc::System> = Counting(std::alloc::System);

fn main() {
    aurastudio_stress::watch_panics();
    let out = std::env::args().nth(1).map_or_else(|| std::path::PathBuf::from("."), std::path::PathBuf::from);
    let mut scenarios = aurastudio_stress::scenarios::all();
    let report = aurastudio_stress::run_all(&mut scenarios, 2, aurastudio_stress::header(), &Rules::default());
    let write = |name: &str, text: String| {
        let p = out.join(name);
        if let Err(e) = std::fs::write(&p, text) { eprintln!("{}: {e}", p.display()); std::process::exit(1); }
        println!("{}", p.display());
    };
    write("diag-report.json", report.to_json());
    write("diag-report.html", report.to_html());
    for s in &report.suspects { println!("suspect: {} ({}) {}", s.rule, s.scenario, s.what); }
    if aurastudio_stress::panicked() { std::process::exit(2); }
}
