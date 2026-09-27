// Keep one integration executable: linking the same VM/GUI dependencies into
// separate test executables is unnecessarily expensive on Windows.
#[path = "editing.rs"]
mod editing;
#[path = "inspect.rs"]
mod inspect;
#[path = "modules.rs"]
mod modules;
#[path = "reactivity.rs"]
mod reactivity;
#[path = "rendering.rs"]
mod rendering;
