// Skjul konsolvinduet i release-builds på Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    cellkeeper_lib::run()
}
