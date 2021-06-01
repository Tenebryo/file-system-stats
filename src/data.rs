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


use std::{path::PathBuf, sync::{Arc, atomic::AtomicBool}};

use std::time::Duration;

use std::sync::atomic::{AtomicUsize, Ordering};

use tokio::fs::*;
use tokio::sync::Mutex;

use async_channel::*;
use tokio::sync::broadcast::channel;

#[derive(Debug, Clone)]
pub struct Entry {
    pub parent : usize,
    pub size : u64,
    pub path : PathBuf,
    pub children : Vec<usize>,
    pub num_files : u64,
    pub is_file : bool,
    pub expanded : bool,
}

#[derive(Debug)]
pub struct ProgressState {
    pub count : AtomicUsize,
    pub done  : AtomicUsize,
    pub complete : AtomicBool,
}

impl ProgressState {
    pub fn new() -> Self {
        Self {
            count : AtomicUsize::new(0),
            done  : AtomicUsize::new(0),
            complete : AtomicBool::new(false),
        }
    }
}

#[derive(Debug, Clone)]
pub struct DirectoryTreeData {
    pub entries : Arc<Mutex<Vec<Entry>>>,
    pub file_types : Arc<Mutex<std::collections::HashMap<Option<String>, u64>>>,
}

impl DirectoryTreeData {
    pub fn new() -> DirectoryTreeData {
        DirectoryTreeData {
            entries : empty_entry_list(),
            file_types : Arc::new(Mutex::new(std::collections::HashMap::new())),
        }
    }

    pub async fn from_root_path(path : PathBuf) -> DirectoryTreeData {
        
        DirectoryTreeData {
            entries : directory_entry_list(path).await,
            file_types : Arc::new(Mutex::new(std::collections::HashMap::new())),
        }
    }

    pub async fn recreate_from_root_path(&self, path : PathBuf) {
        recreate_directory_entry_list(path, self.entries.clone()).await;
        self.file_types.lock().await.clear();
    }
}

const NTASKS : usize = 8;

pub fn empty_entry_list() -> Arc<Mutex<Vec<Entry>>> {
    Arc::new(Mutex::new(vec![]))
}

pub async fn directory_entry_list(path : PathBuf) -> Arc<Mutex<Vec<Entry>>> {

    let meta = metadata(path.clone()).await.unwrap();

    let is_file = meta.is_file();

    Arc::new(Mutex::new(vec![Entry{
        parent : usize::MAX,
        size : if is_file {meta.len()} else {0},
        num_files : if is_file {1} else {0},
        children : vec![],
        path,
        is_file,
        expanded : true,
    }]))
}

pub async fn recreate_directory_entry_list(path : PathBuf, entries : Arc<Mutex<Vec<Entry>>>) {

    let meta = metadata(path.clone()).await.unwrap();

    let is_file = meta.is_file();

    let mut lock = entries.lock().await;
    lock.clear();
    lock.push(Entry{
        parent : usize::MAX,
        size : if is_file {meta.len()} else {0},
        num_files : if is_file {1} else {0},
        children : vec![],
        path,
        is_file,
        expanded : true,
    });
}

pub async fn scan_directory(root : usize, DirectoryTreeData{entries, file_types} : DirectoryTreeData, progress : Arc<ProgressState>) {

    let (tx, rx) : (Sender<Entry>, Receiver<Entry>)= unbounded();

    let (done_tx, _) = channel(1);

    let waiting = Arc::new(AtomicUsize::new(0));

    let mut handles = vec![];

    for _ in 0..NTASKS {
        let rx = rx.clone();
        let tx = tx.clone();
        let mut done_rx = done_tx.subscribe();
        let paths = entries.clone();
        let progress = progress.clone();
        let waiting = waiting.clone();
        let file_types = file_types.clone();
        let h = tokio::task::spawn(async move {
            done_rx.recv().await.unwrap();
            loop {
                waiting.fetch_add(1, Ordering::SeqCst);
                tokio::select! {
                    _ = done_rx.recv() => {
                        break;
                    }
                    Ok(entry) = rx.recv() => {
                        waiting.fetch_sub(1, Ordering::SeqCst);

                        let i = {
                            let mut locked = paths.lock().await;
                            locked.push(entry.clone());
                            locked.len() - 1
                        };
                        
                        if entry.is_file {

                            let mut locked = paths.lock().await;

                            let mut j = locked[i].parent;
                            let n = locked.len();

                            let file_type = entry.path.extension().map(|ext| String::from(ext.to_string_lossy()).to_uppercase());

                            file_types.lock().await.entry(file_type)
                                .and_modify(|v| { *v += entry.size; })
                                .or_insert(entry.size);

                            locked[j].children.push(i);

                            while j < n {
                                locked[j].size += entry.size;
                                locked[j].num_files += 1;

                                j = locked[j].parent;
                            }
                        } else {
                            {
                                let mut locked = paths.lock().await;
                                let j = locked[i].parent;
                                locked[j].children.push(i);
                            }
                            if let Err(e) = process_directory(i, entry.path.clone(), &tx, progress.clone()).await {
                                eprintln!("{:?}: {:?}", e, entry.path);
                            }
                        }
                        
                        progress.done.fetch_add(1, Ordering::SeqCst);
                    }
                }

            }
        });
        handles.push(h);
    }

    {
        let locked = entries.lock().await;

        let root_path = locked[root].path.clone();

        if let Err(e) = process_directory(root, root_path, &tx, progress.clone()).await {
            eprintln!("{:?}", e);
        }


        done_tx.send(()).unwrap();
    }

    let mut clock = tokio::time::interval(Duration::from_millis(10));

    loop {

        clock.tick().await;

        if rx.is_empty() && waiting.load(Ordering::SeqCst) as usize == NTASKS {
            break;
        }
    }

    done_tx.send(()).unwrap();

    for handle in &mut handles {
        handle.await.unwrap();
    }

    progress.complete.store(true, Ordering::SeqCst);
}


