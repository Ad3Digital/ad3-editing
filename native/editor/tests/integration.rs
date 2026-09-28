// Keep one integration executable: linking the same VM/GUI dependencies into
// separate test executables is unnecessarily expensive on Windows.
#[path = "compositor.rs"]
mod compositor;
#[path = "editing.rs"]
mod editing;
#[path = "effects.rs"]
mod effects;
#[path = "fonts.rs"]
mod fonts;
#[path = "geometry.rs"]
mod geometry;
#[path = "inspect.rs"]
mod inspect;
#[path = "masks.rs"]
mod masks;
#[path = "modules.rs"]
mod modules;
#[path = "motion.rs"]
mod motion;
#[path = "paints.rs"]
mod paints;
#[path = "reactivity.rs"]
mod reactivity;
#[path = "rendering.rs"]
mod rendering;
#[path = "text.rs"]
mod text;
#[path = "textures.rs"]
mod textures;
