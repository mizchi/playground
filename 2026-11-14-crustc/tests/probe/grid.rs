fn main() {
    let mut g = vec![vec![0i32; 5]; 4];
    for r in 0..4 { for c in 0..5 { g[r][c] = (r * 5 + c) as i32; } }
    let t: Vec<Vec<i32>> = (0..5).map(|c| g.iter().map(|row| row[c]).collect()).collect();
    println!("{:?}", t[2]);
    let mut a = [[0u8; 3]; 3];
    a[1][2] = 7;
    println!("{:?}", a);
    let dirs = [(0i32, 1i32), (1, 0), (0, -1), (-1, 0)];
    let mut cnt = 0;
    for (dr, dc) in dirs.iter() {
        let (nr, nc) = (1 + dr, 1 + dc);
        if nr >= 0 && nc >= 0 && (nr as usize) < 4 && (nc as usize) < 5 { cnt += g[nr as usize][nc as usize]; }
    }
    println!("{}", cnt);
    let s: i32 = g.clone().into_iter().flatten().sum();
    println!("{}", s);
    let lines = "#.#\n.#.\n#..";
    let m: Vec<Vec<char>> = lines.lines().map(|l| l.chars().collect()).collect();
    println!("{}", m.concat().iter().filter(|&&c| c == '#').count());
}
