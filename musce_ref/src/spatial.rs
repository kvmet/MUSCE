//! The reference game's coordinate layer: an integer `XYZ` on rooms, and the
//! secondary indexes that answer range queries over it without scanning every
//! room. This is game vocabulary, not engine machinery: the engine never reads a
//! coordinate; it only carries the `xyz` component and the `ComponentChanged`
//! trigger the index rides on. So it lives here and registers through
//! `Game.register`/`Game.systems`, like any other game type.
//!
//! Two indexes read the one `xyz` component with different keys: `xyz_cell`, a
//! spatial hash keyed by the cell a room falls in (so `near` retrieves a region by
//! unioning the buckets of the cells it covers), and `xyz_level`, a bucket per
//! z-level. One `xyz` write fans out to both through the component-keyed trigger.
//! The registry is derived,
//! in-memory state held in a `World` resource and rebuilt on boot; nothing about
//! it persists. See `docs/architecture/indexes.md`.

use musce::action::SystemCtx;
use musce::index::{IndexRegistry, Policy};
use musce::world::{EntityId, NamedComponent, World};
use serde::{Deserialize, Serialize};

/// A room's integer position. Rooms only: it exists for range and line-of-sight
/// queries between places, while ordinary containment (a thing in a room) stays
/// room-based. Registered and tracked in [`register`], so it persists and every
/// write feeds the index.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Xyz {
    pub x: i64,
    pub y: i64,
    pub z: i64,
}

impl NamedComponent for Xyz {
    const TAG: &'static str = "xyz";
}

/// Edge length of a spatial-hash cell, in coordinate units, and the grid a range
/// query quantizes to. `near` retrieves every room in the cells a query region
/// covers, so this is a precision/lookups tradeoff: smaller cells hug the requested
/// radius more tightly but make more bucket lookups; larger cells mean fewer
/// lookups over a coarser region. Also the default `@nearby` radius.
pub const CELL: i64 = 10;

/// The spatial-hash index (range queries) and the z-level index, by name.
const CELL_INDEX: &str = "xyz_cell";
const LEVEL_INDEX: &str = "xyz_level";

/// A spatial-hash cell coordinate: a room's `Xyz` floored to the cell grid.
type Cell = (i64, i64, i64);

fn cell_of(p: &Xyz) -> Cell {
    (
        p.x.div_euclid(CELL),
        p.y.div_euclid(CELL),
        p.z.div_euclid(CELL),
    )
}

/// Register the game's `xyz` component and its two indexes' wiring: the component
/// is registered (so coordinates persist) and tracked (so writes feed the index).
/// Called from `Game.register`, before load or seed.
pub(crate) fn register(world: &mut World) {
    world.register_component::<Xyz>();
    world.track_component::<Xyz>();
}

/// Register the game's spatial indexes into `reg`. Shared by the maintainer's boot
/// build and the benchmarks, so both exercise the same indexes: `xyz_cell` (the
/// spatial hash) and `xyz_level` (one bucket per z-level), both over `xyz`.
pub fn register_indexes(reg: &mut IndexRegistry) {
    reg.register::<Xyz, Cell>(CELL_INDEX, Policy::Multi, cell_of);
    reg.register::<Xyz, i64>(LEVEL_INDEX, Policy::Multi, |p| p.z);
}

/// The index maintainer, registered first in `Game.systems` so later systems in
/// the same tick read the updated index. First run builds and baselines the
/// registry; every later tick applies the tick's `ComponentChanged`/`Destroyed`
/// facts incrementally.
pub(crate) fn maintain(ctx: &mut SystemCtx) {
    musce::index::maintain(ctx.world, ctx.facts, register_indexes);
}

/// An entity's coordinates, if it has any.
pub fn coords(world: &World, entity: EntityId) -> Option<Xyz> {
    world.get::<Xyz>(entity).map(|c| *c)
}

/// Rooms whose cell falls within `radius` of `center`, in arbitrary order. A pure
/// index retrieve: it enumerates the cell keys the query region covers and unions
/// their buckets, reading no room's position. The region is quantized to the cell
/// grid (`CELL`), so results are a superset of an exact sphere near the boundary; a
/// caller that needs exact distance filters this batch itself (the index does
/// retrieval, not geometry). Empty if the index is not built yet (before the
/// maintainer's first run).
pub fn near(world: &World, center: &Xyz, radius: i64) -> Vec<EntityId> {
    let Some(reg) = world.resource::<IndexRegistry>() else {
        return Vec::new();
    };
    let Some(idx) = reg.index::<Cell>(CELL_INDEX) else {
        return Vec::new();
    };
    let (cx, cy, cz) = cell_of(center);
    let span = radius.div_euclid(CELL) + 1;
    let mut out = Vec::new();
    for dx in -span..=span {
        for dy in -span..=span {
            for dz in -span..=span {
                out.extend_from_slice(idx.get(&(cx + dx, cy + dy, cz + dz)));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use musce::world::hecs::EntityBuilder;

    fn place(world: &mut World, x: i64, y: i64, z: i64) -> EntityId {
        let mut b = EntityBuilder::new();
        b.add(Xyz { x, y, z });
        world.spawn(b)
    }

    fn build_index(world: &mut World) {
        let mut reg = IndexRegistry::default();
        register_indexes(&mut reg);
        reg.baseline(world);
        world.insert_resource(reg);
    }

    #[test]
    fn near_retrieves_the_covered_region() {
        let mut w = World::new();
        let here = place(&mut w, 0, 0, 0);
        let close = place(&mut w, 3, 0, 0);
        let far = place(&mut w, 100, 0, 0); // cell (10,0,0), outside the covered span
        build_index(&mut w);

        let hits = near(&w, &Xyz { x: 0, y: 0, z: 0 }, 10);
        assert!(hits.contains(&here) && hits.contains(&close));
        assert!(!hits.contains(&far));
    }

    #[test]
    fn near_spans_adjacent_cells() {
        // Two rooms straddling a cell boundary: the region unions both cells, not
        // just the center's.
        let mut w = World::new();
        let a = place(&mut w, CELL - 1, 0, 0); // cell 0
        let b = place(&mut w, CELL + 1, 0, 0); // cell 1
        build_index(&mut w);

        let hits = near(
            &w,
            &Xyz {
                x: CELL - 1,
                y: 0,
                z: 0,
            },
            5,
        );
        assert!(hits.contains(&a) && hits.contains(&b));
    }

    #[test]
    fn near_is_empty_before_the_index_exists() {
        let w = World::new();
        assert!(near(&w, &Xyz { x: 0, y: 0, z: 0 }, 10).is_empty());
    }
}
