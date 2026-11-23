// PAL の Odin backend (js_wasm32)。shell の描画命令 (P4b) を、命令ごとの関数呼び出しで受け取り、ソフトウェアで描く。
// 文字は stb_truetype で、shell と同じフォントファイルを使う。計測 (measure) も同じ規則で返す。
package raster

import "base:runtime"
import "core:math"
import stbtt "vendor:stb/truetype"

// 色は 0xAABBGGRR (little endian で ImageData のバイト順 R, G, B, A になる)
Color :: u32

Clip :: struct { x0, y0, x1, y1: f32 }
Kind :: enum u8 { Clip, Layer }

Glyph_Key :: struct { cp: rune, size: u16 }
Glyph :: struct {
	w, h, xoff, yoff: i32,
	bitmap:           []u8,
}

W, H: i32
fb: []u32
clips: [dynamic]Clip
alphas: [dynamic]f32
stack: [dynamic]Kind
font_data: []u8
font: stbtt.fontinfo
font_ok: bool
glyphs: map[Glyph_Key]Glyph
scratch: []u8

ctx: runtime.Context

// 空の main を置いてエントリポイント付きでビルドする。
// _start がランタイムの @(init) を実行し、stb_truetype が使う libc-shim の malloc が初期化される
// (-no-entry-point にすると @(init) が走らず、malloc が失敗した)
main :: proc() {}

@(export)
init :: proc "c" () {
	ctx = runtime.default_context()
}

/// JS から書き込む領域 (文字列やフォント) を確保する
@(export)
alloc :: proc "c" (n: i32) -> rawptr {
	context = ctx
	return raw_data(make([]u8, int(n)))
}

/// 描画先の大きさ (デバイスピクセル)。フレームバッファの先頭を返す
@(export)
resize :: proc "c" (w, h: i32) -> rawptr {
	context = ctx
	if fb != nil do delete(fb)
	W, H = w, h
	fb = make([]u32, int(w * h))
	return raw_data(fb)
}

@(export)
frame_begin :: proc "c" (bg: Color) {
	context = ctx
	for &p in fb do p = bg
	clear(&clips)
	clear(&alphas)
	clear(&stack)
}

current_clip :: proc() -> Clip {
	c := Clip{0, 0, f32(W), f32(H)}
	for k in clips {
		c = Clip{max(c.x0, k.x0), max(c.y0, k.y0), min(c.x1, k.x1), min(c.y1, k.y1)}
	}
	return c
}

current_alpha :: proc() -> f32 {
	a: f32 = 1
	for v in alphas do a *= v
	return a
}

/// 1 画素に、被覆率 cov で色を重ねる (source-over)
blend :: #force_inline proc(i: int, c: Color, cov: f32) {
	sa := f32(c >> 24) / 255 * cov
	if sa <= 0 do return
	d := fb[i]
	inv := 1 - sa
	r := f32(c & 0xff) * sa + f32(d & 0xff) * inv
	g := f32((c >> 8) & 0xff) * sa + f32((d >> 8) & 0xff) * inv
	b := f32((c >> 16) & 0xff) * sa + f32((d >> 16) & 0xff) * inv
	a := sa * 255 + f32(d >> 24) * inv
	fb[i] = u32(r + 0.5) | u32(g + 0.5) << 8 | u32(b + 0.5) << 16 | u32(min(a, 255) + 0.5) << 24
}

/// 角丸矩形までの符号付き距離 (内側が負)
sd_round_rect :: #force_inline proc(px, py, cx, cy, hx, hy, r: f32) -> (d: f32, qx: f32, qy: f32) {
	qx = abs(px - cx) - (hx - r)
	qy = abs(py - cy) - (hy - r)
	ox := max(qx, 0)
	oy := max(qy, 0)
	d = math.sqrt(ox * ox + oy * oy) + min(max(qx, qy), 0) - r
	return
}

@(export)
rect :: proc "c" (x, y, w, h, radius: f32, fill: Color, stroke: Color, line_width: f32, dash: b32) {
	context = ctx
	if w <= 0 && h <= 0 do return
	alpha := current_alpha()
	cl := current_clip()
	pad := line_width + 1
	x0 := i32(max(math.floor(x - pad), cl.x0))
	y0 := i32(max(math.floor(y - pad), cl.y0))
	x1 := i32(min(math.ceil(x + w + pad), cl.x1))
	y1 := i32(min(math.ceil(y + h + pad), cl.y1))
	hx, hy := max(w, 0) / 2, max(h, 0) / 2
	cx, cy := x + hx, y + hy
	r := min(radius, min(hx, hy))
	fa := (fill >> 24) != 0
	sa := (stroke >> 24) != 0
	// 不透明で全面を覆う画素は、合成せずに直接書ける
	opaque := (fill >> 24) == 0xff && alpha >= 1
	// 角から離れた内側 (この範囲なら距離を計算しなくても被覆率 1): 左右は r、上下は r の内側で、0.5 画素の余裕を取る
	ix0, ix1 := x + r + 0.5, x + w - r - 0.5
	iy0, iy1 := y + r + 0.5, y + h - r - 0.5
	ex0, ex1 := x + 0.5, x + w - 0.5
	ey0, ey1 := y + 0.5, y + h - 0.5
	band := line_width / 2 + 1
	for py in y0 ..< y1 {
		fy := f32(py) + 0.5
		row := int(py * W)
		px := x0
		for px < x1 {
			fx := f32(px) + 0.5
			// 内側の確実な領域: 塗りは一気に書き、枠線だけなら辺の付近まで飛ばす
			inner := ((fx >= ix0 && fx <= ix1) && (fy >= ey0 && fy <= ey1)) || ((fy >= iy0 && fy <= iy1) && (fx >= ex0 && fx <= ex1))
			if inner {
				deep := fx > ex0 + band && fx < ex1 - band && fy > ey0 + band && fy < ey1 - band
				if !sa || deep {
					if fa {
						if opaque do fb[row + int(px)] = fill
						else do blend(row + int(px), fill, alpha)
					} else if deep {
						// 枠線だけ: 右の辺の付近まで飛ばす
						next := i32(math.floor(ex1 - band))
						if next > px {
							px = next
							continue
						}
					}
					px += 1
					continue
				}
			}
			d, qx, qy := sd_round_rect(fx, fy, cx, cy, hx, hy, r)
			i := row + int(px)
			if fa {
				cov := clamp(0.5 - d, 0, 1)
				if cov > 0 do blend(i, fill, cov * alpha)
			}
			if sa {
				cov := clamp(0.5 - (abs(d) - line_width / 2), 0, 1)
				if cov > 0 && dash {
					// 破線: 辺に沿った位置で 5px 描いて 4px 空ける
					t := qx > qy ? fy : fx
					if math.mod(t, 9) >= 5 do cov = 0
				}
				if cov > 0 do blend(i, stroke, cov * alpha)
			}
			px += 1
		}
	}
}

