//! Headless N-vs-N siege benchmark on a planned town.
//!
//!   cargo run --release -p pumpkin-kingdoms --bin kingdoms-siege-bench -- --n 200 --ticks 800 --out-dir /tmp/kingdoms-siege

#![allow(
    clippy::print_stdout,
    clippy::print_stderr,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]

use std::env;
use std::fmt::Write as _;
use std::fs;
use std::path::PathBuf;
use std::process::ExitCode;

use pumpkin_kingdoms::height::HeightGrid;
use pumpkin_kingdoms::info::TownInfo;
use pumpkin_kingdoms::palette::PaletteKind;
use pumpkin_kingdoms::plan::build_plan;
use pumpkin_kingdoms::scene::TownInfoScene;
use pumpkin_kingdoms::sim::{SiegeSim, write_plot, write_plot_snapshots};
use pumpkin_kingdoms::town::town_in_cell;

struct Args {
    seed: u64,
    n: usize,
    ticks: u32,
    out_dir: PathBuf,
}

fn parse_args() -> Result<Args, String> {
    let mut seed = 20_261_025u64;
    let mut n = 200usize;
    let mut ticks = 800u32;
    let mut out_dir = PathBuf::from("kingdoms-siege");
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--seed" => {
                seed = args
                    .next()
                    .ok_or("missing --seed")?
                    .parse()
                    .map_err(|e| format!("seed: {e}"))?;
            }
            "--n" => {
                n = args
                    .next()
                    .ok_or("missing --n")?
                    .parse()
                    .map_err(|e| format!("n: {e}"))?;
            }
            "--ticks" => {
                ticks = args
                    .next()
                    .ok_or("missing --ticks")?
                    .parse()
                    .map_err(|e| format!("ticks: {e}"))?;
            }
            "--out-dir" => {
                out_dir = PathBuf::from(args.next().ok_or("missing --out-dir")?);
            }
            "-h" | "--help" => {
                return Err(
                    "kingdoms-siege-bench [--seed N] [--n 200] [--ticks 800] [--out-dir DIR]"
                        .to_owned(),
                );
            }
            other => return Err(format!("unknown arg {other}")),
        }
    }
    Ok(Args {
        seed,
        n,
        ticks,
        out_dir,
    })
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("{err}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let args = parse_args()?;
    let origin = town_in_cell(args.seed, 0, 0);
    let grid = HeightGrid::synthetic_slope(
        origin.x - 360,
        origin.z - 360,
        origin.x + 360,
        origin.z + 360,
        8,
        72,
        10,
        6,
    );
    let plan = build_plan(args.seed, origin, grid, PaletteKind::Oak);
    let scene = TownInfoScene::from_info(TownInfo::from_plan(&plan));
    println!(
        "town {} keep {:?} gates {} lots {}",
        scene.info().id,
        scene.info().castle.keep.center(),
        scene.info().gates.len(),
        scene.info().buildings.len()
    );

    let sizes = if args.n == 200 {
        vec![50usize, 100, 200, 400]
    } else {
        vec![args.n]
    };

    fs::create_dir_all(&args.out_dir).map_err(|e| e.to_string())?;
    let mut json = String::from("{\n  \"runs\": [\n");
    for (i, n) in sizes.iter().enumerate() {
        let mut sim = SiegeSim::from_scene(&scene, *n, *n);
        let report = sim.run(args.ticks, 20);
        println!(
            "N={n} ticks={} ms/tick={:.3} worst={:.3} defenders_left={} raiders_left={} d_kills={} r_kills={} melee={} peak_melee={} path_capped={}/{}",
            report.ticks,
            report.ms_per_tick,
            report.worst_ms,
            report.defenders_left,
            report.raiders_left,
            report.defender_kills,
            report.raider_kills,
            report.melee_engaged,
            report.peak_melee,
            report.path_capped,
            report.path_attempts
        );
        let plot = args.out_dir.join(format!("siege-{n}.png"));
        write_plot(&scene, &report, &plot).map_err(|e| e.to_string())?;
        println!("wrote {}", plot.display());
        for extra in
            write_plot_snapshots(&scene, &report, &args.out_dir, *n).map_err(|e| e.to_string())?
        {
            println!("wrote {}", extra.display());
        }
        let comma = if i + 1 == sizes.len() { "" } else { "," };
        writeln!(
            json,
            "    {{\"n\": {n}, \"ticks\": {}, \"ms_per_tick\": {:.4}, \"worst_ms\": {:.4}, \"defenders_left\": {}, \"raiders_left\": {}, \"defender_kills\": {}, \"raider_kills\": {}, \"melee_engaged\": {}, \"peak_melee\": {}, \"path_capped\": {}, \"path_attempts\": {}}}{comma}",
            report.ticks,
            report.ms_per_tick,
            report.worst_ms,
            report.defenders_left,
            report.raiders_left,
            report.defender_kills,
            report.raider_kills,
            report.melee_engaged,
            report.peak_melee,
            report.path_capped,
            report.path_attempts,
        )
        .map_err(|e| e.to_string())?;
    }
    json.push_str("  ]\n}\n");
    let json_path = args.out_dir.join("siege-bench.json");
    fs::write(&json_path, json).map_err(|e| e.to_string())?;
    println!("wrote {}", json_path.display());
    Ok(())
}
