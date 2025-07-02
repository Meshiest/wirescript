use crate::{
    bearilog::compiler::CompiledModule,
    brdb::{Position, World},
    builder::{
        errors::BuilderError, helpers::build_dst_to_src, layout::LayoutBuilderContext,
        options::LayoutOptions,
    },
};

mod errors;
mod grid;
mod helpers;
mod layout;
pub mod options;

pub fn layout_module_to_world(
    module: CompiledModule,
    options: LayoutOptions,
) -> Result<World, BuilderError> {
    let mut world = World::new();

    let map = build_dst_to_src(&module);
    let mut ctx = LayoutBuilderContext::new(&map, options);
    ctx.build(module, Position::ZERO);

    world.add_bricks(ctx.bricks);
    world.add_wires(ctx.wires);

    Ok(world)
}

pub use grid::build_grid;

#[cfg(test)]
mod tests {
    use std::error::Error;

    use crate::{
        bearilog::parse_and_compile,
        brdb::Brdb,
        builder::{LayoutOptions, layout_module_to_world},
    };

    #[test]
    fn test() -> Result<(), Box<dyn Error>> {
        let source = "
        inline module add(a, b) -> c {
            c = a + b;
        }
        module foo(a, b, c, d) -> o {
            o = add(add(a, b), add(c, d)) + 2;
        }
        ";

        let options = LayoutOptions::default();
        let world = layout_module_to_world(parse_and_compile(source, "foo", false)?, options)?;
        Brdb::new_memory()?.save("create", &world)?;

        Ok(())
    }
}
