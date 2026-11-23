// ネイティブのホストの描画プロセス: Node のホスト (native/host.ts) から描画命令を標準入力で受け、
// backend-odin-gpu (ブラウザの WebGPU 版と同じコード) で wgpu-native に描く。入力は SDL3 から取り、JSON Lines で標準出力に返す。
//
//   renderer                      # SDL3 の窓 (X11 / Wayland / mac は Metal)
//   renderer --headless 1280x800  # 窓なし。オフスクリーンに描き、capture で PNG に書く (テスト・CI 用)
//
// 標準入力 (Node → 描画): [u32 長さ][u8 種類][中身] の繰り返し (little endian)。座標は論理ピクセル (描画側が拡大率を掛ける)
//   1 FONT    フォントのバイト列
//   2 FRAME   u32 背景色, 描画命令の列 (下の decode_frame)
//   3 CAPTURE PNG の書き出し先のパス
//   4 IME     u8 有効, f32 x, y, w, h (キャレット)
//   5 CURSOR  u8 種類
//   6 QUIT
// 標準出力 (描画 → Node): 1 行 1 つの JSON。ready / size / down / move / up / wheel / key / text / preedit / frame / captured / close / error
package renderer

import "base:runtime"
import "core:c"
import "core:fmt"
import "core:os"
import "core:strconv"
import "core:strings"
import "core:sync"
import "core:sys/posix"
import "core:thread"
import "core:time"
import gpu "../../backend-odin-gpu"
import sdl "vendor:sdl3"
import stbi "vendor:stb/image"
import "vendor:wgpu"
import sdlglue "vendor:wgpu/sdl3glue"

MSG_FONT :: 1
MSG_FRAME :: 2
MSG_CAPTURE :: 3
MSG_IME :: 4
MSG_CURSOR :: 5
MSG_QUIT :: 6

OP_RECT :: 1
OP_TEXT :: 2
OP_CLIP :: 3
OP_LAYER :: 4
OP_POP :: 5

headless: bool
win_w, win_h: i32 = 1280, 800
scale: f32 = 1
window: ^sdl.Window
quit: bool

// ---- 標準出力: JSON Lines ----

emit :: proc(s: string) {
	line := strings.concatenate({s, "\n"}, context.temp_allocator)
	b := transmute([]u8)line
	for len(b) > 0 {
		n := posix.write(1, raw_data(b), uint(len(b)))
		if n <= 0 do return
		b = b[n:]
	}
}

json_str :: proc(s: string) -> string {
	sb := strings.builder_make(context.temp_allocator)
	strings.write_byte(&sb, '"')
	for r in s {
		switch r {
		case '"':
			strings.write_string(&sb, "\\\"")
		case '\\':
			strings.write_string(&sb, "\\\\")
		case '\n':
			strings.write_string(&sb, "\\n")
		case '\r':
			strings.write_string(&sb, "\\r")
		case '\t':
			strings.write_string(&sb, "\\t")
		case:
			if r < 0x20 do fmt.sbprintf(&sb, "\\u%04x", int(r))
			else do strings.write_rune(&sb, r)
		}
	}
	strings.write_byte(&sb, '"')
	return strings.to_string(sb)
}

// ---- 標準入力: [u32 長さ][本体] ----

read_exact :: proc(buf: []u8) -> bool {
	off := 0
	for off < len(buf) {
		n := posix.read(0, raw_data(buf[off:]), uint(len(buf)) - uint(off))
		if n <= 0 do return false
		off += int(n)
	}
	return true
}

/// 1 通を読む。終わり (Node が閉じた) なら nil
read_msg :: proc() -> []u8 {
	hdr: [4]u8
	if !read_exact(hdr[:]) do return nil
	n := int(u32(hdr[0]) | u32(hdr[1]) << 8 | u32(hdr[2]) << 16 | u32(hdr[3]) << 24)
	if n <= 0 || n > 256 << 20 do return nil
	body := make([]u8, n)
	if !read_exact(body) {
		delete(body)
		return nil
	}
	return body
}

