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

use crate::data::*;

use std::sync::atomic::Ordering;

#[derive(Debug, Clone, Copy, Default)]
pub struct TreeMapNode {
    pub min_x : f32,
    pub min_y : f32,
    pub max_x : f32,
    pub max_y : f32,
}

pub struct TreeMap {
    pub nodes : Vec<TreeMapNode>,
    pub leaf : Vec<bool>,
}

impl TreeMap {
    pub fn new() -> TreeMap {
        TreeMap {
            nodes : vec![],
            leaf : vec![],
        }
    }

    pub fn rebuild_tree_map(&mut self, region : (f32, f32), entries : &[Entry], progress: &ProgressState) {
        self.nodes.clear();
        
        let nlen = entries.len();
        let olen1 = self.nodes.len();
        let olen2 = self.leaf.len();
        if nlen > olen1 {
            self.nodes.reserve(nlen - olen1);
        }

        if nlen > olen2 {
            self.leaf.reserve(nlen - olen2);
        }

        self.nodes.clear();
        self.nodes.extend(std::iter::repeat(TreeMapNode::default()).take(nlen));

        self.nodes.clear();
        self.nodes.extend(std::iter::repeat(TreeMapNode::default()).take(nlen));

        self.nodes[0].max_x = region.0;
        self.nodes[0].max_y = region.1;

        progress.count.store(nlen, Ordering::SeqCst);

        r_help(0, entries, &mut self.nodes, &mut self.leaf, progress);
        fn r_help(i : usize, entries : &[Entry], nodes : &mut [TreeMapNode], leaf : &mut [bool], progress: &ProgressState) {
            let mut c = entries[i].children.clone();
            leaf[i] = entries[i].is_file;

            c.sort_by_key(|&j| entries[j].size);

            let cnode = nodes[i];

            let w = cnode.max_x - cnode.min_x;
            let h = cnode.max_y - cnode.min_y;

            progress.done.fetch_add(1, Ordering::SeqCst);

            let size = entries[i].size as f32;

            if w > h {
                let mut x = cnode.min_x;
                for j in c {
                    let r = w * entries[j].size as f32 / size;
                    nodes[j] = TreeMapNode{min_x : x, max_x : x + r, ..cnode};
                    x += r;
                    r_help(j, entries, nodes, leaf, progress);
                }
            } else {
                let mut y = cnode.min_y;
                for j in c {
                    let r = h * entries[j].size as f32 / size;
                    nodes[j] = TreeMapNode{min_y : y, max_y : y + r, ..cnode};
                    y += r;
                    r_help(j, entries, nodes, leaf, progress);
                }
            }

            progress.done.fetch_add(1, Ordering::SeqCst);
        }
    }

    pub fn redraw_tree_map(&self, w : usize, h : usize, buf : &mut [u8]) {
        let TreeMapNode {min_x, min_y, max_x, max_y} = self.nodes[0];

        let dx = max_x - min_x;
        let dy = max_y - min_y;

        fn fill_pixels(col : [u8; 4], x0 : usize, y0 : usize, x1 : usize, y1 : usize, w : usize, h : usize, pix : &mut [u8]) {
            for y in y0..y1 {
                for x in x0..x1 {
                    let i = 4 * (x + w * y);

                    pix[i+0] = col[0];
                    pix[i+1] = col[1];
                    pix[i+2] = col[2];
                    pix[i+3] = col[3];
                }
            }
        }

        let col = [0; 4];

        for (&TreeMapNode {min_x: cmin_x, min_y: cmin_y, max_x: cmax_x, max_y: cmax_y}, &l) in self.nodes.iter().zip(self.leaf.iter()) {
            if l {
                let x0 = ((cmin_x - min_x) / dx).floor() as usize;
                let x1 = ((cmax_x - min_x) / dx).floor() as usize;
                let y0 = ((cmin_y - min_y) / dy).floor() as usize;
                let y1 = ((cmax_y - min_y) / dy).floor() as usize;

                fill_pixels(col, x0, y0, x1, y1, w, h, buf)
            }
        }
    }
}