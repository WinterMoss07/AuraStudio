//! `cargo run --release -p aurastudio-stress --bin diagnose [-- out_dir
//! [baseline.json]]`: every scenario twice, then `diag-report.json` and
//! `diag-report.html`; with a baseline, what changed since it.

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
    if let Some(base) = std::env::args().nth(2) {
        match std::fs::read_to_string(&base).ok().and_then(|b| report.compare(&b)) {
            Some(d) if d.is_empty() => println!("no change above the noise against {base}"),
            Some(d) => for x in d { println!("change: {} {}: {:.0} -> {:.0} (x{:.2})", x.scenario, x.what, x.before, x.after, x.ratio()); },
            None => eprintln!("{base}: not a report"),
        }
    }
    if aurastudio_stress::panicked() { std::process::exit(2); }
}
