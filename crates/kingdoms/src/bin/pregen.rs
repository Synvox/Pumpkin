//! Dev tool: generate the chunks around a town cell and write a top-down PNG.
//!
//! Usage (from a scratch directory or the repo root):
//!   cargo run -p pumpkin-kingdoms --bin kingdoms-pregen -- --seed 20261025 --out town.png

#![allow(clippy::print_stdout, clippy::print_stderr)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use std::collections::HashMap;
use std::env;
use std::path::PathBuf;
use std::process::ExitCode;

use image::{ImageBuffer, Rgb};
use pumpkin_data::Block;
use pumpkin_data::dimension::Dimension;
use pumpkin_kingdoms::marker::MARKER_FLOOR;
use pumpkin_kingdoms::town::{CELL_SIZE, nearest_town, town_in_cell};
use pumpkin_kingdoms::{marker, town};
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::world_seed::Seed;
use pumpkin_world::chunk_system::{Chunk, StagedChunkEnum, generate_single_chunk};
use pumpkin_world::generation::get_world_gen;
use pumpkin_world::generation::proto_chunk::{GenerationCache, ProtoChunk};
use pumpkin_world::world::{BlockAccessor, WorldPortalExt};

const IMAGE_SCALE: u32 = 8;

struct TestPortal;

impl WorldPortalExt for TestPortal {
    fn can_place_at(
        &self,
        _block: &pumpkin_data::Block,
        _state: &pumpkin_data::BlockState,
        _block_accessor: &dyn BlockAccessor,
        _block_pos: &pumpkin_util::math::position::BlockPos,
    ) -> bool {
        true
    }

    fn mirror(
        &self,
        block: &pumpkin_data::Block,
        state_id: pumpkin_data::BlockStateId,
        mirror: pumpkin_data::Mirror,
    ) -> &'static pumpkin_data::BlockState {
        block.mirror(state_id, mirror)
    }

    fn rotate(
        &self,
        block: &pumpkin_data::Block,
        state_id: pumpkin_data::BlockStateId,
        rotation: pumpkin_data::Rotation,
    ) -> &'static pumpkin_data::BlockState {
        block.rotate(state_id, rotation)
    }

    fn spawn_mobs_for_chunk_generation(
        &self,
        _cache: &mut dyn GenerationCache,
        _biome: &'static pumpkin_data::chunk::Biome,
        _chunk_x: i32,
        _chunk_z: i32,
    ) {
    }
}

fn extract_proto(chunk: Chunk) -> ProtoChunk {
    match chunk {
        Chunk::Proto(proto) => *proto,
        Chunk::Level(_) => {
            eprintln!("expected a proto chunk before Full");
            std::process::exit(1);
        }
    }
}

