// PAL の Odin backend の GPU 版 (vendor:wgpu)。js_wasm32 ではブラウザの WebGPU を、ネイティブでは wgpu-native (mac は Metal) を使う。
// 描画命令をすべて GPU のインスタンス (四角形) にし、1 フレームを 1 回のインスタンス描画で描く。
// 角丸・枠線・破線・クリップ・透明度はフラグメントシェーダで計算し (CPU 版 raster.odin と同じ式)、文字は字形のアトラスから描く。
package gpu

import "base:runtime"
import "core:fmt"
import "core:math"
import stbtt "vendor:stb/truetype"
import "vendor:wgpu"

Color :: u32 // 0xAABBGGRR

/// GPU に送る 1 インスタンス (vec4 × 6 = 96 バイト)
Instance :: struct {
	rect:   [4]f32, // x, y, w, h (デバイスピクセル)
	fill:   [4]f32, // 塗り (文字では文字色)
	stroke: [4]f32,
	params: [4]f32, // radius, line_width, dash, kind (0 = 図形, 1 = 字形)
	clip:   [4]f32, // x0, y0, x1, y1
	uv:     [4]f32, // 字形のアトラス上の位置 (正規化)
}

MAX_INSTANCES :: 32768
ATLAS :: 2048

Clip :: struct { x0, y0, x1, y1: f32 }
Kind :: enum u8 { Clip, Layer }
Glyph_Key :: struct { cp: rune, size: u16 }
Glyph :: struct { w, h, xoff, yoff: i32, u0, v0: i32 }

state: struct {
	ctx:      runtime.Context,
	ready:    bool,
	failed:   bool,
	instance: wgpu.Instance,
	surface:  wgpu.Surface,
	adapter:  wgpu.Adapter,
	device:   wgpu.Device,
	queue:    wgpu.Queue,
	config:   wgpu.SurfaceConfiguration,
	pipeline: wgpu.RenderPipeline,
	vbuf:     wgpu.Buffer,
	ubuf:     wgpu.Buffer,
	atlas:    wgpu.Texture,
	bind:     wgpu.BindGroup,
}

W, H: u32
bg: [4]f32
/// canvas に表示せず、オフスクリーンにだけ描く (検証用)。ヘッドレスの Chromium では、WebGPU で canvas に表示すると
/// 「A valid external Instance reference no longer exists」になり、以降の mapAsync まで失敗するため
offscreen: bool

@(export)
set_offscreen :: proc "c" (on: b32) {
	offscreen = bool(on)
}
last_status: i32 = -100
instances: [dynamic]Instance
clips: [dynamic]Clip
alphas: [dynamic]f32
stack: [dynamic]Kind
font_data: []u8
font: stbtt.fontinfo
font_ok: bool
glyphs: map[Glyph_Key]Glyph
atlas_pixels: []u8
atlas_dirty: bool
shelf_x, shelf_y, shelf_h: i32