@(export)
clip :: proc "c" (x, y, w, h: f32) {
	context = ctx
	append(&clips, Clip{x, y, x + w, y + h})
	append(&stack, Kind.Clip)
}

@(export)
layer :: proc "c" (alpha: f32) {
	context = ctx
	append(&alphas, alpha)
	append(&stack, Kind.Layer)
}

// 組み込みの pop と名前がぶつからないよう pop_state にする
@(export)
pop_state :: proc "c" () {
	context = ctx
	if len(stack) == 0 do return
	switch pop(&stack) {
	case .Clip:
		pop(&clips)
	case .Layer:
		pop(&alphas)
	}
}

@(export)
load_font :: proc "c" (ptr: [^]u8, n: i32) -> b32 {
	context = ctx
	font_data = ptr[:n]
	font_ok = bool(stbtt.InitFont(&font, raw_data(font_data), stbtt.GetFontOffsetForIndex(raw_data(font_data), 0)))
	return b32(font_ok)
}

glyph :: proc(cp: rune, size: f32) -> ^Glyph {
	key := Glyph_Key{cp, u16(size * 4)}
	if g, ok := &glyphs[key]; ok do return g
	scale := stbtt.ScaleForMappingEmToPixels(&font, size)
	w, h, xoff, yoff: i32
	bm := stbtt.GetCodepointBitmap(&font, scale, scale, cp, &w, &h, &xoff, &yoff)
	g := Glyph{w = w, h = h, xoff = xoff, yoff = yoff}
	if bm != nil {
		g.bitmap = make([]u8, int(w * h))
		copy(g.bitmap, bm[:w * h])
		stbtt.FreeBitmap(bm, nil)
	}
	glyphs[key] = g
	return &glyphs[key]
}

/// 文字列の幅 (送り幅 + カーニング)。text と同じ規則
advance :: proc(s: string, size: f32) -> f32 {
	scale := stbtt.ScaleForMappingEmToPixels(&font, size)
	x: f32 = 0
	prev: rune = 0
	for r in s {
		if prev != 0 do x += f32(stbtt.GetCodepointKernAdvance(&font, prev, r)) * scale
		adv, lsb: i32
		stbtt.GetCodepointHMetrics(&font, r, &adv, &lsb)
		x += f32(adv) * scale
		prev = r
	}
	return x
}

@(export)
measure :: proc "c" (ptr: [^]u8, n: i32, size: f32) -> f32 {
	context = ctx
	if !font_ok do return 0
	return advance(string(ptr[:n]), size)
}

/// y は Canvas の textBaseline = 'middle' と同じく、em の中央
@(export)
text :: proc "c" (x, y, size: f32, color: Color, underline: b32, ptr: [^]u8, n: i32) {
	context = ctx
	if !font_ok do return
	s := string(ptr[:n])
	scale := stbtt.ScaleForMappingEmToPixels(&font, size)
	asc, desc, gap: i32
	stbtt.GetFontVMetrics(&font, &asc, &desc, &gap)
	baseline := y + (f32(asc) + f32(desc)) * scale / 2
	alpha := current_alpha()
	cl := current_clip()
	pen := x
	prev: rune = 0
	for r in s {
		if prev != 0 do pen += f32(stbtt.GetCodepointKernAdvance(&font, prev, r)) * scale
		g := glyph(r, size)
		gx := i32(math.round(pen)) + g.xoff
		gy := i32(math.round(baseline)) + g.yoff
		for j in 0 ..< g.h {
			py := gy + j
			if f32(py) < cl.y0 || f32(py) >= cl.y1 do continue
			for k in 0 ..< g.w {
				px := gx + k
				if f32(px) < cl.x0 || f32(px) >= cl.x1 do continue
				cov := f32(g.bitmap[j * g.w + k]) / 255
				if cov > 0 do blend(int(py * W + px), color, cov * alpha)
			}
		}
		adv, lsb: i32
		stbtt.GetCodepointHMetrics(&font, r, &adv, &lsb)
		pen += f32(adv) * scale
		prev = r
	}
	if underline {
		w := advance(s, size)
		rect(x, y + size * 0.62, w, 1.5, 0, color, 0, 0, false)
	}
}
