// Without this, a release build pops a console window next to the main window:
// Rust links against the console subsystem by default, so Windows allocates one.
// That console also hosts the process, so closing it kills the app. Kept in debug
// builds because `tauri dev` still needs println/panic output.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    dreampaper_lib::run();
}
