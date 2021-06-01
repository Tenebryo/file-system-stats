/*

   Copyright 2021 Sam Blazes

   Licensed under the Apache License, Version 2.0 (the "License");
   you may not use this file except in compliance with the License.
   You may obtain a copy of the License at

       http://www.apache.org/licenses/LICENSE-2.0

   Unless required by applicable law or agreed to in writing, software
   distributed under the License is distributed on an "AS IS" BASIS,
   WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
   See the License for the specific language governing permissions and
   limitations under the License.

*/

#![feature(drain_filter)]
#![allow(dead_code, unused_macros)]

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Instant;

mod data;
mod renderer;
mod treemap;
use data::*;
use tokio::task::JoinHandle;
use treemap::TreeMap;

use std::sync::atomic::AtomicBool;

use imgui::*;

use clipboard::ClipboardProvider;
use clipboard::ClipboardContext;

#[derive(Debug)]
struct Task {
    name : String,
    id : u64,
    start : Instant,
    end : Option<Instant>,
    progress : Arc<ProgressState>,
    handle : JoinHandle<()>,
}

struct AppState {
    async_runtime : tokio::runtime::Runtime,
    entry_list    : Arc<tokio::sync::Mutex<Vec<Entry>>>,
    task_handles  : Vec<tokio::task::JoinHandle<()>>,
}

macro_rules! cloned_scope {
    ($(name:ident)*, $block:stmt) => {
        {
            $(let $name = $name.clone();)*

            $block
        }
    };
}

macro_rules! build_layout {
    ($name:ident [$width:expr, $height:expr] {$($args:tt)*}) => {
        let mut $name = stretch::node::Stretch::new();
        build_layout!(($name) $($args)*);
        let $name = $name.compute_layout();
    };
    (($name:ident) $($rest:tt)*) => {

        build_layout!(($name) $($rest)*)
    }
}