Reader :: struct {
	b: []u8,
	i: int,
}
r_u8 :: proc(r: ^Reader) -> u8 {
	if r.i + 1 > len(r.b) do return 0
	v := r.b[r.i]
	r.i += 1
	return v
}
r_u32 :: proc(r: ^Reader) -> u32 {
	if r.i + 4 > len(r.b) do return 0
	v := u32(r.b[r.i]) | u32(r.b[r.i + 1]) << 8 | u32(r.b[r.i + 2]) << 16 | u32(r.b[r.i + 3]) << 24
	r.i += 4
	return v
}
r_f32 :: proc(r: ^Reader) -> f32 {
	return transmute(f32)r_u32(r)
}
r_bytes :: proc(r: ^Reader) -> []u8 {
	n := int(r_u32(r))
	if r.i + n > len(r.b) do return nil
	v := r.b[r.i:r.i + n]
	r.i += n
	return v
}

// ---- 命令の処理 ----

decode_frame :: proc(r: ^Reader) -> i32 {
	s := scale
	gpu.frame_begin(r_u32(r))
	for r.i < len(r.b) {
		switch r_u8(r) {
		case OP_RECT:
			x, y, w, h, rad := r_f32(r), r_f32(r), r_f32(r), r_f32(r), r_f32(r)
			fill, stroke := r_u32(r), r_u32(r)
			lw := r_f32(r)
			dash := r_u8(r)
			gpu.rect(x * s, y * s, w * s, h * s, rad * s, fill, stroke, lw * s, b32(dash != 0))
		case OP_TEXT:
			x, y, size := r_f32(r), r_f32(r), r_f32(r)
			color := r_u32(r)
			underline := r_u8(r)
			str := r_bytes(r)
			gpu.text(x * s, y * s, size * s, color, b32(underline != 0), raw_data(str), i32(len(str)))
		case OP_CLIP:
			x, y, w, h := r_f32(r), r_f32(r), r_f32(r), r_f32(r)
			gpu.clip(x * s, y * s, w * s, h * s)
		case OP_LAYER:
			gpu.layer(r_f32(r))
		case OP_POP:
			gpu.pop_state()
		case:
			return -1
		}
	}
	return gpu.frame_end()
}

capture_png :: proc(path: string) -> bool {
	if gpu.capture() == 0 do return false
	for gpu.capture_state() == 1 do wgpu.DevicePoll(gpu.state.device, true)
	if gpu.capture_state() != 2 do return false
	cpath := strings.clone_to_cstring(path, context.temp_allocator)
	return stbi.write_png(cpath, i32(gpu.W), i32(gpu.H), 4, gpu.capture_ptr(), c.ptrdiff_t(gpu.W) * 4) != 0
}

cursors: [sdl.SystemCursor]^sdl.Cursor

handle :: proc(body: []u8) {
	r := Reader{b = body}
	switch r_u8(&r) {
	case MSG_FONT:
		// フォントのデータは描画の間ずっと使うので、本体を手放さない
		ok := gpu.load_font(raw_data(body[1:]), i32(len(body) - 1))
		if !ok do emit(`{"t":"error","message":"フォントを読めない"}`)
		return
	case MSG_FRAME:
		t0 := time.tick_now()
		n := decode_frame(&r)
		ms := time.duration_milliseconds(time.tick_since(t0))
		emit(fmt.tprintf(`{{"t":"frame","n":%d,"ms":%.3f}}`, n, ms))
	case MSG_CAPTURE:
		path := string(r.b[r.i:])
		ok := capture_png(path)
		emit(fmt.tprintf(`{{"t":"captured","ok":%v,"path":%s,"w":%d,"h":%d}}`, ok, json_str(path), gpu.W, gpu.H))
	case MSG_IME:
		on := r_u8(&r) != 0
		x, y, w, h := r_f32(&r), r_f32(&r), r_f32(&r), r_f32(&r)
		if window != nil {
			if on {
				rect := sdl.Rect{i32(x), i32(y), max(1, i32(w)), max(1, i32(h))}
				_ = sdl.SetTextInputArea(window, &rect, 0)
				if !sdl.TextInputActive(window) do _ = sdl.StartTextInput(window)
			} else if sdl.TextInputActive(window) {
				_ = sdl.StopTextInput(window)
			}
		}
	case MSG_CURSOR:
		k := r_u8(&r)
		if window != nil {
			id: sdl.SystemCursor
			switch k {
			case 1:
				id = .POINTER
			case 2:
				id = .MOVE
			case 3:
				id = .EW_RESIZE
			case 4:
				id = .NS_RESIZE
			case 5:
				id = .NOT_ALLOWED
			case:
				id = .DEFAULT
			}
			if cursors[id] == nil do cursors[id] = sdl.CreateSystemCursor(id)
			_ = sdl.SetCursor(cursors[id])
		}
	case MSG_QUIT:
		quit = true
	}
	delete(body)
}

