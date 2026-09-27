// Keep one integration executable: linking the same VM/GUI dependencies into
// separate test executables is unnecessarily expensive on Windows.
#[path = "compositor.rs"]
mod compositor;
#[path = "editing.rs"]
mod editing;
#[path = "geometry.rs"]
mod geometry;
#[path = "inspect.rs"]
mod inspect;
#[path = "modules.rs"]
mod modules;
#[path = "motion.rs"]
mod motion;
#[path = "reactivity.rs"]
mod reactivity;
#[path = "rendering.rs"]
mod rendering;
#[path = "textures.rs"]
mod textures;
