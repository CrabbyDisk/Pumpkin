use std::collections::VecDeque;
use std::iter::{self, Enumerate, Map, RepeatN, repeat_n};
use std::sync::Arc;

use async_trait::async_trait;
use crossbeam::channel::{Receiver, Sender};
use crossbeam::deque;
use itertools::multizip;
use pumpkin_data::BlockState;
use pumpkin_data::noise_router::{
    END_BASE_NOISE_ROUTER, NETHER_BASE_NOISE_ROUTER, OVERWORLD_BASE_NOISE_ROUTER,
};
use pumpkin_util::math::{vector2::Vector2, vector3::Vector3};

use super::{
    biome_coords, noise::router::proto_noise_router::ProtoNoiseRouters,
    settings::gen_settings_from_dimension,
};
use crate::chunk::format::LightContainer;
use crate::generation::proto_chunk::TerrainCache;
use crate::level::{ChunkRequest, Level};
use crate::world::BlockRegistryExt;
use crate::{chunk::ChunkLight, dimension::Dimension};
use crate::{
    chunk::{
        ChunkData, ChunkSections, SubChunk,
        palette::{BiomePalette, BlockPalette},
    },
    generation::{GlobalRandomConfig, Seed, proto_chunk::ProtoChunk},
};

pub trait GeneratorInit {
    fn new(seed: Seed, dimension: Dimension) -> Self;
}

pub trait WorldGenerator {
    fn request_load(&self, request: LoadRequest);
}

pub struct VanillaGenerator {
    random_config: GlobalRandomConfig,
    base_router: ProtoNoiseRouters,
    dimension: Dimension,

    terrain_cache: TerrainCache,

    default_block: &'static BlockState,
}

impl GeneratorInit for VanillaGenerator {
    fn new(seed: Seed, dimension: Dimension) -> Self {
        let random_config = GlobalRandomConfig::new(seed.0, false);

        // TODO: The generation settings contains (part of?) the noise routers too; do we keep the separate or
        // use only the generation settings?
        let base = match dimension {
            Dimension::Overworld => OVERWORLD_BASE_NOISE_ROUTER,
            Dimension::Nether => NETHER_BASE_NOISE_ROUTER,
            Dimension::End => END_BASE_NOISE_ROUTER,
        };
        let terrain_cache = TerrainCache::from_random(&random_config);
        let generation_settings = gen_settings_from_dimension(&dimension);

        let default_block = generation_settings.default_block.get_state();
        let base_router = ProtoNoiseRouters::generate(&base, &random_config);
        Self {
            random_config,
            base_router,
            dimension,
            terrain_cache,
            default_block,
        }
    }
}

impl WorldGenerator for VanillaGenerator {
    fn request_load(&self, requested: LoadRequest) {
        let generation_settings = gen_settings_from_dimension(&self.dimension);

        let height: usize = match self.dimension {
            Dimension::Overworld => 384,
            Dimension::Nether | Dimension::End => 256,
        };
        let sub_chunks = height / BlockPalette::SIZE;
        let sections = (0..sub_chunks).map(|_| SubChunk::default()).collect();
        let mut sections = ChunkSections::new(sections, generation_settings.shape.min_y as i32);

        // These are just vanilla constants
        let light_radius = requested.with_padding(1); //Light needs to propagate to adjacent chunks
        let carver_radius = light_radius.with_padding(1); // Terrain shape needs to be complete in order to generate features
        let biome_radius = carver_radius.with_padding(1); // Ishland couldn't find a reason but vanilla does this so ig yes
        let structure_starts_radius = biome_radius.with_padding(8); // Chunks need to store a reference to nearby structures

        multizip((
            requested,
            light_radius,
            carver_radius,
            biome_radius,
            structure_starts_radius,
        ))
        .for_each(
            |(requested, light_radius, carver_radius, biome_radius, structure_starts_radius)| {
                todo!();
            },
        );

        for y in 0..biome_coords::from_block(generation_settings.shape.height) {
            let relative_y = y as usize;
            let section_index = relative_y / BiomePalette::SIZE;
            let relative_y = relative_y % BiomePalette::SIZE;
            if let Some(section) = sections.sections.get_mut(section_index) {
                for z in 0..BiomePalette::SIZE {
                    for x in 0..BiomePalette::SIZE {
                        let absolute_y =
                            biome_coords::from_block(generation_settings.shape.min_y as i32)
                                + y as i32;
                        let biome =
                            proto_chunk.get_biome(&Vector3::new(x as i32, absolute_y, z as i32));
                        section.biomes.set(x, relative_y, z, biome.id);
                    }
                }
            }
        }
        for y in 0..generation_settings.shape.height {
            let relative_y = (y as i32 - sections.min_y) as usize;
            let section_index = relative_y / BlockPalette::SIZE;
            let relative_y = relative_y % BlockPalette::SIZE;
            if let Some(section) = sections.sections.get_mut(section_index) {
                for z in 0..BlockPalette::SIZE {
                    for x in 0..BlockPalette::SIZE {
                        let absolute_y = generation_settings.shape.min_y as i32 + y as i32;
                        let block = proto_chunk
                            .get_block_state(&Vector3::new(x as i32, absolute_y, z as i32));
                        section.block_states.set(x, relative_y, z, block.0);
                    }
                }
            }
        }
    }
}

