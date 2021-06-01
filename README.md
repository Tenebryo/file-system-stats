# File System Stats

File System Stats is a simple tool to diagnose where large amounts of storage
are being used, inspired by WinDirStat. The goal of this project was to make
a faster version of WinDirStat with perhaps not all the features. The current
version scans 350k files on my 1TB SSD in under 45 seconds.

## Installation

 * Install nightly Rust via [rustup.rs](https://rustup.rs/)
 * Build with the command `cargo +nightly build --release`
 * Executable will be in the `target/release/` directory

## Implemented Features

 - [x] Directory walking and scanning
 - [x] Simple UI with Dear IMGUI
 - [x] Directory tree orderered by size of contents
 - [x] Total file size by file extension
 - [x] Directory scan progress
 - [ ] File size treemap (planned but not complete)