SHADER :: `
struct U { size: vec2f, atlas: vec2f };
@group(0) @binding(0) var<uniform> u: U;
@group(0) @binding(1) var atlas: texture_2d<f32>;
@group(0) @binding(2) var samp: sampler;

struct Inst {
	@location(0) rect: vec4f,
	@location(1) fill: vec4f,
	@location(2) stroke: vec4f,
	@location(3) params: vec4f,
	@location(4) clip: vec4f,
	@location(5) uv: vec4f,
};
struct VOut {
	@builtin(position) pos: vec4f,
	@location(0) p: vec2f,
	@location(1) uv: vec2f,
	@location(2) @interpolate(flat) rect: vec4f,
	@location(3) @interpolate(flat) fill: vec4f,
	@location(4) @interpolate(flat) stroke: vec4f,
	@location(5) @interpolate(flat) params: vec4f,
	@location(6) @interpolate(flat) clip: vec4f,
};

@vertex fn vs(@builtin(vertex_index) vi: u32, i: Inst) -> VOut {
	var corners = array<vec2f, 6>(vec2f(0.0, 0.0), vec2f(1.0, 0.0), vec2f(0.0, 1.0), vec2f(0.0, 1.0), vec2f(1.0, 0.0), vec2f(1.0, 1.0));
	let c = corners[vi];
	// 図形はアンチエイリアスと枠線のぶんだけ広げる
	let e = select(i.params.y * 0.5 + 1.0, 0.0, i.params.w > 0.5);
	let p = i.rect.xy - vec2f(e) + c * (i.rect.zw + vec2f(2.0 * e));
	var o: VOut;
	o.pos = vec4f(p.x / u.size.x * 2.0 - 1.0, 1.0 - p.y / u.size.y * 2.0, 0.0, 1.0);
	o.p = p;
	o.uv = mix(i.uv.xy, i.uv.zw, c);
	o.rect = i.rect; o.fill = i.fill; o.stroke = i.stroke; o.params = i.params; o.clip = i.clip;
	return o;
}

@fragment fn fs(v: VOut) -> @location(0) vec4f {
	if (v.p.x < v.clip.x || v.p.y < v.clip.y || v.p.x >= v.clip.z || v.p.y >= v.clip.w) { discard; }
	if (v.params.w > 0.5) {
		let a = textureSampleLevel(atlas, samp, v.uv, 0.0).r * v.fill.a;
		return vec4f(v.fill.rgb * a, a);
	}
	// 角丸矩形までの符号付き距離 (CPU 版 sd_round_rect と同じ)
	let h = max(v.rect.zw, vec2f(0.0)) * 0.5;
	let r = min(v.params.x, min(h.x, h.y));
	let q = abs(v.p - (v.rect.xy + h)) - (h - vec2f(r));
	let d = length(max(q, vec2f(0.0))) + min(max(q.x, q.y), 0.0) - r;
	let cf = clamp(0.5 - d, 0.0, 1.0) * v.fill.a;
	var cs = clamp(0.5 - (abs(d) - v.params.y * 0.5), 0.0, 1.0) * v.stroke.a;
	if (v.params.z > 0.5) {
		let t = select(v.p.x, v.p.y, q.x > q.y);
		if (t - floor(t / 9.0) * 9.0 >= 5.0) { cs = 0.0; }
	}
	// 枠線を塗りの上に重ねる (premultiplied)
	let a = cf * (1.0 - cs) + cs;
	return vec4f(v.fill.rgb * cf * (1.0 - cs) + v.stroke.rgb * cs, a);
}
`

main :: proc() {
	state.ctx = context
	state.instance = wgpu.CreateInstance(nil)
	if state.instance == nil {
		state.failed = true
		return
	}
	if !offscreen {
		state.surface = wgpu.InstanceCreateSurface(state.instance, &wgpu.SurfaceDescriptor{
			nextInChain = &wgpu.SurfaceSourceCanvasHTMLSelector{sType = .SurfaceSourceCanvasHTMLSelector, selector = "#gpu-screen"},
		})
	}
	wgpu.InstanceRequestAdapter(state.instance, &{compatibleSurface = state.surface}, {callback = on_adapter})
}

on_adapter :: proc "c" (status: wgpu.RequestAdapterStatus, adapter: wgpu.Adapter, message: string, u1, u2: rawptr) {
	context = state.ctx
	if status != .Success || adapter == nil {
		state.failed = true
		return
	}
	state.adapter = adapter
	wgpu.AdapterRequestDevice(adapter, nil, {callback = on_device})
}

