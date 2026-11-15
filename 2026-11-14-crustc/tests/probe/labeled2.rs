fn find(grid: &[[i32; 3]; 3], t: i32) -> Option<(usize, usize)> {
    let mut r = None;
    'rows: for (i, row) in grid.iter().enumerate() {
        for (j, &v) in row.iter().enumerate() {
            if v == t {
                r = Some((i, j));
                break 'rows;
            }
        }
    }
    r
}

fn main() {
    let g = [[1, 2, 3], [4, 5, 6], [7, 8, 9]];
    println!("{:?} {:?}", find(&g, 6), find(&g, 10));
    let mut count = 0;
    'a: while count < 100 {
        let mut k = 0;
        loop {
            k += 1;
            count += 1;
            if k == 3 {
                continue 'a;
            }
            if count > 20 {
                break 'a;
            }
        }
    }
    let v = 'blk: {
        if count > 10 {
            break 'blk count * 2;
        }
        0
    };
    let s = loop {
        count -= 7;
        if count < 0 {
            break String::from("neg");
        }
    };
    let mut it = vec![3, 1, 4].into_iter();
    let first_big = 'f: {
        while let Some(x) = it.next() {
            if x > 3 {
                break 'f Some(x);
            }
        }
        None
    };
    println!("{} {} {} {:?}", count, v, s, first_big);
}
