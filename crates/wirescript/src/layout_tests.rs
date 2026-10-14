//! Layout tests that lower real Wirescript source. Layout lives in
//! `wirescript-ir`, which has no front end, so these run here and reach the
//! internals they check through `layout::internals`.

mod code;
mod region;
mod wall;