on_device :: proc "c" (status: wgpu.RequestDeviceStatus, device: wgpu.Device, message: string, u1, u2: rawptr) {
	context = state.ctx
	if status != .Success || device == nil {
		state.failed = true
		return
	}
	state.device = device
	state.queue = wgpu.DeviceGetQueue(device)

	module := wgpu.DeviceCreateShaderModule(device, &{nextInChain = &wgpu.ShaderSourceWGSL{sType = .ShaderSourceWGSL, code = SHADER}})
	bgl := wgpu.DeviceCreateBindGroupLayout(device, &{
		entryCount = 3,
		entries = raw_data([]wgpu.BindGroupLayoutEntry{
			{binding = 0, visibility = {.Vertex, .Fragment}, buffer = {type = .Uniform}},
			{binding = 1, visibility = {.Fragment}, texture = {sampleType = .Float, viewDimension = ._2D}},
			{binding = 2, visibility = {.Fragment}, sampler = {type = .Filtering}},
		}),
	})
	layout := wgpu.DeviceCreatePipelineLayout(device, &{bindGroupLayoutCount = 1, bindGroupLayouts = &bgl})
	attrs := []wgpu.VertexAttribute{
		{format = .Float32x4, offset = 0, shaderLocation = 0},
		{format = .Float32x4, offset = 16, shaderLocation = 1},
		{format = .Float32x4, offset = 32, shaderLocation = 2},
		{format = .Float32x4, offset = 48, shaderLocation = 3},
		{format = .Float32x4, offset = 64, shaderLocation = 4},
		{format = .Float32x4, offset = 80, shaderLocation = 5},
	}
	blend := wgpu.BlendState{
		color = {operation = .Add, srcFactor = .One, dstFactor = .OneMinusSrcAlpha},
		alpha = {operation = .Add, srcFactor = .One, dstFactor = .OneMinusSrcAlpha},
	}
	state.pipeline = wgpu.DeviceCreateRenderPipeline(device, &{
		layout = layout,
		vertex = {
			module = module,
			entryPoint = "vs",
			bufferCount = 1,
			buffers = &wgpu.VertexBufferLayout{stepMode = .Instance, arrayStride = size_of(Instance), attributeCount = uint(len(attrs)), attributes = raw_data(attrs)},
		},
		fragment = &{
			module = module,
			entryPoint = "fs",
			targetCount = 1,
			targets = &wgpu.ColorTargetState{format = .BGRA8Unorm, blend = &blend, writeMask = wgpu.ColorWriteMaskFlags_All},
		},
		primitive = {topology = .TriangleList},
		multisample = {count = 1, mask = 0xFFFFFFFF},
	})
	state.vbuf = wgpu.DeviceCreateBuffer(device, &{usage = {.Vertex, .CopyDst}, size = MAX_INSTANCES * size_of(Instance)})
	state.ubuf = wgpu.DeviceCreateBuffer(device, &{usage = {.Uniform, .CopyDst}, size = 16})
	state.atlas = wgpu.DeviceCreateTexture(device, &{
		usage = {.TextureBinding, .CopyDst},
		dimension = ._2D,
		size = {ATLAS, ATLAS, 1},
		format = .R8Unorm,
		mipLevelCount = 1,
		sampleCount = 1,
	})
	// 字形は整数の画素に置くので、最近傍で読む (CPU 版と同じ画素になる)
	sampler := wgpu.DeviceCreateSampler(device, &{
		addressModeU = .ClampToEdge, addressModeV = .ClampToEdge, addressModeW = .ClampToEdge,
		magFilter = .Nearest, minFilter = .Nearest, mipmapFilter = .Nearest,
		lodMinClamp = 0, lodMaxClamp = 1, maxAnisotropy = 1,
	})
	view := wgpu.TextureCreateView(state.atlas, nil)
	state.bind = wgpu.DeviceCreateBindGroup(device, &{
		layout = bgl,
		entryCount = 3,
		entries = raw_data([]wgpu.BindGroupEntry{
			{binding = 0, buffer = state.ubuf, size = 16},
			{binding = 1, textureView = view},
			{binding = 2, sampler = sampler},
		}),
	})
	atlas_pixels = make([]u8, ATLAS * ATLAS)
	state.ready = true
}

/// 直近のフレームで表面のテクスチャを取ったときの状態 (調べる用)
@(export)
surface_status :: proc "c" () -> i32 {
	return last_status
}

@(export)
is_ready :: proc "c" () -> i32 {
	return state.failed ? -1 : state.ready ? 1 : 0
}

@(export)
alloc :: proc "c" (n: i32) -> rawptr {
	context = state.ctx
	return raw_data(make([]u8, int(n)))
}