async fn process_directory(i : usize, path : PathBuf, queue : &Sender<Entry>, progress : Arc<ProgressState>) -> std::io::Result<()> {
    
    let mut dir = read_dir(path).await?;

    loop {
        match dir.next_entry().await {
            Ok(Some(subdir)) => {
                let meta = subdir.metadata().await;

                if meta.is_err() {
                    continue;
                }

                let meta = meta.unwrap();

                let size = meta.len();
        
                progress.count.fetch_add(1, Ordering::SeqCst);
        
                if meta.is_dir() {
                    queue.send(Entry{
                        parent : i,
                        size,
                        num_files : 0,
                        path : subdir.path().into(),
                        children : vec![],
                        is_file : false,
                        expanded : false,
                    }).await.unwrap();
                } else if meta.is_file() {

                    queue.send(Entry{
                        parent : i,
                        size,
                        num_files : 1,
                        path : subdir.path().into(),
                        children : vec![],
                        is_file : true,
                        expanded : false,
                    }).await.unwrap();
                }
            }
            Err(_) => {}
            _ => break
        }
    }

    Ok(())
}


pub fn recalculate_data(entries : &mut [Entry]) {

    // clear directory sizes, file counts, and children
    for e in entries.iter_mut() {
        if e.is_file {
            e.size = 0;
            e.num_files = 0;
        }
        e.children.clear();
    }

    for i in 1..(entries.len()) {
        let p = entries[i].parent;
        entries[p].children.push(i);
    }

    fn r_help(entries : &mut [Entry], i : usize) -> (u64, u64) {

        let mut size = 0;
        let mut files = 0;

        for c in entries[i].children.clone() {
            let (s, f) = r_help(entries, c);
            size += s;
            files += f;
        }

        entries[i].size = size;
        entries[i].num_files = files;

        (size, files)
    }

    r_help(entries, 0);
}

pub fn delete_subtree(entries : &mut Vec<Entry>, subtree : usize) {
    let mut new_idx = entries.iter().map(|_| usize::MAX).collect::<Vec<_>>();
    let mut next_i = 0;
    for i in 0..(entries.len()) {
        let parent = entries[i].parent;

        if parent == subtree || i == subtree {
            entries[i].parent = subtree;
        } else {
            new_idx[i] = next_i;
            next_i += 1;
        }
    }

    for (i, ni) in new_idx.iter().cloned().enumerate() {
        if i == ni || ni == usize::MAX {
            continue;
        }

        entries.swap(i, ni);
    }

    entries.truncate(next_i);

    recalculate_data(entries)
}

pub fn sort_subtree_by_size(i : usize, entries : &mut [Entry]) {

    for c in entries[i].children.clone() {
        sort_subtree_by_size(c, entries);
    }

    let mut j = i;

    while entries[j].parent < entries.len() {
        let mut children = entries[j].children.clone();
        children.sort_by_cached_key(|&e| entries[e].size);
        entries[j].children = children;

        j = entries[j].parent;
    }
}

pub fn find_path_index(path : PathBuf, entries : &[Entry]) -> Option<usize> {
    let mut i = 0;

    'outer: loop {
        for &c in entries[i].children.iter() {
            let p = &entries[c].path;

            if path == *p {
                return Some(c);
            }

            if path.starts_with(p) {
                i = c;
                continue 'outer;
            }
        }
        break
    }

    None
}

#[test]
fn test_perf() {

    use std::time::Instant;

    let root = PathBuf::from("C:\\");

    let progress = Arc::new(ProgressState::new());

    let start = Instant::now();
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let entries = directory_entry_list(root).await;
            let file_types = Arc::new(Mutex::new(std::collections::HashMap::new()));
            scan_directory(0, DirectoryTreeData{entries, file_types}, progress).await
        });

    let elapsed = start.elapsed();


    eprintln!("Elapsed: {:?}", elapsed);


}