fn main() {
    let mut _clipboard_ctx: ClipboardContext = ClipboardProvider::new().unwrap();

    let filesystem_data = DirectoryTreeData::new();

    let async_runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();

    let system = renderer::init("File System Stats");

    let root_path = Arc::new(std::sync::Mutex::new(ImString::new(String::new())));

    let choosing_path = Arc::new(AtomicBool::new(false));

    let tree_map : Arc<tokio::sync::Mutex<Option<TreeMap>>> = Arc::new(tokio::sync::Mutex::new(None));

    let mut tasks = vec![];

    let mut gen_id = 0..;

    system.main_loop(|_, ui, win| {

        let w = win.inner_size().width;
        let h = win.inner_size().height;

        let root_window_height = 64.0;
        let tree_map_height = 256.0;

        Window::new(im_str!("Root Selection"))
            .size([w as f32, root_window_height], Condition::Always)
            .position([0.0, 0.0], Condition::Always)
            .collapsible(false)
            .resizable(false)
            .no_nav()
            .build(ui, || {

                ui.input_text(im_str!(""), &mut *root_path.lock().unwrap())
                    .build();

                ui.same_line(ui.window_content_region_width() - 2.0*72.0);

                if ui.button(im_str!("Browse"), [64.0, 24.0]) && !choosing_path.load(Ordering::SeqCst) {
                    choosing_path.store(true, Ordering::SeqCst);

                    let choosing_path = choosing_path.clone();
                    let root_path = root_path.clone();
                    async_runtime.spawn_blocking(move || {

                        match nfd::open_pick_folder(None) {
                            Ok(nfd::Response::Okay(file_path))     => {
                                println!("File path = {:?}", file_path);
                                *root_path.lock().unwrap() = file_path.into();
                            },
                            Ok(nfd::Response::Cancel)              => println!("User canceled"),
                            _ => {}
                        }

                        choosing_path.store(false, Ordering::SeqCst);
                    });
                }
                
                ui.same_line(ui.window_content_region_width() - 72.0);

                if ui.button(im_str!("Scan"), [64.0, 24.0]) {

                    let id = gen_id.next().unwrap();

                    let progress = Arc::new(ProgressState::new());

                    let root_path = root_path.lock().unwrap().to_string();

                    let start = Instant::now();
                    // tx.send(AsyncMessage::Scan(id, root_path.clone(), progress.clone())).unwrap();

                    println!("start scan of {:?}", root_path);
                    
                    let filesystem_data = filesystem_data.clone();
                    let path = root_path.clone();
                    let pg = progress.clone();

                    let h = async_runtime.spawn(async move {
                        filesystem_data.recreate_from_root_path(path.into()).await;

                        scan_directory(0, filesystem_data, pg).await;
                    });
                    
                    tasks.push( Task {
                        id,
                        name : format!("Scan [{}]", root_path),
                        start,
                        end : None,
                        progress,
                        handle : h,
                    });
                }
            });

        // this window shows the directory tree
        Window::new(im_str!("Directory Tree"))
            .size([w as f32 - 256.0, h as f32 - root_window_height - tree_map_height], Condition::Always)
            .position([0.0, root_window_height], Condition::Always)
            .collapsible(false)
            .resizable(false)
            .no_nav()
            .build(ui, || {

                let mut display_entries = vec![];
                {
                    let elock = async_runtime.block_on(filesystem_data.entries.lock());

                    if !elock.is_empty() {

                        let mut i = 0;
                        display_entries.push(elock[0].clone());
                        display_entries[0].parent = 0;

                        while i < display_entries.len() {
                            if display_entries[i].expanded {

                                let n = display_entries[i].children.len();

                                display_entries[i].children.sort_by_cached_key(|&c| -(elock[c].size as i64));

                                for j in 0..n {
                                    let k = display_entries[i].children[j];
                                    display_entries.push(elock[k].clone());
                                    display_entries.last_mut().unwrap().parent = k;
                                    display_entries[i].children[j] = display_entries.len() - 1;
                                }
                            } else {

                            }

                            i += 1;
                        }
                    }
                }

                if !display_entries.is_empty() {

                    display_file_tree(&mut display_entries, ui);

                    let mut elock = async_runtime.block_on(filesystem_data.entries.lock());
            
                    for e in display_entries.iter_mut() {
                        if e.parent < elock.len() {
                            elock[e.parent].expanded = e.expanded;
                        }
                    }
                }

                if ui.button(im_str!("Clear"), [64.0, 16.0]) {
                    let mut elock = async_runtime.block_on(filesystem_data.entries.lock());
                    elock.clear();
                }
            });
        // this window shows the progress of tasks such as the 
        Window::new(im_str!("File Type Statistics"))
            .size([256.0, h as f32 - root_window_height - 128.0], Condition::Always)
            .position([w as f32 - 256.0, root_window_height], Condition::Always)
            .collapsible(false)
            .resizable(false)
            .no_nav()
            .build(ui, || {
                
                ui.text("Extension");
                ui.same_line(ui.window_content_region_width() - 96.0);
                ui.text("Total Space");
                ui.separator();

                let mut data = {
                    let ft_lock = async_runtime.block_on(filesystem_data.file_types.lock());
                 
                    ft_lock.iter().map(|(k,v)| (k.clone(), v.clone())).collect::<Vec<_>>()
                };

                data.sort_by_key(|(_,v)| u64::MAX - v);

                for (k, v) in data.into_iter() {

                    if let Some(ext) = k {
                        ui.text(ext);
                    } else {
                        ui.text("<none>");
                    }
                    
                    ui.same_line(ui.window_content_region_width() - 96.0);
                    ui.text(format!("{}", ByteSize(v)));

                }
                
            });

        
        // this window shows the progress of tasks such as the 
        Window::new(im_str!("Tasks"))
            .size([256.0, 128.0], Condition::Always)
            .position([w as f32 - 256.0, h as f32 - 128.0], Condition::Always)
            .collapsible(false)
            .resizable(false)
            .no_nav()
            .build(ui, || {

                for Task{progress: p, start : s, name : n, handle, end, ..} in tasks.iter_mut() {

                    if p.complete.load(Ordering::SeqCst) {
                        if end.is_none() {
                            *end = Some(Instant::now());
                        }
                        continue;
                    }

                    let count = p.count.load(Ordering::SeqCst) as f32;
                    let done = p.done.load(Ordering::SeqCst) as f32;
                    let frac = done/count;
                    let time = s.elapsed().as_secs_f32();
                    let rem = time * (1.0 / frac - 1.0);

                    ui.text(n);
                    ui.same_line(ui.window_content_region_width() - 20.0);

                    if ui.button(im_str!("X"), [16.0, 16.0]) {
                        p.complete.store(true, Ordering::SeqCst);
                        // tx.send(AsyncMessage::AbortTask(*id)).unwrap();
                        handle.abort();
                    }

                    ui.text(format!("{} / {}", done, count));

                    let overlay = ImString::from(format!("{:.1}s", rem));
                    ProgressBar::new(frac)
                        .overlay_text(&overlay)
                        .build(&ui);
                }

                ui.separator();

                for Task{progress: p, name : n, start, end, ..} in tasks.iter().rev() {

                    if !p.complete.load(Ordering::SeqCst) {
                        continue;
                    }

                    
                    ui.text(format!("{} - Complete", n));
                    if let Some(end) = *end {
                        ui.text(format!("  {:4.2}s", end.duration_since(*start).as_secs_f32()));
                    }
                }
            });

            
        // this window shows the treemap of file sizes
        Window::new(im_str!("Treemap"))
            .size([w as f32 - 256.0, tree_map_height], Condition::Always)
            .position([0.0, h as f32 - tree_map_height], Condition::Always)
            .collapsible(false)
            .resizable(false)
            .no_nav()
            .build(ui, || {
                let tree_map_built = async_runtime.block_on(tree_map.lock()).is_none();

                let w_min = ui.window_content_region_min();
                let w_max = ui.window_content_region_max();

                if tree_map_built {
                    if ui.button(im_str!("Create Treemap"), [128.0, 24.0]) {
                        let tree_map = tree_map.clone();
                        let entries = filesystem_data.entries.clone();
                        let start = Instant::now();
                        let id = gen_id.next().unwrap();
                        let progress = Arc::new(ProgressState::new());
                        let pg = progress.clone();

                        let jh = async_runtime.spawn(async move {
                            let mut treemap = TreeMap::new();

                            treemap.rebuild_tree_map((w_max[0] - w_min[0], w_max[1] - w_min[1]), &*entries.lock().await, pg.as_ref());
                            
                            *tree_map.lock().await = Some(treemap);

                            pg.complete.store(true, Ordering::SeqCst);
                        });

                        
                        tasks.push( Task {
                            id,
                            name : format!("Treemap Layout"),
                            start,
                            end : None,
                            progress,
                            handle : jh,
                        });
                    }
                } else {
                }
            });
    });

    println!("Exiting...");

    for Task{handle,..} in tasks {
        handle.abort();
    }
}