@(export)
resize :: proc "c" (w, h: u32) {
	context = state.ctx
	W, H = w, h
	if offscreen do return
	state.config = wgpu.SurfaceConfiguration{
		device = state.device,
		usage = {.RenderAttachment},
		format = .BGRA8Unorm,
		width = w,
		height = h,
		presentMode = .Fifo,
		alphaMode = .Opaque,
	}
	wgpu.SurfaceConfigure(state.surface, &state.config)
}

to_f :: proc(c: Color) -> [4]f32 {
	return {f32(c & 0xff) / 255, f32((c >> 8) & 0xff) / 255, f32((c >> 16) & 0xff) / 255, f32(c >> 24) / 255}
}

@(export)
frame_begin :: proc "c" (background: Color) {
	context = state.ctx
	bg = to_f(background)
	clear(&instances)
	clear(&clips)
	clear(&alphas)
	clear(&stack)
}

current_clip :: proc() -> [4]f32 {
	c := [4]f32{0, 0, f32(W), f32(H)}
	for k in clips do c = {max(c.x, k.x0), max(c.y, k.y0), min(c.z, k.x1), min(c.w, k.y1)}
	return c
}

current_alpha :: proc() -> f32 {
	a: f32 = 1
	for v in alphas do a *= v
	return a
}

push :: proc(inst: Instance) {
	if len(instances) < MAX_INSTANCES do append(&instances, inst)
}

@(export)
rect :: proc "c" (x, y, w, h, radius: f32, fill: Color, stroke: Color, line_width: f32, dash: b32) {
	context = state.ctx
	a := current_alpha()
	f := to_f(fill)
	s := to_f(stroke)
	f.a *= a
	s.a *= a
	push({rect = {x, y, w, h}, fill = f, stroke = s, params = {radius, line_width, dash ? 1 : 0, 0}, clip = current_clip()})
}

@(export)
clip :: proc "c" (x, y, w, h: f32) {
	context = state.ctx
	append(&clips, Clip{x, y, x + w, y + h})
	append(&stack, Kind.Clip)
}

@(export)
layer :: proc "c" (alpha: f32) {
	context = state.ctx
	append(&alphas, alpha)
	append(&stack, Kind.Layer)
}

@(export)
pop_state :: proc "c" () {
	context = state.ctx
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
	context = state.ctx
	font_data = ptr[:n]
	font_ok = bool(stbtt.InitFont(&font, raw_data(font_data), stbtt.GetFontOffsetForIndex(raw_data(font_data), 0)))
	return b32(font_ok)
}

/// 字形をアトラスに焼く (棚詰め)。いっぱいになったら焼かない
glyph :: proc(cp: rune, size: f32) -> Glyph {
	key := Glyph_Key{cp, u16(size * 4)}
	if g, ok := glyphs[key]; ok do return g
	scale := stbtt.ScaleForMappingEmToPixels(&font, size)
	x0, y0, x1, y1: i32
	stbtt.GetCodepointBitmapBox(&font, cp, scale, scale, &x0, &y0, &x1, &y1)
	g := Glyph{w = x1 - x0, h = y1 - y0, xoff = x0, yoff = y0}
	if g.w > 0 && g.h > 0 {
		if shelf_x + g.w + 1 > ATLAS {
			shelf_x = 0
			shelf_y += shelf_h + 1
			shelf_h = 0
		}
		if shelf_y + g.h <= ATLAS {
			g.u0, g.v0 = shelf_x, shelf_y
			stbtt.MakeCodepointBitmap(&font, &atlas_pixels[shelf_y * ATLAS + shelf_x], g.w, g.h, ATLAS, scale, scale, cp)
			shelf_x += g.w + 1
			shelf_h = max(shelf_h, g.h)
			atlas_dirty = true
		} else {
			g.w, g.h = 0, 0
		}
	}
	glyphs[key] = g
	return g
}

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
	context = state.ctx
	if !font_ok do return 0
	return advance(string(ptr[:n]), size)
}

