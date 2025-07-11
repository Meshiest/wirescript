use std::num::NonZeroU8;

use builder::options::{GridOptions, LayoutOptions};
use js_sys::Reflect;
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
pub fn get_modules(source: String) -> Result<Vec<String>, String> {
    Ok(bearilog::parse_modules(&source)?
        .into_iter()
        .map(|m| m.name)
        .collect())
}

#[wasm_bindgen]
pub fn graphviz(source: String, module: String, inline: bool) -> Result<String, String> {
    let res = match bearilog::parse_and_compile(&source, &module, inline) {
        Ok(res) => res,
        Err(e) => return Err(e.to_string()),
    };

    bearilog::graphviz::render(&res).map_err(|e| e.to_string())
}

#[wasm_bindgen]
pub fn layout(source: String, module: String, options: JsValue) -> Result<Vec<u8>, JsValue> {
    if !options.is_object() {
        return Err(JsError::new("Options must be an object").into());
    }

    let get_u8 = |key: &str| -> u8 {
        Reflect::get(&options, &key.into())
            .ok()
            .and_then(|v| v.as_f64().map(|f| f as u8))
            .unwrap_or(0)
    };

    let inline = Reflect::get(&options, &"inline".into())
        .ok()
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let layout_options = LayoutOptions {
        gap_v: get_u8("gapV"),
        gap_h: get_u8("gapH"),
        margin: get_u8("margin"),
        padding: get_u8("padding"),
        indent: get_u8("indent"),
        flat: Reflect::get(&options, &"flat".into())
            .ok()
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
    };

    let res = match bearilog::parse_and_compile(&source, &module, inline) {
        Ok(res) => res,
        Err(e) => return Err(e.to_string().into()),
    };

    let world = builder::layout_module_to_world(res, layout_options).map_err(|e| e.to_string())?;

    Ok(world.to_brz_vec().map_err(|e| e.to_string())?)
}

#[wasm_bindgen]
pub fn grid(source: String, module: String, options: JsValue) -> Result<Vec<u8>, JsValue> {
    if !options.is_object() {
        return Err(JsError::new("Options must be an object").into());
    }

    let get_u8 = |key: &str| -> u8 {
        Reflect::get(&options, &key.into())
            .ok()
            .and_then(|v| v.as_f64().map(|f| f as u8))
            .unwrap_or(0)
    };

    let inline = Reflect::get(&options, &"inline".into())
        .ok()
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let grid_options = GridOptions {
        height: NonZeroU8::new(get_u8("height")).unwrap_or(NonZeroU8::new(8).unwrap()),
        width: NonZeroU8::new(get_u8("width")).unwrap_or(NonZeroU8::new(8).unwrap()),
        layers: Reflect::get(&options, &"layers".into())
            .ok()
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        iobelow: Reflect::get(&options, &"iobelow".into())
            .ok()
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
    };

    let res = match bearilog::parse_and_compile(&source, &module, inline) {
        Ok(res) => res,
        Err(e) => return Err(e.to_string().into()),
    };

    let world = builder::build_grid(res, grid_options);

    Ok(world.to_brz_vec().map_err(|e| e.to_string())?)
}