#[derive(Clone, Copy)]
struct LoadRequest {
    origin: i32,
    radius: u32,
}

const LIGHT_RADIUS: u32 = 1;
const CARVER_RADIUS: u32 = LIGHT_RADIUS + 1;
const BIOME_RADIUS: u32 = CARVER_RADIUS + 1;
const STRUCTURE_STARTS_RADIUS: u32 = CARVER_RADIUS + 8;
impl IntoIterator for LoadRequest {
    type Item = (
        RingIterator,
        RingIterator,
        RingIterator,
        RingIterator,
        RingIterator,
    );

    type IntoIter = Map<Enumerate<RepeatN<Self::Item>>, fn((usize, Self::Item)) -> Self::Item>;

    fn into_iter(self) -> Self::IntoIter {
        // These are just vanilla constants
        let ring = self.with_radius(0).into();
        let light_radius = self.with_radius(LIGHT_RADIUS).into();
        let carver_radius = self.with_radius(CARVER_RADIUS).into(); // Terrain shape needs to be complete in order to generate features
        let biome_radius = self.with_radius(BIOME_RADIUS).into(); // Ishland couldn't find a reason but vanilla does this so ig yes
        let structure_starts_radius = self.with_radius(STRUCTURE_STARTS_RADIUS).into(); // Chunks need to store a reference to nearby structures

        repeat_n(
            (
                ring,
                light_radius,
                carver_radius,
                biome_radius,
                structure_starts_radius,
            ),
            self.radius as usize,
        )
        .enumerate()
        .map(
            |(i, (ring, light_radius, carver_radius, biome_radius, structure_starts_radius))| {
                (
                    ring.with_padding(i as u32),
                    light_radius.with_padding(i as u32),
                    carver_radius.with_padding(i as u32),
                    biome_radius.with_padding(i as u32),
                    structure_starts_radius.with_padding(i as u32),
                )
            },
        )
    }
}
impl LoadRequest {
    const fn with_radius(self, radius: u32) -> Self {
        Self {
            origin: self.origin,
            radius,
        }
    }
}

#[derive(Clone, Copy)]
struct RingIterator {
    index: usize,
    position: i32,
    radius: u32,
}

impl RingIterator {
    fn with_padding(self, padding: u32) -> Self {
        Self {
            index: self.index,
            position: self.position,
            radius: self.radius + padding,
        }
    }
}

impl From<LoadRequest> for RingIterator {
    fn from(value: LoadRequest) -> Self {
        RingIterator {
            index: 0,
            position: value.origin,
            radius: value.radius,
        }
    }
}

impl Iterator for RingIterator {
    type Item = Vector2<i32>;

    fn next(&mut self) -> Option<Self::Item> {
        todo!()
    }
}
/// Call in a new thread
fn initialize_generator(rx: Receiver<LoadRequest>, generator: impl WorldGenerator, level: ()) {
    let mut queue = VecDeque::new();

    let mut poll_countdown = 0;
    loop {
        if poll_countdown == 0 {
            while let Ok(task) = rx.try_recv() {
                queue.push_front(task.into_iter());
            }
            poll_countdown = queue.len(); // Or set it to a constant
        }
        if let Some(mut task) = queue.pop_back() {
            if let Some(work) = task.next() {
                // Do stuff with work
                queue.push_front(task);
            }
        } else {
            // The task queue is empty
            let Ok(value) = rx.recv() else { return }; // Blocks
            queue.push_front(value.into_iter());
        }
        poll_countdown -= 1;
    }
}

fn initialize_pyramid(pos: Vector2<i32>) {
    todo!()
}