/// Build ui elements
fn display_file_tree(display_entries : &mut Vec<Entry>, ui : &Ui) {
    
    let cw = ui.window_content_region_width();

    #[derive(Debug, Clone, Copy, Default)]
    struct DrawParams {
        psize : u64,
        content_width : f32,
    }

    let size = display_entries[0].size;
    r_help(0, display_entries, ui, DrawParams{psize : size, content_width: cw});
    fn r_help(i : usize, entries : &mut [Entry], ui : &Ui, dp : DrawParams) {

        let name = ImString::from(
            format!("{:80}",
            if i == 0 {
                entries[i].path.to_str().unwrap()
            } else {
                entries[i].path.components().last().unwrap().as_os_str().to_str().unwrap()
            }
        ));
        let mut expanded = false;

        let progress_bar_start = dp.content_width - 96.0;
        let file_size_location = progress_bar_start - 96.0;

        macro_rules! entry_display {
            ($size:expr, $prog:expr, $pbs:expr, $fsl:expr) => {
                ui.same_line($fsl);
                ui.text(format!("{: >9}", format!("{}", ByteSize($size))));
                ui.same_line($pbs);
                ProgressBar::new($prog).build(ui);
            }
        }

        let size = entries[i].size;

        TreeNode::new(&name)
            .allow_item_overlap(true)
            .framed(true)
            .leaf(entries[i].is_file)
            .opened(entries[i].expanded, Condition::Once)
            .build(ui, || {

                entry_display!(size, size as f32 / dp.psize as f32, progress_bar_start, file_size_location);

                if entries[i].expanded {
                    for &c in entries[i].children.clone().iter() {
                        // if entries[c].is_file {
                        //     ui.text(entries[c].path.file_name().unwrap().to_str().unwrap());
                        //     entry_display!(entries[c].size, entries[c].size as f32 / size as f32);
                        // } else {
                            r_help(c, entries, ui, DrawParams{psize : size, ..dp});
                        // }
                    }
                }
                expanded = true;
            });
        
        if !expanded {
            entry_display!(size, size as f32 / dp.psize as f32, progress_bar_start, file_size_location);
        }
            
        entries[i].expanded = expanded;
    }



}

struct ByteSize(u64);

impl std::fmt::Display for ByteSize {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        const SUFFIXES : [&str; 6] = ["B", "KB", "MB", "GB", "TB", "PB"];

        let mut size = self.0 as f32;
        let mut i = 0;
        for _ in SUFFIXES.iter() {
            if size > 1000.0 {
                size /= 1000.0;
                i += 1;
            }
        }

        write!(f, "{:.1}{}", size, SUFFIXES[i])
    }
}