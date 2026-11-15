// (未対応: `-> impl Iterator` の戻り値)
fn evens(n: u32) -> impl Iterator<Item = u32> { (0..n).filter(|x| x % 2 == 0) }
struct Grid { w: usize, h: usize }
impl Grid { fn cells(&self) -> impl Iterator<Item = (usize, usize)> + '_ { (0..self.h).flat_map(move |y| (0..self.w).map(move |x| (x, y))) } }
fn main() {
    let g = Grid { w: 2, h: 2 };
    println!("{:?}", g.cells().collect::<Vec<_>>());
    println!("{:?}", evens(10).collect::<Vec<_>>());
}