// ---- wgpu の準備 (wgpu-native は callback を ProcessEvents の中で呼ぶ) ----

adapter_done, device_done: bool

on_adapter :: proc "c" (status: wgpu.RequestAdapterStatus, adapter: wgpu.Adapter, message: string, u1, u2: rawptr) {
	adapter_done = true
	if status == .Success do gpu.state.adapter = adapter
}

on_device :: proc "c" (status: wgpu.RequestDeviceStatus, device: wgpu.Device, message: string, u1, u2: rawptr) {
	device_done = true
	if status == .Success do gpu.state.device = device
}

on_device_lost :: proc "c" (device: ^wgpu.Device, reason: wgpu.DeviceLostReason, message: string, u1, u2: rawptr) {}

on_error :: proc "c" (device: ^wgpu.Device, type: wgpu.ErrorType, message: string, u1, u2: rawptr) {
	context = runtime.default_context()
	emit(fmt.tprintf(`{{"t":"error","message":%s}}`, json_str(fmt.tprintf("wgpu: %v %s", type, message))))
}

init_gpu :: proc() -> string {
	gpu.state.ctx = context
	gpu.state.instance = wgpu.CreateInstance(nil)
	if gpu.state.instance == nil do return "wgpu の instance を作れない"
	if !headless do gpu.state.surface = sdlglue.GetSurface(gpu.state.instance, window)
	wgpu.InstanceRequestAdapter(gpu.state.instance, &{compatibleSurface = gpu.state.surface}, {mode = .AllowProcessEvents, callback = on_adapter})
	for !adapter_done do wgpu.InstanceProcessEvents(gpu.state.instance)
	if gpu.state.adapter == nil do return "adapter を取れない"
	wgpu.AdapterRequestDevice(gpu.state.adapter, &{
		deviceLostCallbackInfo = {mode = .AllowProcessEvents, callback = on_device_lost},
		uncapturedErrorCallbackInfo = {callback = on_error},
	}, {mode = .AllowProcessEvents, callback = on_device})
	for !device_done do wgpu.InstanceProcessEvents(gpu.state.instance)
	if gpu.state.device == nil do return "device を取れない"
	if headless {
		gpu.state.format = .RGBA8Unorm
		gpu.set_offscreen(true)
	} else {
		// 表面が推奨する形式のうち、sRGB でないもの (Canvas 2D と同じ色の計算にする)
		caps, _ := wgpu.SurfaceGetCapabilities(gpu.state.surface, gpu.state.adapter)
		formats := caps.formats[:caps.formatCount]
		gpu.state.format = formats[0] if len(formats) > 0 else .BGRA8Unorm
		for f in formats {
			if f == .BGRA8Unorm || f == .RGBA8Unorm {
				gpu.state.format = f
				break
			}
		}
	}
	gpu.setup(gpu.state.device)
	return ""
}

adapter_name :: proc() -> string {
	info, _ := wgpu.AdapterGetInfo(gpu.state.adapter)
	return fmt.tprintf("%s (%v)", info.device, info.backendType)
}

// ---- 窓 ----

