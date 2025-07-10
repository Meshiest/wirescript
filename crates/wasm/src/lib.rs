use builder::options::{LayoutMode, LayoutOptions};
use serde::Deserialize;
use wasm_bindgen::prelude::*;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    pub inline: bool,
    #[serde(default)]
    pub layout: LayoutMode,
    #[serde(default)]
    pub layout_options: LayoutOptions,
    #[serde(default)]
    pub grid_options: String,
}

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

// TODO: function for converting bricks to rectangles for previews

#[wasm_bindgen]
pub fn layout(
    source: String,
    module: String,
    inline: bool,
    options: JsValue,
) -> Result<String, JsValue> {
    // let options = serde_wasm_bindgen::from_value(options)?;

    Ok("".to_string())
}
