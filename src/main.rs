//! The Whisker executable: a window program whose `main` is the library's
//! `run`.

#![windows_subsystem = "windows"]

fn main() {
    whisker::run();
}