resize_to_window :: proc() {
	pw, ph: c.int
	sdl.GetWindowSize(window, &win_w, &win_h)
	sdl.GetWindowSizeInPixels(window, &pw, &ph)
	scale = f32(pw) / f32(max(1, win_w))
	gpu.resize(u32(pw), u32(ph))
	emit(fmt.tprintf(`{{"t":"size","w":%d,"h":%d,"scale":%.3f}}`, win_w, win_h, scale))
}

/// SDL の scancode を、ブラウザの KeyboardEvent.code と同じ名前にする (shell のキーの対応はブラウザと共通にする)
code_name :: proc(sc: sdl.Scancode) -> string {
	#partial switch sc {
	case .A ..= .Z:
		return fmt.tprintf("Key%c", 'A' + rune(int(sc) - int(sdl.Scancode.A)))
	case ._1 ..= ._9:
		return fmt.tprintf("Digit%c", '1' + rune(int(sc) - int(sdl.Scancode._1)))
	case ._0:
		return "Digit0"
	case .RETURN:
		return "Enter"
	case .ESCAPE:
		return "Escape"
	case .BACKSPACE:
		return "Backspace"
	case .TAB:
		return "Tab"
	case .SPACE:
		return "Space"
	case .LEFT:
		return "ArrowLeft"
	case .RIGHT:
		return "ArrowRight"
	case .UP:
		return "ArrowUp"
	case .DOWN:
		return "ArrowDown"
	case .F6:
		return "F6"
	}
	return ""
}

/// KeyboardEvent.key に当たる名前 (文字のキーは文字入力 (text) で届くので、特別なキーだけ)
key_name :: proc(code: string) -> string {
	switch code {
	case "Enter", "Escape", "Backspace", "Tab", "ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown", "F6":
		return code
	case "Space":
		return " "
	}
	return ""
}

/// 標準入力を読む係 (窓のあるとき)。読んだものは queue に積み、SDL のイベントで本体を起こす
queue_mu: sync.Mutex
queue: [dynamic][]u8
wake_event: u32
reader_done: bool

reader_proc :: proc() {
	for {
		body := read_msg()
		sync.mutex_lock(&queue_mu)
		if body == nil do reader_done = true
		else do append(&queue, body)
		sync.mutex_unlock(&queue_mu)
		ev: sdl.Event
		ev.type = sdl.EventType(wake_event)
		_ = sdl.PushEvent(&ev)
		if body == nil do return
	}
}

drain :: proc() {
	// 溜まったものをまとめて引き取る (読む係は新しい配列に積み続ける)
	sync.mutex_lock(&queue_mu)
	msgs := queue
	queue = {}
	done := reader_done
	sync.mutex_unlock(&queue_mu)
	defer delete(msgs)
	// 古い FRAME は飛ばし、最後の FRAME だけを描く (Node が先に進んでいても、溜めない)
	last_frame := -1
	for m, i in msgs do if len(m) > 0 && m[0] == MSG_FRAME do last_frame = i
	for m, i in msgs {
		if len(m) > 0 && m[0] == MSG_FRAME && i != last_frame {
			delete(m)
			emit(`{"t":"frame","n":0,"ms":0,"skipped":true}`)
			continue
		}
		handle(m)
	}
	if done do quit = true
}

