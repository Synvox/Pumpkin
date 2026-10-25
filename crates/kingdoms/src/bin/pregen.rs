//! Offline planner, determinism check, and PNG renderer.
//!
//! Usage:
//!   cargo run -p pumpkin-kingdoms --bin kingdoms-pregen -- --seed 20261025 --cell 0,0 --out-dir renders

#![allow(
    clippy::print_stdout,
    clippy::print_stderr,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::too_many_lines
)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use std::env;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use pumpkin_data::dimension::Dimension;
use pumpkin_kingdoms::height::register_world_gen;
use pumpkin_kingdoms::marker;
use pumpkin_kingdoms::palette::PaletteKind;
use pumpkin_kingdoms::plan::{plan_for, plan_from_cache};
use pumpkin_kingdoms::render::write_renders;
use pumpkin_kingdoms::stamp::{MARKER_FLOOR, fill_volume};
use pumpkin_kingdoms::town::{CELL_SIZE, nearest_town, town_in_cell};
use pumpkin_util::world_seed::Seed;
use pumpkin_world::generation::generator::WorldGenerator;
use pumpkin_world::generation::get_world_gen;

fn parse_args() -> Result<Args, String> {
    let mut seed = 20_261_025u64;
    let mut cell_x = 0;
    let mut cell_z = 0;
    let mut out_dir = PathBuf::from("kingdoms-renders");
    let mut all = false;
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--seed" => {
                seed = args
                    .next()
                    .ok_or("missing --seed value")?
                    .parse()
                    .map_err(|_| "invalid --seed")?;
            }
            "--cell" => {
                let value = args.next().ok_or("missing --cell value")?;
                let (x, z) = value.split_once(',').ok_or("expected --cell x,z")?;
                cell_x = x.parse().map_err(|_| "invalid cell x")?;
                cell_z = z.parse().map_err(|_| "invalid cell z")?;
            }
            "--out-dir" | "--out" => {
                out_dir = PathBuf::from(args.next().ok_or("missing --out-dir value")?);
            }
            "--all" => all = true,
            "--help" | "-h" => {
                println!(
                    "kingdoms-pregen --seed <u64> --cell <x,z> --out-dir <dir> [--all]\n\
                     Plans a town from seed+cell, checks plan determinism, writes PNG renders."
                );
                return Err(String::new());
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    Ok(Args {
        seed,
        cell_x,
        cell_z,
        out_dir,
        all,
    })
}

struct Args {
    seed: u64,
    cell_x: i32,
    cell_z: i32,
    out_dir: PathBuf,
    all: bool,
}

fn make_gen(seed: u64) -> Arc<WorldGenerator> {
    let boxed = get_world_gen(
        Seed(seed),
        Dimension::OVERWORLD,
        false,
        Vec::new(),
        String::new(),
    );
    let world_gen = Arc::from(boxed);
    register_world_gen(Arc::clone(&world_gen));
    marker::register_hooks(world_gen.extensions());
    world_gen
}

fn render_one(seed: u64, cell_x: i32, cell_z: i32, out_dir: &std::path::Path, stem: &str) -> bool {
    let world_gen = make_gen(seed);
    let town = town_in_cell(seed, cell_x, cell_z);
    println!(
        "seed {seed} cell ({cell_x},{cell_z}) origin ({}, {}) cell size {CELL_SIZE}",
        town.x, town.z
    );
    let plan_a = plan_for(seed, cell_x, cell_z, &world_gen);
    let Some(plan_b) = plan_from_cache(seed, cell_x, cell_z) else {
        eprintln!("plan missing from cache after build");
        return false;
    };
    if plan_a.castle.bounds != plan_b.castle.bounds || plan_a.well != plan_b.well {
        eprintln!("plan mismatch between two cache reads");
        return false;
    }
    let (w, d) = plan_a.castle_footprint();
    println!(
        "  castle {}x{} at {:?} well {:?} palette {:?} lots {}",
        w,
        d,
        plan_a.castle.bounds,
        plan_a.well,
        plan_a.palette,
        plan_a.lots.len()
    );
    if w < 128 || d < 128 {
        eprintln!("castle smaller than 128x128");
        return false;
    }

    let vol_a = fill_volume(&plan_a);
    let vol_b = fill_volume(&plan_b);
    let mut mismatch = 0u32;
    for z in plan_a.bounds.min_z..=plan_a.bounds.max_z {
        for x in plan_a.bounds.min_x..=plan_a.bounds.max_x {
            if vol_a.top(x, z).map(|t| t.1.id) != vol_b.top(x, z).map(|t| t.1.id) {
                mismatch += 1;
            }
        }
    }
    if mismatch != 0 {
        eprintln!("{mismatch} top-block mismatches between two plan fills");
        return false;
    }
    println!("  two plan fills produced the same top blocks");

    let well_ok = vol_a
        .top(plan_a.well.0, plan_a.well.1)
        .is_some_and(|(_, s)| s.id == MARKER_FLOOR.default_state.id || !s.is_air());
    if !well_ok {
        println!("  (well column is built; mossy floor may sit under water)");
    }

    if let Err(error) = write_renders(&plan_a, out_dir, stem) {
        eprintln!("render failed: {error}");
        return false;
    }
    println!("  wrote {}/{stem}-*.png", out_dir.display());
    let _ = nearest_town(seed, 0, 0);
    true
}

fn find_variant_seeds() -> Vec<(u64, i32, i32, String)> {
    let mut out = vec![(20_261_025u64, 0, 0, "plains".to_string())];
    for seed in [7u64, 42, 2026, 314_159, 1_234_567, 88_512, 9_001] {
        let world_gen = make_gen(seed);
        let plan = plan_for(seed, 0, 0, &world_gen);
        let (lo, hi) = plan.heights.range(
            plan.castle.bounds.min_x,
            plan.castle.bounds.min_z,
            plan.castle.bounds.max_x,
            plan.castle.bounds.max_z,
        );
        let relief = hi - lo;
        let label = match plan.palette {
            PaletteKind::Spruce if relief >= 10 => "taiga-hill",
            PaletteKind::Spruce => "taiga",
            PaletteKind::Sandstone => "desert",
            PaletteKind::Oak if relief >= 12 => "hillside",
            PaletteKind::Oak => "plains",
        };
        if (label.contains("hill") || label.contains("taiga") || label.contains("desert"))
            && !out.iter().any(|(_, _, _, l)| l == label)
        {
            out.push((seed, 0, 0, label.to_string()));
        }
        if out.len() >= 4 {
            break;
        }
    }
    out
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(args) => args,
        Err(message) => {
            if !message.is_empty() {
                eprintln!("{message}");
                return ExitCode::from(2);
            }
            return ExitCode::SUCCESS;
        }
    };

    let jobs = if args.all {
        find_variant_seeds()
    } else {
        vec![(args.seed, args.cell_x, args.cell_z, "town".to_string())]
    };

    let mut ok = true;
    for (seed, cx, cz, label) in jobs {
        let stem = format!("town-{label}-{seed}");
        if !render_one(seed, cx, cz, &args.out_dir, &stem) {
            ok = false;
        }
    }
    if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}
