//! 記録した命令の列 (crustc_ir::Recorder) を機械語にする: minclif の FunctionBuilder に組み立て直し、
//! minclif のまま (レジスタ割り当て + x86-64) か、Cranelift のバックエンド (crustc-clif) で機械語にする

use crate::ir::{Recorder, Reloc, StackSlot};

/// minclif の記録から (機械語, 再配置, trap) を作るバックエンド (引数: 記録, 入口で 0 にするスロット, -O か)
pub type ClBackend = fn(&crate::minclif::FunctionBuilder, &[StackSlot], bool) -> Result<(Vec<u8>, Vec<Reloc>, Vec<(u32, u8)>), String>;

/// 記録を機械語にする。cl が None なら minclif で
pub fn lower(rec: &Recorder, late: &[StackSlot], opt: bool, cl: Option<ClBackend>) -> Result<(Vec<u8>, Vec<Reloc>, Vec<(u32, u8)>), String> {
    let (mut fb, sm) = crate::minclif::from_recording(rec);
    // (記録のスロットの番号を minclif のものに)
    let late: Vec<StackSlot> = late.iter().map(|s| StackSlot(sm[s.0 as usize])).collect();
    let late = &late[..];
    match cl {
        // (Cranelift のバックエンド: 記録した命令の列を Cranelift の IR にしてコンパイルする)
        Some(cl) => cl(&fb, late, opt),
        None => {
            fb.set_zero(late);
            let t = fb.traps();
            let (c, r) = fb.finish_zeroing(late);
            Ok((c, r, t))
        }
    }
}