@(export)
text :: proc "c" (x, y, size: f32, color: Color, underline: b32, ptr: [^]u8, n: i32) {
	context = state.ctx
	if !font_ok do return
	s := string(ptr[:n])
	scale := stbtt.ScaleForMappingEmToPixels(&font, size)
	asc, desc, gap: i32
	stbtt.GetFontVMetrics(&font, &asc, &desc, &gap)
	baseline := y + (f32(asc) + f32(desc)) * scale / 2
	c := to_f(color)
	c.a *= current_alpha()
	cl := current_clip()
	pen := x
	prev: rune = 0
	for r in s {
		if prev != 0 do pen += f32(stbtt.GetCodepointKernAdvance(&font, prev, r)) * scale
		g := glyph(r, size)
		if g.w > 0 {
			gx := math.round(pen) + f32(g.xoff)
			gy := math.round(baseline) + f32(g.yoff)
			push({
				rect = {gx, gy, f32(g.w), f32(g.h)},
				fill = c,
				params = {0, 0, 0, 1},
				clip = cl,
				uv = {f32(g.u0) / ATLAS, f32(g.v0) / ATLAS, f32(g.u0 + g.w) / ATLAS, f32(g.v0 + g.h) / ATLAS},
			})
		}
		adv, lsb: i32
		stbtt.GetCodepointHMetrics(&font, r, &adv, &lsb)
		pen += f32(adv) * scale
		prev = r
	}
	if underline {
		w := advance(s, size)
		push({rect = {x, y + size * 0.62, w, 1.5}, fill = c, params = {0, 0, 0, 0}, clip = cl})
	}
}

/// たまった命令を GPU に送る (アトラス、uniform、インスタンス)
upload :: proc() -> int {
	if atlas_dirty {
		wgpu.QueueWriteTexture(state.queue, &{texture = state.atlas}, raw_data(atlas_pixels), uint(len(atlas_pixels)), &{bytesPerRow = ATLAS, rowsPerImage = ATLAS}, &{ATLAS, ATLAS, 1})
		atlas_dirty = false
	}
	u := [4]f32{f32(W), f32(H), ATLAS, ATLAS}
	wgpu.QueueWriteBuffer(state.queue, state.ubuf, 0, &u, size_of(u))
	n := len(instances)
	if n > 0 do wgpu.QueueWriteBuffer(state.queue, state.vbuf, 0, raw_data(instances), uint(n * size_of(Instance)))
	return n
}

/// 1 回のインスタンス描画のパスを積む (表示用と読み戻し用で共通)
encode :: proc(enc: wgpu.CommandEncoder, view: wgpu.TextureView, n: int) {
	pass := wgpu.CommandEncoderBeginRenderPass(enc, &{
		colorAttachmentCount = 1,
		colorAttachments = &wgpu.RenderPassColorAttachment{
			view = view,
			loadOp = .Clear,
			storeOp = .Store,
			depthSlice = wgpu.DEPTH_SLICE_UNDEFINED,
			clearValue = {f64(bg.r), f64(bg.g), f64(bg.b), 1},
		},
	})
	wgpu.RenderPassEncoderSetPipeline(pass, state.pipeline)
	wgpu.RenderPassEncoderSetBindGroup(pass, 0, state.bind)
	if n > 0 {
		wgpu.RenderPassEncoderSetVertexBuffer(pass, 0, state.vbuf, 0, u64(n * size_of(Instance)))
		wgpu.RenderPassEncoderDraw(pass, 6, u32(n), 0, 0)
	}
	wgpu.RenderPassEncoderEnd(pass)
	wgpu.RenderPassEncoderRelease(pass)
}

/// たまった命令を 1 回のインスタンス描画で描く
@(export)
frame_end :: proc "c" () -> i32 {
	context = state.ctx
	if !state.ready do return 0
	n := upload()
	if offscreen do return i32(n) // 描くのは capture のとき
	st := wgpu.SurfaceGetCurrentTexture(state.surface)
	last_status = i32(st.status)
	// ブラウザ (Odin の wgpu.js) は状態を書き込まない (TODO のまま) ので 0 になる。テクスチャが取れていれば成功とみなす
	ok := st.texture != nil && (st.status == .SuccessOptimal || st.status == .SuccessSuboptimal || i32(st.status) == 0)
	if !ok {
		if st.texture != nil do wgpu.TextureRelease(st.texture)
		return 0
	}
	defer wgpu.TextureRelease(st.texture)
	view := wgpu.TextureCreateView(st.texture, nil)
	defer wgpu.TextureViewRelease(view)
	enc := wgpu.DeviceCreateCommandEncoder(state.device, nil)
	defer wgpu.CommandEncoderRelease(enc)
	encode(enc, view, n)
	cmd := wgpu.CommandEncoderFinish(enc, nil)
	defer wgpu.CommandBufferRelease(cmd)
	wgpu.QueueSubmit(state.queue, {cmd})
	wgpu.SurfacePresent(state.surface)
	return i32(n)
}

