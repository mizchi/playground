use std::collections::{HashMap, HashSet, VecDeque};
fn bfs(adj: &HashMap<u32, Vec<u32>>, start: u32) -> Vec<u32> {
    let mut seen = HashSet::new();
    let mut q = VecDeque::new();
    let mut order = vec![];
    q.push_back(start);
    seen.insert(start);
    while let Some(u) = q.pop_front() {
        order.push(u);
        if let Some(ns) = adj.get(&u) {
            for &v in ns { if seen.insert(v) { q.push_back(v); } }
        }
    }
    order
}
fn dfs(adj: &HashMap<u32, Vec<u32>>, u: u32, seen: &mut HashSet<u32>, out: &mut Vec<u32>) {
    if !seen.insert(u) { return; }
    out.push(u);
    for &v in adj.get(&u).map(|v| v.as_slice()).unwrap_or(&[]) { dfs(adj, v, seen, out); }
}
fn main() {
    let edges = [(1, 2), (1, 3), (2, 4), (3, 4), (4, 5), (5, 1)];
    let mut adj: HashMap<u32, Vec<u32>> = HashMap::new();
    for &(a, b) in edges.iter() { adj.entry(a).or_insert_with(Vec::new).push(b); }
    println!("{:?}", bfs(&adj, 1));
    let mut seen = HashSet::new(); let mut out = vec![];
    dfs(&adj, 1, &mut seen, &mut out);
    println!("{:?}", out);
    let grid = ["..#", ".#.", "..."];
    let g: Vec<Vec<bool>> = grid.iter().map(|r| r.chars().map(|c| c == '#').collect()).collect();
    let mut dist = vec![vec![usize::MAX; 3]; 3];
    let mut dq = VecDeque::from(vec![(0usize, 0usize)]);
    dist[0][0] = 0;
    while let Some((r, c)) = dq.pop_front() {
        for (dr, dc) in [(0i32, 1i32), (1, 0), (0, -1), (-1, 0)] {
            let (nr, nc) = (r as i32 + dr, c as i32 + dc);
            if nr < 0 || nc < 0 || nr >= 3 || nc >= 3 { continue; }
            let (nr, nc) = (nr as usize, nc as usize);
            if g[nr][nc] || dist[nr][nc] != usize::MAX { continue; }
            dist[nr][nc] = dist[r][c] + 1;
            dq.push_back((nr, nc));
        }
    }
    println!("{}", dist[2][2]);
}
