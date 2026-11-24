//! tract (純 Rust の ONNX ランタイム) で ../lgbm.onnx・../mlp16.onnx を推論し、速さと Python の予測との差を測る。
//!   cargo run --release
//!   ORT_DYLIB_PATH=<libonnxruntime.so> cargo run --release --features ort   (ONNX Runtime 本体でも測る)
use std::time::Instant;
use tract_onnx::prelude::*;

const D: usize = 11;
const ROWS: usize = 100_000;

fn read_f32(path: &str) -> Vec<f32> {
    std::fs::read(path).unwrap().chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect()
}

fn main() -> TractResult<()> {
    let here = concat!(env!("CARGO_MANIFEST_DIR"), "/..");
    let x = read_f32(&format!("{here}/../data/cache_X.f32"));
    for name in ["lgbm", "mlp16"] {
        let path = format!("{here}/{name}.onnx");
        // 1 行用と、10 万行まとめて用に、入力の形を固定した 2 つのプランを作る
        let plan = |n: usize| -> TractResult<_> {
            tract_onnx::onnx()
                .model_for_path(&path)?
                .with_input_fact(0, f32::fact([n, D]).into())?
                .into_optimized()?
                .into_runnable()
        };
        let (one, batch) = match (plan(1), plan(ROWS)) {
            (Ok(a), Ok(b)) => (a, b),
            (Err(e), _) | (_, Err(e)) => {
                println!("tract {name}: 読み込めない: {e:#}");
                println!("bench lang=rust-tract task=onnx model={name} error=1");
                continue;
            }
        };
        let xs = &x[..ROWS * D];
        let input = tract_ndarray::Array2::from_shape_vec((ROWS, D), xs.to_vec())?.into_tensor();
        batch.run(tvec!(input.clone().into()))?;
        let t0 = Instant::now();
        let out = batch.run(tvec!(input.into()))?;
        let batch_ns = t0.elapsed().as_nanos() as f64 / ROWS as f64;
        let ours: Vec<f32> = out[0].to_plain_array_view::<f32>()?.iter().copied().collect();

        let k = 20_000;
        let t0 = Instant::now();
        for i in 0..k {
            let row = tract_ndarray::Array2::from_shape_vec((1, D), xs[i * D..(i + 1) * D].to_vec())?.into_tensor();
            std::hint::black_box(one.run(tvec!(row.into()))?);
        }
        let one_ns = t0.elapsed().as_nanos() as f64 / k as f64;

        let r = read_f32(&format!("{here}/ref_{name}.f32"));
        let max = ours.iter().zip(&r).map(|(a, b)| (a - b).abs()).fold(0f32, f32::max);
        println!("rust tract {name}: 1-row {one_ns:.0} ns, batch {batch_ns:.0} ns/row, max |diff| vs python {max:.1e}");
        println!("bench lang=rust-tract task=onnx model={name} one_row_ns={one_ns:.0} batch_ns={batch_ns:.0}");
    }
    #[cfg(feature = "ort")]
    run_ort(here, &x).expect("ort");
    Ok(())
}

#[cfg(feature = "ort")]
fn run_ort(here: &str, x: &[f32]) -> ort::Result<()> {
    use ort::session::Session;
    use ort::value::TensorRef;
    let lib = std::env::var("ORT_DYLIB_PATH").expect("ORT_DYLIB_PATH に libonnxruntime.so の場所を入れる");
    ort::init_from(&lib).expect("load libonnxruntime").commit();
    for name in ["lgbm", "mlp16"] {
        let mut sess = Session::builder()?.with_intra_threads(1)?.commit_from_file(format!("{here}/{name}.onnx"))?;
        let xs = &x[..ROWS * D];
        let mut batch = |sess: &mut Session| -> ort::Result<Vec<f32>> {
            let out = sess.run(ort::inputs!["x" => TensorRef::from_array_view(([ROWS, D], xs))?])?;
            Ok(out["p"].try_extract_tensor::<f32>()?.1.to_vec())
        };
        batch(&mut sess)?;
        let t0 = Instant::now();
        let ours = batch(&mut sess)?;
        let batch_ns = t0.elapsed().as_nanos() as f64 / ROWS as f64;
        let k = 20_000;
        let t0 = Instant::now();
        for i in 0..k {
            let out = sess.run(ort::inputs!["x" => TensorRef::from_array_view(([1usize, D], &xs[i * D..(i + 1) * D]))?])?;
            std::hint::black_box(out["p"].try_extract_tensor::<f32>()?.1[0]);
        }
        let one_ns = t0.elapsed().as_nanos() as f64 / k as f64;
        let r = read_f32(&format!("{here}/ref_{name}.f32"));
        let max = ours.iter().zip(&r).map(|(a, b)| (a - b).abs()).fold(0f32, f32::max);
        println!("rust ort {name}: 1-row {one_ns:.0} ns, batch {batch_ns:.0} ns/row, max |diff| vs python {max:.1e}");
        println!("bench lang=rust-ort task=onnx model={name} one_row_ns={one_ns:.0} batch_ns={batch_ns:.0}");
    }
    Ok(())
}