// ---- 読み戻し (検証用): 同じ命令をオフスクリーンのテクスチャに描き、画素を CPU に戻す ----
cap: struct {
	tex:    wgpu.Texture,
	buf:    wgpu.Buffer,
	w, h:   u32,
	bpr:    u32,
	pixels: []u8, // RGBA に並べ替えたもの
	state:  i32,  // 0 = 何もしていない, 1 = 待ち, 2 = 済み, -1 = 失敗
}

@(export)
capture :: proc "c" () -> i32 {
	context = state.ctx
	if !state.ready || cap.state == 1 do return 0
	if cap.w != W || cap.h != H {
		if cap.tex != nil do wgpu.TextureRelease(cap.tex)
		if cap.buf != nil do wgpu.BufferRelease(cap.buf)
		cap.w, cap.h = W, H
		cap.bpr = (W * 4 + 255) / 256 * 256
		cap.tex = wgpu.DeviceCreateTexture(state.device, &{usage = {.RenderAttachment, .CopySrc}, dimension = ._2D, size = {W, H, 1}, format = .BGRA8Unorm, mipLevelCount = 1, sampleCount = 1})
		cap.buf = wgpu.DeviceCreateBuffer(state.device, &{usage = {.CopyDst, .MapRead}, size = u64(cap.bpr * H)})
		delete(cap.pixels)
		cap.pixels = make([]u8, int(W * H * 4))
	}
	n := upload()
	view := wgpu.TextureCreateView(cap.tex, nil)
	defer wgpu.TextureViewRelease(view)
	enc := wgpu.DeviceCreateCommandEncoder(state.device, nil)
	defer wgpu.CommandEncoderRelease(enc)
	encode(enc, view, n)
	wgpu.CommandEncoderCopyTextureToBuffer(enc, &{texture = cap.tex}, &{layout = {bytesPerRow = cap.bpr, rowsPerImage = H}, buffer = cap.buf}, &{W, H, 1})
	cmd := wgpu.CommandEncoderFinish(enc, nil)
	defer wgpu.CommandBufferRelease(cmd)
	wgpu.QueueSubmit(state.queue, {cmd})
	cap.state = 1
	wgpu.BufferMapAsync(cap.buf, {.Read}, 0, uint(cap.bpr * H), {callback = on_mapped})
	return 1
}

on_mapped :: proc "c" (status: wgpu.MapAsyncStatus, message: wgpu.StringView, u1, u2: rawptr) {
	context = state.ctx
	if status != .Success {
		fmt.println("[gpu] map 失敗:", status, message)
		cap.state = -1
		return
	}
	if cap.state != 1 do return // 失敗のあとに成功が来ることがある (wgpu.js の .catch().then())
	src := wgpu.BufferGetConstMappedRange(cap.buf, 0, uint(cap.bpr * cap.h))
	for y in 0 ..< int(cap.h) {
		for x in 0 ..< int(cap.w) {
			s := y * int(cap.bpr) + x * 4
			d := (y * int(cap.w) + x) * 4
			cap.pixels[d + 0] = src[s + 2] // BGRA → RGBA
			cap.pixels[d + 1] = src[s + 1]
			cap.pixels[d + 2] = src[s + 0]
			cap.pixels[d + 3] = src[s + 3]
		}
	}
	wgpu.BufferUnmap(cap.buf)
	cap.state = 2
}

@(export)
capture_state :: proc "c" () -> i32 {
	return cap.state
}

@(export)
capture_ptr :: proc "c" () -> rawptr {
	return raw_data(cap.pixels)
}