handle_event :: proc(ev: ^sdl.Event) {
	#partial switch ev.type {
	case .QUIT, .WINDOW_CLOSE_REQUESTED:
		emit(`{"t":"close"}`)
		quit = true
	case .WINDOW_RESIZED, .WINDOW_PIXEL_SIZE_CHANGED, .WINDOW_DISPLAY_SCALE_CHANGED:
		resize_to_window()
	case .MOUSE_BUTTON_DOWN:
		if ev.button.button == 1 do emit(fmt.tprintf(`{{"t":"down","x":%.2f,"y":%.2f,"clicks":%d}}`, ev.button.x, ev.button.y, ev.button.clicks))
	case .MOUSE_BUTTON_UP:
		if ev.button.button == 1 do emit(`{"t":"up"}`)
	case .MOUSE_MOTION:
		emit(fmt.tprintf(`{{"t":"move","x":%.2f,"y":%.2f}}`, ev.motion.x, ev.motion.y))
	case .MOUSE_WHEEL:
		// 1 目盛りを 40px にする (ブラウザの deltaMode=1 の 20 × 2 行ほど)
		emit(fmt.tprintf(`{{"t":"wheel","x":%.2f,"y":%.2f,"dy":%.2f}}`, ev.wheel.mouse_x, ev.wheel.mouse_y, -ev.wheel.y * 40))
	case .KEY_DOWN:
		code := code_name(ev.key.scancode)
		if code == "" do return
		alt := .LALT in ev.key.mod || .RALT in ev.key.mod
		shift := .LSHIFT in ev.key.mod || .RSHIFT in ev.key.mod
		emit(fmt.tprintf(`{{"t":"key","code":%s,"key":%s,"alt":%v,"shift":%v}}`, json_str(code), json_str(key_name(code)), alt, shift))
	case .TEXT_INPUT:
		emit(fmt.tprintf(`{{"t":"text","text":%s}}`, json_str(string(ev.text.text))))
	case .TEXT_EDITING:
		emit(fmt.tprintf(`{{"t":"preedit","text":%s}}`, json_str(string(ev.edit.text))))
	}
}

main :: proc() {
	args := os.args
	for i := 1; i < len(args); i += 1 {
		a := args[i]
		if a == "--headless" && i + 1 < len(args) {
			headless = true
			i += 1
			wh := strings.split(args[i], "x", context.temp_allocator)
			if len(wh) == 2 {
				w, _ := strconv.parse_int(wh[0])
				h, _ := strconv.parse_int(wh[1])
				win_w, win_h = i32(w), i32(h)
			}
		} else if a == "--scale" && i + 1 < len(args) {
			i += 1
			v, _ := strconv.parse_f64(args[i])
			scale = f32(v)
		}
	}

	if !headless {
		if !sdl.Init({.VIDEO}) {
			emit(fmt.tprintf(`{{"t":"error","message":%s}}`, json_str(fmt.tprintf("SDL を始められない: %s", sdl.GetError()))))
			return
		}
		window = sdl.CreateWindow("AI-native desktop", win_w, win_h, {.RESIZABLE, .HIGH_PIXEL_DENSITY})
		if window == nil {
			emit(fmt.tprintf(`{{"t":"error","message":%s}}`, json_str(fmt.tprintf("窓を作れない: %s", sdl.GetError()))))
			return
		}
	}
	if why := init_gpu(); why != "" {
		emit(fmt.tprintf(`{{"t":"error","message":%s}}`, json_str(why)))
		return
	}
	if headless do gpu.resize(u32(f32(win_w) * scale), u32(f32(win_h) * scale))
	else {
		pw, ph: c.int
		sdl.GetWindowSize(window, &win_w, &win_h)
		sdl.GetWindowSizeInPixels(window, &pw, &ph)
		scale = f32(pw) / f32(max(1, win_w))
		gpu.resize(u32(pw), u32(ph))
	}
	emit(fmt.tprintf(`{{"t":"ready","w":%d,"h":%d,"scale":%.3f,"headless":%v,"format":"%v","adapter":%s}}`, win_w, win_h, scale, headless, gpu.state.format, json_str(adapter_name())))

	if headless {
		// 窓なし: 標準入力を順に処理する (入力のイベントは来ない。Node のホストが代わりに送る)
		for !quit {
			body := read_msg()
			if body == nil do break
			handle(body)
			free_all(context.temp_allocator)
		}
		return
	}

	wake_event = sdl.RegisterEvents(1)
	t := thread.create_and_start(reader_proc)
	_ = t
	ev: sdl.Event
	for !quit {
		if sdl.WaitEventTimeout(&ev, 100) {
			if u32(ev.type) == wake_event do drain()
			else do handle_event(&ev)
			for !quit && sdl.PollEvent(&ev) {
				if u32(ev.type) == wake_event do drain()
				else do handle_event(&ev)
			}
		}
		free_all(context.temp_allocator)
	}
	sdl.DestroyWindow(window)
	sdl.Quit()
}