fn parse_args() -> Result<(u64, i32, i32, PathBuf), String> {
    let mut seed = 20_261_025u64;
    let mut cell_x = 0;
    let mut cell_z = 0;
    let mut out = PathBuf::from("kingdoms-town.png");
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
            "--out" => {
                out = PathBuf::from(args.next().ok_or("missing --out value")?);
            }
            "--help" | "-h" => {
                println!(
                    "kingdoms-pregen --seed <u64> --cell <x,z> --out <png>\n\
                     Generates Features-stage chunks around one town cell and writes a top-down PNG."
                );
                return Err(String::new());
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    Ok((seed, cell_x, cell_z, out))
}

fn block_color(id: pumpkin_data::BlockStateId) -> Rgb<u8> {
    if id == MARKER_FLOOR.default_state.id {
        return Rgb([80, 140, 70]);
    }
    if id == Block::WATER.default_state.id {
        return Rgb([50, 90, 200]);
    }
    if id == Block::COBBLESTONE.default_state.id {
        return Rgb([120, 120, 120]);
    }
    if id == Block::OAK_LOG.default_state.id {
        return Rgb([110, 80, 40]);
    }
    if id == Block::GRASS_BLOCK.default_state.id {
        return Rgb([70, 160, 60]);
    }
    if id == Block::DIRT.default_state.id {
        return Rgb([120, 85, 50]);
    }
    if id == Block::STONE.default_state.id {
        return Rgb([90, 90, 90]);
    }
    if id == Block::SAND.default_state.id {
        return Rgb([210, 200, 140]);
    }
    Rgb([40, 40, 40])
}

fn top_block(chunk: &ProtoChunk, x: i32, z: i32) -> pumpkin_data::BlockStateId {
    let y = GenerationCache::top_motion_blocking_block_height_exclusive(chunk, x, z) - 1;
    chunk.get_block_state(&Vector3::new(x, y, z))
}

fn generate_around_town(
    seed: u64,
    town: town::TownPos,
    order: &[(i32, i32)],
) -> HashMap<(i32, i32), ProtoChunk> {
    let world_gen = get_world_gen(
        Seed(seed),
        Dimension::OVERWORLD,
        false,
        Vec::new(),
        String::new(),
    );
    marker::register_hooks(world_gen.extensions());

    let mut chunks = HashMap::new();
    for &(cx, cz) in order {
        let generated =
            generate_single_chunk(&world_gen, &TestPortal, cx, cz, StagedChunkEnum::Features);
        chunks.insert((cx, cz), extract_proto(generated));
    }
    let _ = town;
    chunks
}

fn well_chunks(town_x: i32, town_z: i32) -> Vec<(i32, i32)> {
    chunks_covering(town_x, town_z, 2)
}

fn chunks_covering(center_x: i32, center_z: i32, radius: i32) -> Vec<(i32, i32)> {
    let mut set = Vec::new();
    for dx in -radius..=radius {
        for dz in -radius..=radius {
            let key = ((center_x + dx) >> 4, (center_z + dz) >> 4);
            if !set.contains(&key) {
                set.push(key);
            }
        }
    }
    set
}

fn main() -> ExitCode {
    let (seed, cell_x, cell_z, out) = match parse_args() {
        Ok(args) => args,
        Err(message) => {
            if !message.is_empty() {
                eprintln!("{message}");
                return ExitCode::from(2);
            }
            return ExitCode::SUCCESS;
        }
    };

    let town = town_in_cell(seed, cell_x, cell_z);
    let nearest = nearest_town(seed, 0, 0);
    println!(
        "seed {seed} cell ({cell_x},{cell_z}) town ({}, {}) nearest-to-origin ({}, {}) cell size {CELL_SIZE}",
        town.x, town.z, nearest.x, nearest.z
    );

    let mut chunks_needed = well_chunks(town.x, town.z);
    chunks_needed.sort_unstable();
    let mut reverse = chunks_needed.clone();
    reverse.reverse();

    let first = generate_around_town(seed, town, &chunks_needed);
    let second = generate_around_town(seed, town, &reverse);

    let mut mismatch = 0u32;
    for &(cx, cz) in &chunks_needed {
        let Some(a) = first.get(&(cx, cz)) else {
            continue;
        };
        let Some(b) = second.get(&(cx, cz)) else {
            mismatch += 1;
            continue;
        };
        for lx in 0..16 {
            for lz in 0..16 {
                let x = cx * 16 + lx;
                let z = cz * 16 + lz;
                if top_block(a, x, z) != top_block(b, x, z) {
                    mismatch += 1;
                }
            }
        }
    }
    if mismatch == 0 {
        println!("two generation orders produced the same top blocks around the well");
    } else {
        eprintln!("{mismatch} top-block mismatches between generation orders");
        return ExitCode::from(1);
    }

    let radius = 24;
    let view = chunks_covering(town.x, town.z, radius);
    let extra: Vec<(i32, i32)> = view
        .into_iter()
        .filter(|chunk| !first.contains_key(chunk))
        .collect();
    let extra_chunks = generate_around_town(seed, town, &extra);
    let mut tiles = first;
    tiles.extend(extra_chunks);

    let size = u32::try_from(radius * 2 + 1).unwrap_or(1);
    let mut image = ImageBuffer::new(size, size);
    for dz in -radius..=radius {
        for dx in -radius..=radius {
            let x = town.x + dx;
            let z = town.z + dz;
            let key = (x >> 4, z >> 4);
            let color = tiles
                .get(&key)
                .map_or(Rgb([0, 0, 0]), |chunk| block_color(top_block(chunk, x, z)));
            let px = u32::try_from(dx + radius).unwrap_or(0);
            let pz = u32::try_from(dz + radius).unwrap_or(0);
            image.put_pixel(px, pz, color);
        }
    }

    let mut scaled = ImageBuffer::new(size * IMAGE_SCALE, size * IMAGE_SCALE);
    for y in 0..size {
        for x in 0..size {
            let pixel = *image.get_pixel(x, y);
            for sy in 0..IMAGE_SCALE {
                for sx in 0..IMAGE_SCALE {
                    scaled.put_pixel(x * IMAGE_SCALE + sx, y * IMAGE_SCALE + sy, pixel);
                }
            }
        }
    }

    if let Some(parent) = out.parent()
        && !parent.as_os_str().is_empty()
    {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Err(error) = scaled.save(&out) {
        eprintln!("failed to write {}: {error}", out.display());
        return ExitCode::from(1);
    }
    println!("wrote {}", out.display());
    ExitCode::SUCCESS
}
